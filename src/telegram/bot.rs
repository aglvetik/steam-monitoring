use std::{
    sync::Arc,
    time::{Duration, Instant},
};

use chrono::{Days, Duration as ChronoDuration, LocalResult, TimeZone, Utc};
use chrono_tz::Europe::Berlin;
use teloxide::{
    payloads::GetUpdatesSetters,
    prelude::{Request, Requester},
    types::{ChatId, Message, Update, UpdateKind},
    ApiError, Bot, RequestError,
};
use tokio::time::{sleep, timeout};
use tracing::{error, info, warn};

use crate::{
    config::Config,
    db::repository::Repository,
    deepseek::AiDescription,
    error::{AppError, AppResult},
    scheduler::{CheckRunner, CheckSummary},
    steam::{
        FreePromotion, PromotionEvaluation, PromotionSkipReason, SteamClient,
        SteamDbFreePromotionsReport, SteamGameData, SteamHttpDebugReport, SteamStoreSearchReport,
    },
    utils::{html::truncate_chars, time::format_berlin_datetime},
};

use super::{
    commands::TelegramCommand,
    formatting::{build_post, format_price},
    publisher::TelegramPublisher,
};

const GET_UPDATES_LONG_POLL_TIMEOUT_SECONDS: u32 = 30;
const GET_UPDATES_REQUEST_TIMEOUT_SECONDS: u64 = 75;
const PERMANENT_POLLING_ERROR_THRESHOLD: u32 = 3;

pub async fn register_commands(bot: &Bot) -> AppResult<()> {
    bot.set_my_commands(TelegramCommand::public_menu_commands())
        .await?;
    Ok(())
}

enum PollingErrorDisposition {
    Retry {
        sleep_for: Duration,
        context: String,
    },
    Permanent {
        context: String,
    },
    Fatal {
        context: String,
    },
}

pub async fn run(
    bot: Bot,
    repo: Arc<Repository>,
    config: Arc<Config>,
    steam: Arc<SteamClient>,
    check_runner: Arc<CheckRunner>,
) -> AppResult<()> {
    let me = bot.get_me().await?;
    let bot_username = me.user.username.unwrap_or_default();
    let mut offset = 0i32;
    let heartbeat_interval = Duration::from_secs(config.telegram_polling_heartbeat_seconds.max(1));
    let error_sleep = Duration::from_secs(config.telegram_polling_error_sleep_seconds.max(1));
    let stale_after = Duration::from_secs(config.telegram_polling_stale_seconds.max(1));
    let request_timeout = Duration::from_secs(GET_UPDATES_REQUEST_TIMEOUT_SECONDS);
    let mut last_successful_get_updates_at = Instant::now();
    let mut last_processed_update_at: Option<Instant> = None;
    let mut last_update_id: Option<u32> = None;
    let mut next_heartbeat_at = Instant::now() + heartbeat_interval;
    let mut consecutive_permanent_errors = 0u32;

    info!(
        heartbeat_seconds = heartbeat_interval.as_secs(),
        stale_seconds = stale_after.as_secs(),
        error_sleep_seconds = error_sleep.as_secs(),
        request_timeout_seconds = request_timeout.as_secs(),
        "Telegram polling started"
    );

    loop {
        maybe_log_polling_heartbeat(
            &mut next_heartbeat_at,
            heartbeat_interval,
            last_update_id,
            last_successful_get_updates_at,
            last_processed_update_at,
        );

        if last_successful_get_updates_at.elapsed() > stale_after {
            error!(
                last_update_id = last_update_id
                    .map(|value| value.to_string())
                    .unwrap_or_else(|| "none".to_string()),
                seconds_since_success = last_successful_get_updates_at.elapsed().as_secs(),
                "Telegram polling stale, shutting down for systemd restart"
            );
            return Err(AppError::Other(
                "Telegram polling stale, shutting down for systemd restart".to_string(),
            ));
        }

        let request = bot
            .get_updates()
            .offset(offset)
            .limit(50)
            .timeout(GET_UPDATES_LONG_POLL_TIMEOUT_SECONDS)
            .send();
        let updates = match timeout(request_timeout, request).await {
            Ok(Ok(updates)) => {
                last_successful_get_updates_at = Instant::now();
                consecutive_permanent_errors = 0;
                updates
            }
            Ok(Err(error)) => match classify_polling_error(error, error_sleep) {
                PollingErrorDisposition::Retry { sleep_for, context } => {
                    warn!("{context}");
                    sleep(sleep_for).await;
                    continue;
                }
                PollingErrorDisposition::Permanent { context } => {
                    consecutive_permanent_errors += 1;
                    error!(
                        consecutive_permanent_errors,
                        "Telegram polling permanent error: {context}"
                    );

                    if consecutive_permanent_errors >= PERMANENT_POLLING_ERROR_THRESHOLD {
                        error!("Telegram polling fatal error, shutting down: {context}");
                        return Err(AppError::Other(context));
                    }

                    sleep(error_sleep).await;
                    continue;
                }
                PollingErrorDisposition::Fatal { context } => {
                    error!("Telegram polling fatal error, shutting down: {context}");
                    return Err(AppError::Other(context));
                }
            },
            Err(_) => {
                warn!(
                    timeout_seconds = request_timeout.as_secs(),
                    "Telegram getUpdates temporary error, retrying: request timed out"
                );
                sleep(error_sleep).await;
                continue;
            }
        };

        for update in updates {
            last_update_id = Some(update.id.0);
            last_processed_update_at = Some(Instant::now());
            offset = next_offset(&update);

            let Some(message) = extract_message(update) else {
                continue;
            };
            let Some(text) = message.text() else {
                continue;
            };
            let Some(command) = TelegramCommand::parse(text, &bot_username) else {
                continue;
            };

            if let Err(error) = handle_command(
                &bot,
                &message,
                command,
                &repo,
                &config,
                &steam,
                check_runner.clone(),
            )
            .await
            {
                error!("Telegram command failed: {error}");
                if let Err(send_error) = bot
                    .send_message(
                        message.chat.id,
                        "Не удалось обработать команду. Попробуйте еще раз позже.",
                    )
                    .await
                {
                    warn!("Failed to send Telegram command error message: {send_error}");
                }
            }
        }
    }
}

fn maybe_log_polling_heartbeat(
    next_heartbeat_at: &mut Instant,
    heartbeat_interval: Duration,
    last_update_id: Option<u32>,
    last_successful_get_updates_at: Instant,
    last_processed_update_at: Option<Instant>,
) {
    let now = Instant::now();
    if now < *next_heartbeat_at {
        return;
    }

    info!(
        last_update_id = last_update_id
            .map(|value| value.to_string())
            .unwrap_or_else(|| "none".to_string()),
        seconds_since_successful_get_updates = last_successful_get_updates_at.elapsed().as_secs(),
        seconds_since_processed_update = last_processed_update_at
            .map(|value| value.elapsed().as_secs().to_string())
            .unwrap_or_else(|| "n/a".to_string()),
        "Telegram polling alive"
    );

    *next_heartbeat_at = now + heartbeat_interval;
}

fn classify_polling_error(error: RequestError, default_sleep: Duration) -> PollingErrorDisposition {
    match error {
        RequestError::RetryAfter(seconds) => PollingErrorDisposition::Retry {
            sleep_for: Duration::from_secs(seconds.seconds().max(1).into()),
            context: format!(
                "Telegram getUpdates temporary error, retrying: retry after {} seconds",
                seconds.seconds()
            ),
        },
        RequestError::Network(network_error) => PollingErrorDisposition::Retry {
            sleep_for: default_sleep,
            context: format!(
                "Telegram getUpdates temporary error, retrying: {}",
                describe_network_error(&network_error)
            ),
        },
        RequestError::InvalidJson { raw, source } if invalid_json_looks_temporary(&raw) => {
            PollingErrorDisposition::Retry {
                sleep_for: default_sleep,
                context: format!(
                    "Telegram getUpdates temporary error, retrying: invalid JSON from Telegram ({source})"
                ),
            }
        }
        RequestError::Api(ApiError::InvalidToken) => PollingErrorDisposition::Fatal {
            context: "Telegram polling fatal error, shutting down: invalid bot token".to_string(),
        },
        RequestError::Api(ApiError::CantGetUpdates) => PollingErrorDisposition::Fatal {
            context:
                "Telegram polling fatal error, shutting down: webhook is active for this bot"
                    .to_string(),
        },
        RequestError::Api(ApiError::TerminatedByOtherGetUpdates) => {
            PollingErrorDisposition::Fatal {
                context:
                    "Telegram polling fatal error, shutting down: another getUpdates consumer is running"
                        .to_string(),
            }
        }
        other => PollingErrorDisposition::Permanent {
            context: other.to_string(),
        },
    }
}

fn describe_network_error(error: &reqwest_011::Error) -> String {
    if error.is_timeout() {
        format!("network timeout: {error}")
    } else if error.is_connect() {
        format!("network connect error: {error}")
    } else if error.is_request() {
        format!("network request error: {error}")
    } else if error.is_body() {
        format!("network body read error: {error}")
    } else {
        format!("network error: {error}")
    }
}

fn invalid_json_looks_temporary(raw: &str) -> bool {
    let normalized = raw.trim().to_ascii_lowercase();
    normalized.starts_with('<')
        || normalized.contains("bad gateway")
        || normalized.contains("gateway timeout")
        || normalized.contains("service unavailable")
        || normalized.contains("internal server error")
        || normalized.contains("<html")
}

async fn handle_command(
    bot: &Bot,
    msg: &Message,
    command: TelegramCommand,
    repo: &Repository,
    config: &Config,
    steam: &SteamClient,
    check_runner: Arc<CheckRunner>,
) -> AppResult<()> {
    match command {
        TelegramCommand::Start => handle_start(bot, msg, repo).await,
        TelegramCommand::On => handle_on(bot, msg, repo).await,
        TelegramCommand::Off => handle_off(bot, msg, repo, config).await,
        TelegramCommand::Status => handle_status(bot, msg, repo, config).await,
        TelegramCommand::CheckNow => handle_check_now(bot, msg, config, check_runner).await,
        TelegramCommand::DebugSteamHttp => handle_debug_steam_http(bot, msg, config, steam).await,
        TelegramCommand::DebugStoreSearch => {
            handle_debug_store_search(bot, msg, config, steam).await
        }
        TelegramCommand::DebugSteamDb => handle_debug_steamdb(bot, msg, config, steam).await,
        TelegramCommand::DebugFreeUntil { appid } => {
            handle_debug_free_until(bot, msg, config, steam, appid).await
        }
        TelegramCommand::TestPost => handle_test_post(bot, msg, repo, config).await,
        TelegramCommand::PreviewApp { appid } => {
            handle_preview_app(bot, msg, config, steam, appid).await
        }
    }
}

async fn handle_start(bot: &Bot, msg: &Message, repo: &Repository) -> AppResult<()> {
    if msg.chat.is_private() {
        set_chat_enabled(repo, msg, true).await?;
        bot.send_message(
            msg.chat.id,
            "Готово ✅ Я буду присылать тебе сообщения, когда в Steam появятся временно бесплатные игры. Чтобы отключить рассылку, напиши /off.",
        )
        .await?;
    } else {
        touch_chat(repo, msg, false).await?;
        bot.send_message(
            msg.chat.id,
            "Чтобы включить рассылку бесплатных игр Steam в этой группе, напишите /on. Чтобы выключить — /off.",
        )
        .await?;
    }

    Ok(())
}

async fn handle_on(bot: &Bot, msg: &Message, repo: &Repository) -> AppResult<()> {
    set_chat_enabled(repo, msg, true).await?;

    let reply = if msg.chat.is_private() {
        "Рассылка включена ✅"
    } else {
        "Рассылка бесплатных игр Steam включена для этой группы ✅"
    };

    bot.send_message(msg.chat.id, reply).await?;
    Ok(())
}

async fn handle_off(bot: &Bot, msg: &Message, repo: &Repository, config: &Config) -> AppResult<()> {
    let chat_id = msg.chat.id.0.to_string();
    if msg.chat.is_channel() && config.telegram_main_channel_id.as_deref() == Some(chat_id.as_str())
    {
        bot.send_message(
            msg.chat.id,
            "Основной канал из конфигурации всегда остается целью публикации.",
        )
        .await?;
        return Ok(());
    }

    set_chat_enabled(repo, msg, false).await?;

    let reply = if msg.chat.is_private() {
        "Рассылка выключена. Чтобы снова включить, напиши /on."
    } else {
        "Рассылка бесплатных игр Steam выключена для этой группы."
    };

    bot.send_message(msg.chat.id, reply).await?;
    Ok(())
}

async fn handle_status(
    bot: &Bot,
    msg: &Message,
    repo: &Repository,
    config: &Config,
) -> AppResult<()> {
    if !ensure_admin(bot, msg, config).await? {
        return Ok(());
    }

    let chat_id = msg.chat.id.0.to_string();
    let record = repo.get_chat(&chat_id).await?;
    let enabled = record.as_ref().map(|item| item.enabled).unwrap_or(false);
    let is_main_channel = config.telegram_main_channel_id.as_deref() == Some(chat_id.as_str());

    let reply = format!(
        "Статус текущего чата\n\
         Чат: {}\n\
         Идентификатор: {}\n\
         Тип: {}\n\
         Рассылка: {}\n\
         Основной канал: {}\n\
         RUN_STARTUP_CHECK: {}\n\
         STEAM_COUNTRY: {}\n\
         STEAM_LANGUAGE: {}",
        chat_title(msg),
        public_identifier(msg),
        chat_type(msg),
        if enabled {
            "включена"
        } else {
            "выключена"
        },
        if is_main_channel { "да" } else { "нет" },
        config.run_startup_check,
        config.steam_country,
        config.steam_language,
    );

    send_admin_reply(bot, msg.chat.id, reply, "Failed to send /status reply").await;
    Ok(())
}

async fn handle_check_now(
    bot: &Bot,
    msg: &Message,
    config: &Config,
    check_runner: Arc<CheckRunner>,
) -> AppResult<()> {
    let user_id = admin_user_id(msg);
    if !config.is_admin(user_id) {
        bot.send_message(msg.chat.id, "Эта команда доступна только администратору.")
            .await?;
        return Ok(());
    }

    bot.send_message(msg.chat.id, "Запускаю проверку Steam...")
        .await?;

    match check_runner
        .clone()
        .run_guarded(format!("telegram:/check_now by {user_id}"))
        .await
    {
        Ok(summary) => {
            let reply = format_check_summary(&summary);
            if let Err(error) = send_message_with_retry(bot, msg.chat.id, &reply).await {
                warn!("Steam check completed, but failed to send Telegram summary: {error}");
            }
        }
        Err(error) => {
            error!("Manual Steam check failed: {error}");
            send_admin_reply(
                bot,
                msg.chat.id,
                format!(
                    "Проверка Steam завершилась с ошибкой.\n\
                     Вероятная причина: не удалось получить данные Steam.\n\
                     Ошибка: {}",
                    short_error_message(&error.to_string())
                ),
                "Failed to send /check_now error reply",
            )
            .await;
        }
    }

    Ok(())
}

async fn handle_debug_steam_http(
    bot: &Bot,
    msg: &Message,
    config: &Config,
    steam: &SteamClient,
) -> AppResult<()> {
    if !ensure_admin(bot, msg, config).await? {
        return Ok(());
    }

    if !msg.chat.is_private() {
        send_admin_reply(
            bot,
            msg.chat.id,
            "Выполните /debug_steam_http в личном чате с ботом.",
            "Failed to send /debug_steam_http private-chat hint",
        )
        .await;
        return Ok(());
    }

    let report = steam.debug_featured_categories_http().await;
    send_admin_reply(
        bot,
        msg.chat.id,
        format_debug_report(&report),
        "Failed to send /debug_steam_http reply",
    )
    .await;
    Ok(())
}

async fn handle_debug_steamdb(
    bot: &Bot,
    msg: &Message,
    config: &Config,
    steam: &SteamClient,
) -> AppResult<()> {
    if !ensure_admin(bot, msg, config).await? {
        return Ok(());
    }

    if !msg.chat.is_private() {
        send_admin_reply(
            bot,
            msg.chat.id,
            "Выполните /debug_steamdb в личном чате с ботом.",
            "Failed to send /debug_steamdb private-chat hint",
        )
        .await;
        return Ok(());
    }

    let report = steam.debug_steamdb_free_promotions().await;
    send_admin_reply(
        bot,
        msg.chat.id,
        format_steamdb_debug_report(&report),
        "Failed to send /debug_steamdb reply",
    )
    .await;
    Ok(())
}

async fn handle_debug_store_search(
    bot: &Bot,
    msg: &Message,
    config: &Config,
    steam: &SteamClient,
) -> AppResult<()> {
    if !ensure_admin(bot, msg, config).await? {
        return Ok(());
    }

    if !msg.chat.is_private() {
        send_admin_reply(
            bot,
            msg.chat.id,
            "Выполните /debug_store_search в личном чате с ботом.",
            "Failed to send /debug_store_search private-chat hint",
        )
        .await;
        return Ok(());
    }

    let report = steam.debug_store_search_free_specials().await;
    send_admin_reply(
        bot,
        msg.chat.id,
        format_store_search_debug_report(&report),
        "Failed to send /debug_store_search reply",
    )
    .await;
    Ok(())
}

async fn handle_debug_free_until(
    bot: &Bot,
    msg: &Message,
    config: &Config,
    steam: &SteamClient,
    appid_arg: Option<String>,
) -> AppResult<()> {
    if !ensure_admin(bot, msg, config).await? {
        return Ok(());
    }

    if !msg.chat.is_private() {
        send_admin_reply(
            bot,
            msg.chat.id,
            "Выполните /debug_free_until в личном чате с ботом.",
            "Failed to send /debug_free_until private-chat hint",
        )
        .await;
        return Ok(());
    }

    let Some(appid_arg) = appid_arg else {
        send_admin_reply(
            bot,
            msg.chat.id,
            "Использование: /debug_free_until <appid>",
            "Failed to send /debug_free_until usage reply",
        )
        .await;
        return Ok(());
    };

    let Ok(appid) = appid_arg.trim().parse::<u32>() else {
        send_admin_reply(
            bot,
            msg.chat.id,
            "App ID должен быть положительным числом.",
            "Failed to send /debug_free_until validation reply",
        )
        .await;
        return Ok(());
    };

    match steam.lookup_app_store_page_free_until(appid).await {
        Ok(report) => {
            let status = if report.free_until.is_some() {
                "найдено"
            } else {
                "не найдено"
            };
            let free_until = report
                .free_until
                .map(|value| format_berlin_datetime(&value))
                .unwrap_or_else(|| "не найдено".to_string());
            let fragment = report.matched_text.unwrap_or_else(|| "n/a".to_string());
            let reply = format!(
                "Проверка free_until Steam Store\n\n\
                 AppID: {}\n\
                 URL: {}\n\
                 Response bytes: {}\n\
                 Статус: {}\n\
                 Бесплатно до: {}\n\
                 Диагностика: {}\n\
                 Фрагмент: {}",
                report.appid,
                report.url,
                report.response_bytes,
                status,
                free_until,
                report.diagnostic,
                truncate_chars(&fragment, 220),
            );

            send_admin_reply(
                bot,
                msg.chat.id,
                reply,
                "Failed to send /debug_free_until reply",
            )
            .await;
        }
        Err(error) => {
            error!("Steam store page free_until lookup failed for app {appid}: {error}");
            send_admin_reply(
                bot,
                msg.chat.id,
                format!(
                    "Не удалось получить страницу Steam: {}",
                    short_error_message(&error.to_string())
                ),
                "Failed to send /debug_free_until error reply",
            )
            .await;
        }
    }

    Ok(())
}

async fn handle_test_post(
    bot: &Bot,
    msg: &Message,
    repo: &Repository,
    config: &Config,
) -> AppResult<()> {
    if !ensure_admin(bot, msg, config).await? {
        return Ok(());
    }

    let target_chat_ids = repo
        .resolve_publish_target_chat_ids(config.telegram_main_channel_id.as_deref())
        .await?;
    let post = build_test_post();
    let publisher = TelegramPublisher::new(bot.clone());

    let mut success_count = 0usize;
    let mut failed_count = 0usize;

    for chat_id in &target_chat_ids {
        match publisher.publish_to_chat(chat_id, &post).await {
            Ok(_) => {
                success_count += 1;
            }
            Err(error) => {
                failed_count += 1;
                warn!("Test post failed for chat {chat_id}: {error}");
            }
        }
    }

    let summary = format!(
        "Тестовая рассылка завершена.\n\
         Чатов в рассылке: {}\n\
         Успешно: {}\n\
         С ошибкой: {}",
        target_chat_ids.len(),
        success_count,
        failed_count
    );
    send_admin_reply(bot, msg.chat.id, summary, "Failed to send /test_post reply").await;
    Ok(())
}

async fn handle_preview_app(
    bot: &Bot,
    msg: &Message,
    config: &Config,
    steam: &SteamClient,
    appid_arg: Option<String>,
) -> AppResult<()> {
    if !ensure_admin(bot, msg, config).await? {
        return Ok(());
    }

    let Some(appid_arg) = appid_arg else {
        send_admin_reply(
            bot,
            msg.chat.id,
            "Использование: /preview_app <appid>",
            "Failed to send /preview_app usage reply",
        )
        .await;
        return Ok(());
    };

    let Ok(appid) = appid_arg.trim().parse::<i64>() else {
        send_admin_reply(
            bot,
            msg.chat.id,
            "App ID должен быть положительным числом.",
            "Failed to send /preview_app validation reply",
        )
        .await;
        return Ok(());
    };

    if appid <= 0 {
        send_admin_reply(
            bot,
            msg.chat.id,
            "App ID должен быть положительным числом.",
            "Failed to send /preview_app validation reply",
        )
        .await;
        return Ok(());
    }

    let details = match steam.fetch_app_details(appid).await {
        Ok(Some(details)) => details,
        Ok(None) => {
            send_admin_reply(
                bot,
                msg.chat.id,
                "Steam не вернул данные для этого appid или приложение недоступно в выбранном регионе.",
                "Failed to send /preview_app unavailable reply",
            )
            .await;
            return Ok(());
        }
        Err(error) => {
            error!("Steam preview fetch failed for app {appid}: {error}");
            send_admin_reply(
                bot,
                msg.chat.id,
                format!(
                    "Не удалось получить данные Steam: {}",
                    short_error_message(&error.to_string())
                ),
                "Failed to send /preview_app error reply",
            )
            .await;
            return Ok(());
        }
    };

    let game = steam.build_game_data(&details, None);
    let evaluation = steam.evaluate_free_promotion(&details, None);
    let price = details.price_overview.as_ref();

    let initial_price = price
        .map(|value| format_price(value.initial, Some(&value.currency)))
        .unwrap_or_else(|| "нет данных".to_string());
    let final_price = price
        .map(|value| format_price(value.r#final, Some(&value.currency)))
        .unwrap_or_else(|| "нет данных".to_string());
    let discount_percent = price
        .map(|value| format!("{}%", value.discount_percent))
        .unwrap_or_else(|| "нет данных".to_string());

    let reply = match evaluation {
        PromotionEvaluation::Publishable(promotion) => format!(
            "🔎 Предпросмотр Steam App\n\n\
             AppID: {appid}\n\
             Название: {}\n\
             Тип: {}\n\
             Free-to-play: {}\n\
             Обычная цена: {}\n\
             Текущая цена: {}\n\
             Скидка: {}\n\
             Бесплатно до: {}\n\
             Steam URL: {}\n\n\
             Результат: будет опубликовано",
            game.name,
            game.kind.as_deref().unwrap_or("unknown"),
            bool_ru(game.is_free_to_play),
            initial_price,
            final_price,
            discount_percent,
            promotion
                .free_until
                .map(|value| value.to_rfc3339())
                .unwrap_or_else(|| "Steam не указал".to_string()),
            game.steam_url,
        ),
        PromotionEvaluation::Skipped(reason) => format!(
            "🔎 Предпросмотр Steam App\n\n\
             AppID: {appid}\n\
             Название: {}\n\
             Тип: {}\n\
             Free-to-play: {}\n\
             Обычная цена: {}\n\
             Текущая цена: {}\n\
             Скидка: {}\n\n\
             Результат: не будет опубликовано\n\
             Причина: {}",
            game.name,
            game.kind.as_deref().unwrap_or("unknown"),
            bool_ru(game.is_free_to_play),
            initial_price,
            final_price,
            discount_percent,
            reason.preview_reason_ru(),
        ),
    };

    send_admin_reply(bot, msg.chat.id, reply, "Failed to send /preview_app reply").await;
    Ok(())
}

async fn ensure_admin(bot: &Bot, msg: &Message, config: &Config) -> AppResult<bool> {
    if config.is_admin(admin_user_id(msg)) {
        return Ok(true);
    }

    bot.send_message(msg.chat.id, "Эта команда доступна только администратору.")
        .await?;
    Ok(false)
}

fn format_check_summary(summary: &CheckSummary) -> String {
    let mut lines = vec![
        "Проверка Steam завершена.".to_string(),
        format!("Steam candidates: {}", summary.steam_candidate_app_ids),
        format!("Steam prefilter passed: {}", summary.steam_prefilter_passed),
        format!(
            "Steam prefilter skipped: {}",
            summary.steam_prefilter_skipped
        ),
        format!(
            "Steam без данных цены: {}",
            summary.steam_missing_candidate_price_data
        ),
        format!(
            "Steam Store Search entries parsed: {}",
            summary.steam_search_entries_parsed
        ),
        format!(
            "Steam Store Search 100% free candidates: {}",
            summary.steam_search_free_candidates
        ),
        format!(
            "Steam Store Search skipped: {}",
            summary.steam_search_skipped
        ),
        format!(
            "Steam Store Search missing appid: {}",
            summary.steam_search_missing_appid
        ),
        format!(
            "Steam Store Search missing price data: {}",
            summary.steam_search_missing_price_data
        ),
        format!("SteamDB entries parsed: {}", summary.steamdb_entries_parsed),
        format!("SteamDB Free to Keep: {}", summary.steamdb_free_to_keep),
        format!("Запросов appdetails: {}", summary.appdetails_requests),
        format!("Детали получены: {}", summary.app_details_fetched),
        format!("Дат окончания найдено: {}", summary.free_until_found),
        format!("Валидных акций: {}", summary.valid_free_promotions),
        format!(
            "Опубликовано сообщений: {}",
            summary.posts_successfully_sent
        ),
        format!("Дубликатов пропущено: {}", summary.duplicate_posts_skipped),
        format!("Ошибок: {}", summary.errors),
    ];

    if summary.app_details_unavailable > 0 {
        lines.push(format!(
            "Данные Steam недоступны: {}",
            summary.app_details_unavailable
        ));
    }

    if summary.rate_limited > 0 {
        lines.push(format!("Ограничение Steam / 429: {}", summary.rate_limited));
    }

    if let Some(error) = summary.steam_search_source_error.as_deref() {
        if error.contains("disabled by config") {
            lines.push("Steam Store Search: disabled".to_string());
        } else {
            lines.push("Steam Store Search: ошибка загрузки, источник пропущен.".to_string());
        }
    }

    if let Some(error) = summary.steamdb_source_error.as_deref() {
        if error.contains("disabled by config") {
            lines.push("SteamDB: disabled".to_string());
        } else if summary.steamdb_http_status == Some(403) || error.contains("403") {
            lines.push("SteamDB: 403 Forbidden, source skipped".to_string());
        } else {
            lines.push("SteamDB: ошибка загрузки, источник пропущен.".to_string());
        }
    }

    if summary.steamdb_parse_errors > 0 {
        lines.push(format!(
            "SteamDB parse errors: {}",
            summary.steamdb_parse_errors
        ));
    }

    if !summary.skip_breakdown.is_empty() {
        let mut breakdown = summary
            .skip_breakdown
            .iter()
            .filter(|(reason, count)| {
                **count > 0 && !matches!(reason, PromotionSkipReason::AppDetailsUnavailable)
            })
            .map(|(reason, count)| (reason.breakdown_label_ru(), *count))
            .collect::<Vec<_>>();
        breakdown.sort_by(|left, right| left.0.cmp(right.0));

        if !breakdown.is_empty() {
            lines.push(String::new());
            lines.push("Причины пропуска:".to_string());
            for (label, count) in breakdown {
                lines.push(format!("- {label}: {count}"));
            }
        }
    }

    if let Some(reason) = summary.likely_reason_ru() {
        lines.push(String::new());
        lines.push(format!("Вероятная причина: {reason}"));
    }

    if summary.rate_limited > 0 {
        lines.push(String::new());
        lines.push(
            "Steam временно ограничил часть запросов. Следующая проверка попробует снова."
                .to_string(),
        );
    }

    lines.join("\n")
}

fn format_store_search_debug_report(report: &SteamStoreSearchReport) -> String {
    let mut lines = vec![
        format!("Steam Store Search endpoint: {}", report.url),
        format!(
            "HTTP status: {}",
            report
                .http_status
                .map(|value| value.to_string())
                .unwrap_or_else(|| "n/a".to_string())
        ),
        format!(
            "Response bytes: {}",
            report
                .response_bytes
                .map(|value| value.to_string())
                .unwrap_or_else(|| "n/a".to_string())
        ),
        format!("Parsed entries: {}", report.parsed_entries),
        format!(
            "Accepted 100% free candidates: {}",
            report.accepted_candidates
        ),
        format!("Skipped: {}", report.skipped_count),
        format!("Missing appid: {}", report.missing_appid_skipped),
        format!("Missing price data: {}", report.missing_price_data),
    ];

    if let Some(error) = report.error.as_deref() {
        lines.push(format!("Error: {error}"));
        if report.http_status == Some(403) || error.contains("403") {
            lines.push("SteamDB заблокировал запрос с VPS, источник пропущен.".to_string());
        }
    }

    if !report.accepted_entries.is_empty() {
        lines.push(String::new());
        lines.push("Accepted entries:".to_string());

        for entry in report.accepted_entries.iter().take(10) {
            lines.push(format!(
                "- {} | {} | {} -> {} | {}% | {}",
                entry.appid,
                entry.name,
                format_price(entry.regular_price_cents, entry.currency.as_deref()),
                format_price(entry.final_price_cents, entry.currency.as_deref()),
                entry.discount_percent,
                entry.store_url
            ));
        }
    }

    lines.join("\n")
}

fn format_steamdb_debug_report(report: &SteamDbFreePromotionsReport) -> String {
    let mut lines = vec![
        format!("SteamDB URL: {}", report.url),
        format!(
            "HTTP status: {}",
            report
                .http_status
                .map(|value| value.to_string())
                .unwrap_or_else(|| "n/a".to_string())
        ),
        format!("SteamDB entries parsed: {}", report.entries_parsed),
        format!("Free to Keep accepted: {}", report.free_to_keep_accepted),
        format!("Play For Free skipped: {}", report.play_for_free_skipped),
        format!("Missing appid skipped: {}", report.missing_appid_skipped),
        format!("Expired skipped: {}", report.expired_skipped),
        format!("Parse errors: {}", report.parse_errors),
    ];

    if report.obvious_non_game_skipped > 0 {
        lines.push(format!(
            "Obvious non-game skipped: {}",
            report.obvious_non_game_skipped
        ));
    }

    if let Some(bytes) = report.response_bytes {
        lines.push(format!("Response bytes: {bytes}"));
    }

    if let Some(error) = report.error.as_deref() {
        lines.push(format!("Error: {error}"));
    }

    if !report.accepted_entries.is_empty() {
        lines.push(String::new());
        lines.push("Accepted entries:".to_string());

        for entry in report.accepted_entries.iter().take(10) {
            let started = entry
                .started_at
                .map(|value| format_berlin_datetime(&value))
                .unwrap_or_else(|| "не указано".to_string());
            let expires = entry
                .expires_at
                .map(|value| format_berlin_datetime(&value))
                .unwrap_or_else(|| "не указано".to_string());
            lines.push(format!(
                "- {} | {} | старт: {} | до: {} | {}",
                entry.appid, entry.name, started, expires, entry.store_url
            ));
        }
    }

    lines.join("\n")
}

fn format_debug_report(report: &SteamHttpDebugReport) -> String {
    format!(
        "Steam endpoint: {}\n\
         HTTP: {}\n\
         Response bytes: {}\n\
         JSON parse: {}\n\
         Candidate app ids: {}\n\
         Stage: {}\n\
         Error: {}",
        report.url,
        if report.http_success { "ok" } else { "failed" },
        report
            .response_bytes
            .map(|value| value.to_string())
            .unwrap_or_else(|| "n/a".to_string()),
        if report.json_parse_success {
            "ok"
        } else {
            "failed"
        },
        report
            .candidate_app_ids
            .map(|value| value.to_string())
            .unwrap_or_else(|| "n/a".to_string()),
        report.stage,
        report.error.as_deref().unwrap_or("none"),
    )
}

fn build_test_post() -> super::formatting::FormattedPost {
    let game = SteamGameData {
        appid: 0,
        name: "Test Steam Game".to_string(),
        steam_url: "https://store.steampowered.com/".to_string(),
        kind: Some("game".to_string()),
        is_free_to_play: false,
        header_image: None,
        capsule_image: None,
        short_description: Some(
            "Это тестовый пост для проверки публикации бесплатных игр Steam.".to_string(),
        ),
        genres: vec![
            "Тест".to_string(),
            "Steam".to_string(),
            "Бесплатно".to_string(),
        ],
        categories: Vec::new(),
    };

    let promotion = FreePromotion {
        appid: 0,
        currency: Some("EUR".to_string()),
        regular_price_cents: Some(1999),
        final_price_cents: Some(0),
        discount_percent: Some(100),
        free_until: Some(test_free_until_tomorrow_berlin()),
        source: "telegram_test_post".to_string(),
    };

    let ai = AiDescription {
        appid: 0,
        language: "russian".to_string(),
        short_description: "Это тестовый пост для проверки публикации бесплатных игр Steam."
            .to_string(),
        why_play: "Если этот пост появился в канале, значит Telegram-публикация, HTML-форматирование и рассылка работают корректно."
            .to_string(),
        tags_line: Some("Тест / Steam / Бесплатно".to_string()),
        model: None,
    };

    build_post(&game, &promotion, &ai)
}

fn short_error_message(error: &str) -> String {
    truncate_chars(error.trim(), 180)
}

async fn send_admin_reply(bot: &Bot, chat_id: ChatId, text: impl Into<String>, context: &str) {
    let text = text.into();
    if let Err(error) = send_message_with_retry(bot, chat_id, &text).await {
        warn!("{context}: {error}");
    }
}

async fn send_message_with_retry(bot: &Bot, chat_id: ChatId, text: &str) -> AppResult<()> {
    let retry_delays = [
        Duration::from_secs(1),
        Duration::from_secs(3),
        Duration::from_secs(7),
    ];
    let mut last_error = None;

    for attempt in 0..=retry_delays.len() {
        match bot.send_message(chat_id, text.to_string()).await {
            Ok(_) => return Ok(()),
            Err(error) => {
                warn!(
                    attempt = attempt + 1,
                    "Telegram reply send attempt failed: {error}"
                );
                last_error = Some(error);

                if let Some(delay) = retry_delays.get(attempt) {
                    sleep(*delay).await;
                }
            }
        }
    }

    Err(last_error.map(Into::into).unwrap_or_else(|| {
        crate::error::AppError::Other("failed to send Telegram reply".to_string())
    }))
}

fn bool_ru(value: bool) -> &'static str {
    if value {
        "да"
    } else {
        "нет"
    }
}

fn test_free_until_tomorrow_berlin() -> chrono::DateTime<Utc> {
    let berlin_now = Utc::now().with_timezone(&Berlin);
    let tomorrow_date = berlin_now.date_naive().checked_add_days(Days::new(1));

    if let Some(tomorrow_date) = tomorrow_date {
        if let Some(naive) = tomorrow_date.and_hms_opt(19, 0, 0) {
            match Berlin.from_local_datetime(&naive) {
                LocalResult::Single(datetime) => return datetime.with_timezone(&Utc),
                LocalResult::Ambiguous(datetime, _) => return datetime.with_timezone(&Utc),
                LocalResult::None => {}
            }
        }
    }

    Utc::now() + ChronoDuration::days(1)
}

fn admin_user_id(msg: &Message) -> i64 {
    msg.from
        .as_ref()
        .map(|user| user.id.0 as i64)
        .unwrap_or_default()
}

async fn touch_chat(repo: &Repository, msg: &Message, default_enabled: bool) -> AppResult<()> {
    let chat_id = msg.chat.id.0.to_string();
    let existing = repo.get_chat(&chat_id).await?;
    let enabled = existing.map(|item| item.enabled).unwrap_or(default_enabled);
    repo.upsert_chat(&chat_id, chat_type(msg), Some(&chat_title(msg)), enabled)
        .await
}

async fn set_chat_enabled(repo: &Repository, msg: &Message, enabled: bool) -> AppResult<()> {
    let chat_id = msg.chat.id.0.to_string();
    repo.set_chat_enabled(&chat_id, chat_type(msg), Some(&chat_title(msg)), enabled)
        .await
}

fn next_offset(update: &Update) -> i32 {
    if update.id.0 >= i32::MAX as u32 {
        i32::MAX
    } else {
        update.id.0 as i32 + 1
    }
}

fn extract_message(update: Update) -> Option<Message> {
    match update.kind {
        UpdateKind::Message(message) | UpdateKind::ChannelPost(message) => Some(message),
        _ => None,
    }
}

fn chat_type(msg: &Message) -> &'static str {
    if msg.chat.is_private() {
        "private"
    } else if msg.chat.is_channel() {
        "channel"
    } else if msg.chat.is_supergroup() {
        "supergroup"
    } else if msg.chat.is_group() {
        "group"
    } else {
        "unknown"
    }
}

fn chat_title(msg: &Message) -> String {
    msg.chat
        .title()
        .map(ToString::to_string)
        .or_else(|| msg.chat.username().map(|username| format!("@{username}")))
        .or_else(|| msg.chat.first_name().map(ToString::to_string))
        .unwrap_or_else(|| format!("chat {}", msg.chat.id.0))
}

fn public_identifier(msg: &Message) -> String {
    msg.chat
        .username()
        .map(|username| format!("@{username}"))
        .unwrap_or_else(|| msg.chat.id.0.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn invalid_html_json_is_treated_as_temporary() {
        assert!(invalid_json_looks_temporary(
            "<html><title>502 Bad Gateway</title></html>"
        ));
    }

    #[test]
    fn invalid_token_is_a_fatal_polling_error() {
        let disposition = classify_polling_error(
            RequestError::Api(ApiError::InvalidToken),
            Duration::from_secs(5),
        );

        assert!(matches!(disposition, PollingErrorDisposition::Fatal { .. }));
    }

    #[test]
    fn conflict_get_updates_is_a_fatal_polling_error() {
        let disposition = classify_polling_error(
            RequestError::Api(ApiError::TerminatedByOtherGetUpdates),
            Duration::from_secs(5),
        );

        assert!(matches!(disposition, PollingErrorDisposition::Fatal { .. }));
    }
}

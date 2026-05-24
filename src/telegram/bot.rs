use std::{sync::Arc, time::Duration};

use chrono::{Days, Duration as ChronoDuration, LocalResult, TimeZone, Utc};
use chrono_tz::Europe::Berlin;
use teloxide::{
    payloads::GetUpdatesSetters,
    prelude::{Request, Requester},
    types::{ChatId, Message, Update, UpdateKind},
    Bot,
};
use tokio::time::sleep;
use tracing::{error, warn};

use crate::{
    config::Config,
    db::repository::Repository,
    deepseek::AiDescription,
    error::AppResult,
    scheduler::{CheckRunner, CheckSummary},
    steam::{
        FreePromotion, PromotionEvaluation, PromotionSkipReason, SteamClient, SteamGameData,
        SteamHttpDebugReport,
    },
    utils::html::truncate_chars,
};

use super::{
    commands::TelegramCommand,
    formatting::{build_post, format_price},
    publisher::TelegramPublisher,
};

pub async fn register_commands(bot: &Bot) -> AppResult<()> {
    bot.set_my_commands(TelegramCommand::public_menu_commands())
        .await?;
    Ok(())
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

    loop {
        let updates = match bot
            .get_updates()
            .offset(offset)
            .limit(50)
            .timeout(30)
            .send()
            .await
        {
            Ok(updates) => updates,
            Err(error) => {
                error!("Telegram get_updates failed: {error}");
                sleep(Duration::from_secs(5)).await;
                continue;
            }
        };

        for update in updates {
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
        format!("Кандидатов от Steam: {}", summary.candidate_app_ids),
        format!("Прошли prefilter: {}", summary.passed_prefilter),
        format!("Пропущено prefilter: {}", summary.skipped_by_prefilter),
        format!(
            "Нет данных цены в candidate: {}",
            summary.missing_candidate_price_data
        ),
        format!("Запросов appdetails: {}", summary.appdetails_requests),
        format!("Детали получены: {}", summary.app_details_fetched),
        format!("Ограничение Steam / 429: {}", summary.rate_limited),
        format!("Валидных акций: {}", summary.valid_free_promotions),
        format!(
            "Опубликовано сообщений: {}",
            summary.posts_successfully_sent
        ),
        format!("Ошибок: {}", summary.errors),
    ];

    if summary.app_details_unavailable > 0 {
        lines.push(format!(
            "Данные Steam недоступны: {}",
            summary.app_details_unavailable
        ));
    }

    if summary.duplicate_posts_skipped > 0 {
        lines.push(format!(
            "Дубликатов пропущено: {}",
            summary.duplicate_posts_skipped
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

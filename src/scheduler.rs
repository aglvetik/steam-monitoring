use std::{collections::HashMap, sync::Arc, time::Duration};

use chrono::{DateTime, Utc};
use tokio::{
    sync::Mutex,
    time::{self, MissedTickBehavior},
};
use tracing::{error, info, warn};

use crate::{
    db::{models::PriceEventRecord, repository::Repository},
    deepseek::{client::DeepSeekClient, AiDescription},
    error::{AppError, AppResult},
    steam::{FreePromotion, SteamCandidate, SteamClient},
    telegram::{formatting::build_post, publisher::TelegramPublisher},
    utils::time::parse_rfc3339_utc,
};

#[derive(Debug, Clone, Default)]
pub struct CheckSummary {
    pub reason: String,
    pub target_chats: usize,
    pub candidate_app_ids: usize,
    pub app_details_fetched: usize,
    pub valid_free_promotions: usize,
    pub posts_attempted: usize,
    pub posts_successfully_sent: usize,
    pub duplicate_posts_skipped: usize,
    pub deepseek_failures: usize,
    pub telegram_failures: usize,
    pub errors: usize,
    pub steam_request_failed: bool,
}

impl CheckSummary {
    pub fn likely_reason(&self) -> Option<&'static str> {
        if self.posts_successfully_sent > 0 {
            return None;
        }
        if self.steam_request_failed {
            return Some("Steam request failed");
        }
        if self.target_chats == 0 && self.valid_free_promotions > 0 {
            return Some("no enabled chats configured");
        }
        if self.valid_free_promotions == 0 {
            return Some("no valid 100% free paid-game promotions found");
        }
        if self.posts_attempted == 0 && self.duplicate_posts_skipped > 0 {
            return Some("all valid promotions were already published");
        }
        if self.posts_attempted > 0
            && self.posts_successfully_sent == 0
            && self.telegram_failures > 0
        {
            return Some("Telegram publish failed");
        }
        if self.posts_successfully_sent == 0 && self.deepseek_failures > 0 {
            return Some("DeepSeek failed");
        }

        None
    }
}

struct AiDescriptionOutcome {
    description: AiDescription,
    used_fallback: bool,
}

#[derive(Clone)]
pub struct CheckRunner {
    repo: Arc<Repository>,
    steam: Arc<SteamClient>,
    deepseek: Arc<DeepSeekClient>,
    publisher: Arc<TelegramPublisher>,
    main_channel_id: Option<String>,
    run_lock: Arc<Mutex<()>>,
}

impl CheckRunner {
    pub fn new(
        repo: Arc<Repository>,
        steam: Arc<SteamClient>,
        deepseek: Arc<DeepSeekClient>,
        publisher: Arc<TelegramPublisher>,
        main_channel_id: Option<String>,
    ) -> Self {
        Self {
            repo,
            steam,
            deepseek,
            publisher,
            main_channel_id,
            run_lock: Arc::new(Mutex::new(())),
        }
    }

    pub async fn run_guarded(
        self: Arc<Self>,
        reason: impl Into<String>,
    ) -> AppResult<CheckSummary> {
        let reason = reason.into();
        let runner = self.clone();
        let task_reason = reason.clone();
        let join = tokio::spawn(async move { runner.run_once(&task_reason).await });

        match join.await {
            Ok(result) => result,
            Err(error) if error.is_panic() => {
                Err(AppError::Other(format!("steam check panicked ({reason})")))
            }
            Err(error) => Err(AppError::Other(format!(
                "steam check task cancelled ({reason}): {error}"
            ))),
        }
    }

    async fn run_once(&self, reason: &str) -> AppResult<CheckSummary> {
        let _guard = self.run_lock.lock().await;

        info!("Starting Steam check: {reason}");

        let mut summary = CheckSummary {
            reason: reason.to_string(),
            ..CheckSummary::default()
        };

        let target_chat_ids = self
            .repo
            .resolve_publish_target_chat_ids(self.main_channel_id.as_deref())
            .await?;
        summary.target_chats = target_chat_ids.len();

        let active_events = self.repo.list_active_price_events().await?;
        let active_map = active_events
            .into_iter()
            .map(|event| (event.appid, event))
            .collect::<HashMap<_, _>>();

        info!(reason = reason, "Steam candidate fetch starting");
        let fetched_candidates = match self.steam.fetch_candidate_free_promotions().await {
            Ok(candidates) => candidates,
            Err(error) => {
                summary.errors += 1;
                summary.steam_request_failed = true;
                return Err(error);
            }
        };
        summary.candidate_app_ids = fetched_candidates.len();
        info!(
            reason = reason,
            candidate_count = summary.candidate_app_ids,
            "Steam candidate fetch finished"
        );

        let mut candidate_map = fetched_candidates
            .into_iter()
            .map(|candidate| (candidate.appid, candidate))
            .collect::<HashMap<_, _>>();

        for (appid, active_event) in &active_map {
            candidate_map
                .entry(*appid)
                .or_insert_with(|| SteamCandidate {
                    appid: *appid,
                    source: "active_recheck".to_string(),
                    free_until: active_event
                        .free_until
                        .as_deref()
                        .and_then(parse_rfc3339_utc),
                    ..SteamCandidate::default()
                });
        }

        let mut appids = candidate_map.keys().copied().collect::<Vec<_>>();
        appids.sort_unstable();

        for appid in appids {
            let candidate = candidate_map.get(&appid);
            let details = match self.steam.fetch_app_details(appid).await {
                Ok(Some(details)) => {
                    summary.app_details_fetched += 1;
                    details
                }
                Ok(None) => continue,
                Err(error) => {
                    summary.errors += 1;
                    warn!("Steam details fetch failed for app {appid}: {error}");
                    continue;
                }
            };

            let game = self.steam.build_game_data(&details, candidate);
            if let Err(error) = self.repo.upsert_game(&game).await {
                summary.errors += 1;
                warn!(
                    "Failed to upsert game {} ({}): {error}",
                    game.appid, game.name
                );
                continue;
            }

            if let Some(mut promotion) = self.steam.detect_free_promotion(&details, candidate) {
                summary.valid_free_promotions += 1;

                if promotion.free_until.is_none() {
                    promotion.free_until = active_map
                        .get(&promotion.appid)
                        .and_then(existing_free_until);
                }

                let price_event = match self
                    .repo
                    .create_or_reuse_active_price_event(&promotion)
                    .await
                {
                    Ok(event) => event,
                    Err(error) => {
                        summary.errors += 1;
                        warn!(
                            "Failed to create price event for app {}: {error}",
                            game.appid
                        );
                        continue;
                    }
                };

                if target_chat_ids.is_empty() {
                    continue;
                }

                let ai_outcome = match load_ai_description(
                    &self.repo,
                    &self.deepseek,
                    &game,
                    &promotion,
                )
                .await
                {
                    Ok(outcome) => outcome,
                    Err(error) => {
                        summary.errors += 1;
                        warn!(
                            "Failed to prepare AI description for app {}: {error}",
                            game.appid
                        );
                        AiDescriptionOutcome {
                            description: self.deepseek.fallback_description(&game),
                            used_fallback: true,
                        }
                    }
                };
                if ai_outcome.used_fallback {
                    summary.deepseek_failures += 1;
                }

                let post = build_post(&game, &promotion, &ai_outcome.description);

                for chat_id in &target_chat_ids {
                    match self
                        .repo
                        .has_published_post(game.appid, chat_id, price_event.id)
                        .await
                    {
                        Ok(true) => {
                            summary.duplicate_posts_skipped += 1;
                            continue;
                        }
                        Ok(false) => {}
                        Err(error) => {
                            summary.errors += 1;
                            warn!(
                                "Failed to check duplicate publication for app {} in chat {}: {error}",
                                game.appid, chat_id
                            );
                            continue;
                        }
                    }

                    summary.posts_attempted += 1;

                    match self.publisher.publish_to_chat(chat_id, &post).await {
                        Ok(message_id) => {
                            summary.posts_successfully_sent += 1;
                            if let Err(error) = self
                                .repo
                                .save_published_post(
                                    game.appid,
                                    chat_id,
                                    message_id,
                                    price_event.id,
                                )
                                .await
                            {
                                summary.errors += 1;
                                warn!(
                                    "Failed to save published post for app {} in chat {}: {error}",
                                    game.appid, chat_id
                                );
                            }
                        }
                        Err(error) => {
                            summary.errors += 1;
                            summary.telegram_failures += 1;
                            warn!(
                                "Failed to publish app {} to chat {}: {error}",
                                game.appid, chat_id
                            );
                        }
                    }
                }
            } else if active_map.contains_key(&appid) {
                if let Err(error) = self.repo.end_active_price_events_for_app(appid).await {
                    summary.errors += 1;
                    warn!("Failed to close active promotion for app {appid}: {error}");
                }
            }
        }

        info!(
            reason = %summary.reason,
            target_chats = summary.target_chats,
            candidate_app_ids = summary.candidate_app_ids,
            app_details_fetched = summary.app_details_fetched,
            valid_free_promotions = summary.valid_free_promotions,
            posts_attempted = summary.posts_attempted,
            posts_successfully_sent = summary.posts_successfully_sent,
            duplicate_posts_skipped = summary.duplicate_posts_skipped,
            errors = summary.errors,
            "Steam check finished"
        );

        Ok(summary)
    }
}

pub fn spawn_scheduler(
    runner: Arc<CheckRunner>,
    check_interval_minutes: u64,
    run_startup_check: bool,
) {
    std::thread::spawn(move || {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build();

        match runtime {
            Ok(runtime) => {
                runtime.block_on(run_loop(runner, check_interval_minutes, run_startup_check));
            }
            Err(error) => {
                error!("Failed to start scheduler runtime: {error}");
            }
        }
    });
}

async fn run_loop(runner: Arc<CheckRunner>, check_interval_minutes: u64, run_startup_check: bool) {
    if run_startup_check {
        if let Err(error) = runner.clone().run_guarded("startup").await {
            error!("Steam check failed (startup): {error}");
        }
    } else {
        info!("Skipping startup Steam check because RUN_STARTUP_CHECK=false");
    }

    let mut interval = time::interval(Duration::from_secs(check_interval_minutes.max(1) * 60));
    interval.set_missed_tick_behavior(MissedTickBehavior::Skip);
    interval.tick().await;

    loop {
        interval.tick().await;
        if let Err(error) = runner.clone().run_guarded("interval").await {
            error!("Steam check failed (interval): {error}");
        }
    }
}

async fn load_ai_description(
    repo: &Repository,
    deepseek: &DeepSeekClient,
    game: &crate::steam::SteamGameData,
    promotion: &FreePromotion,
) -> AppResult<AiDescriptionOutcome> {
    if let Some(cached) = repo.get_ai_description(game.appid, "russian").await? {
        return Ok(AiDescriptionOutcome {
            description: AiDescription {
                appid: cached.appid,
                language: cached.language,
                short_description: cached.short_description,
                why_play: cached.why_play,
                tags_line: cached.tags_line,
                model: cached.model,
            },
            used_fallback: false,
        });
    }

    match deepseek.generate_description(game, promotion).await {
        Ok(description) => {
            repo.upsert_ai_description(&description).await?;
            Ok(AiDescriptionOutcome {
                description,
                used_fallback: false,
            })
        }
        Err(error) => {
            warn!("DeepSeek failed for app {}: {error}", game.appid);
            Ok(AiDescriptionOutcome {
                description: deepseek.fallback_description(game),
                used_fallback: true,
            })
        }
    }
}

fn existing_free_until(event: &PriceEventRecord) -> Option<DateTime<Utc>> {
    event.free_until.as_deref().and_then(parse_rfc3339_utc)
}

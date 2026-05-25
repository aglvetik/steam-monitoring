mod config;
mod db;
mod deepseek;
mod error;
mod scheduler;
mod steam;
mod telegram;
mod utils;

use std::{path::Path, sync::Arc};

use config::Config;
use db::repository::Repository;
use error::{AppError, AppResult};
use reqwest_011::Client as TelegramHttpClient;
use scheduler::{spawn_scheduler, CheckRunner};
use sqlx::SqlitePool;
use teloxide::Bot;
use tracing::info;
use tracing_subscriber::EnvFilter;

#[tokio::main]
async fn main() -> AppResult<()> {
    let config = Arc::new(Config::from_env()?);
    init_tracing(&config.rust_log);

    tokio::fs::create_dir_all("data").await?;
    ensure_database_directory(&config.database_url).await?;

    let pool = SqlitePool::connect(&config.database_url).await?;
    sqlx::query("PRAGMA journal_mode = WAL;")
        .execute(&pool)
        .await?;
    sqlx::migrate!("./migrations").run(&pool).await?;

    let repo = Arc::new(Repository::new(pool));
    if let Some(main_channel_id) = config.telegram_main_channel_id.as_deref() {
        repo.upsert_chat(main_channel_id, "channel", Some("Main channel"), true)
            .await?;
    }

    let steam_client = Arc::new(steam::SteamClient::new(steam::SteamClientConfig {
        country: config.steam_country.clone(),
        language: config.steam_language.clone(),
        store_search_enabled: config.enable_steam_store_search_source,
        store_search_url: config.steam_store_search_url.clone(),
        store_search_count: config.steam_store_search_count,
        steamdb_enabled: config.enable_steamdb_source,
        steamdb_url: config.steamdb_free_promotions_url.clone(),
        steamdb_user_agent: config.steamdb_user_agent.clone(),
        steamdb_timeout_seconds: config.steamdb_timeout_seconds,
    })?);
    let deepseek_client = Arc::new(deepseek::client::DeepSeekClient::new(
        config.deepseek_api_key.clone(),
        config.deepseek_model.clone(),
    ));

    let telegram_http = TelegramHttpClient::builder()
        .user_agent("steam-free-games-bot/0.1")
        .no_proxy()
        .build()
        .map_err(|error| {
            AppError::Other(format!("failed to build Telegram HTTP client: {error}"))
        })?;
    let bot = Bot::with_client(config.telegram_bot_token.clone(), telegram_http);
    telegram::bot::register_commands(&bot).await?;

    let publisher = Arc::new(telegram::publisher::TelegramPublisher::new(bot.clone()));
    let check_runner = Arc::new(CheckRunner::new(
        repo.clone(),
        steam_client.clone(),
        deepseek_client,
        publisher,
        config.telegram_main_channel_id.clone(),
        config.enable_store_page_free_until_lookup,
        config.store_page_lookup_delay_ms,
    ));
    spawn_scheduler(
        check_runner.clone(),
        config.check_interval_minutes,
        config.run_startup_check,
    );

    info!("steam-free-games-bot started");
    telegram::bot::run(bot, repo, config, steam_client, check_runner).await
}

fn init_tracing(default_level: &str) {
    let filter = EnvFilter::try_from_default_env()
        .or_else(|_| EnvFilter::try_new(default_level))
        .unwrap_or_else(|_| EnvFilter::new("info"));

    tracing_subscriber::fmt().with_env_filter(filter).init();
}

async fn ensure_database_directory(database_url: &str) -> AppResult<()> {
    let path = database_url
        .strip_prefix("sqlite://")
        .or_else(|| database_url.strip_prefix("sqlite:"))
        .unwrap_or(database_url);

    if let Some(parent) = Path::new(path).parent() {
        if !parent.as_os_str().is_empty() {
            tokio::fs::create_dir_all(parent).await?;
        }
    }

    Ok(())
}

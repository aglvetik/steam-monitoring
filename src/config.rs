use std::{collections::HashSet, env};

use dotenvy::dotenv;

use crate::error::{AppError, AppResult};

#[derive(Debug, Clone)]
pub struct Config {
    pub telegram_bot_token: String,
    pub telegram_main_channel_id: Option<String>,
    pub admin_user_ids: HashSet<i64>,
    pub deepseek_api_key: Option<String>,
    pub deepseek_model: String,
    pub steam_country: String,
    pub steam_language: String,
    pub steam_appdetails_batch_size: usize,
    pub steam_appdetails_batch_delay_ms: u64,
    pub check_interval_minutes: u64,
    pub run_startup_check: bool,
    pub database_url: String,
    pub rust_log: String,
}

impl Config {
    pub fn from_env() -> AppResult<Self> {
        let _ = dotenv();

        let telegram_bot_token = required("TELEGRAM_BOT_TOKEN")?;
        let telegram_main_channel_id = optional("TELEGRAM_MAIN_CHANNEL_ID");
        let admin_user_ids = parse_admin_user_ids(optional("ADMIN_USER_IDS"))?;
        let deepseek_api_key = optional("DEEPSEEK_API_KEY");
        let deepseek_model =
            env::var("DEEPSEEK_MODEL").unwrap_or_else(|_| "deepseek-v4-flash".to_string());
        let steam_country = env::var("STEAM_COUNTRY").unwrap_or_else(|_| "DE".to_string());
        let steam_language = env::var("STEAM_LANGUAGE").unwrap_or_else(|_| "russian".to_string());
        let steam_appdetails_batch_size = env::var("STEAM_APPDETAILS_BATCH_SIZE")
            .ok()
            .and_then(|value| value.parse::<usize>().ok())
            .filter(|value| *value > 0)
            .unwrap_or(50);
        let steam_appdetails_batch_delay_ms = env::var("STEAM_APPDETAILS_BATCH_DELAY_MS")
            .ok()
            .and_then(|value| value.parse::<u64>().ok())
            .unwrap_or(1200);
        let check_interval_minutes = env::var("CHECK_INTERVAL_MINUTES")
            .ok()
            .and_then(|value| value.parse::<u64>().ok())
            .filter(|value| *value > 0)
            .unwrap_or(60);
        let run_startup_check = env::var("RUN_STARTUP_CHECK")
            .ok()
            .map(|value| {
                matches!(
                    value.trim().to_ascii_lowercase().as_str(),
                    "1" | "true" | "yes" | "on"
                )
            })
            .unwrap_or(false);
        let database_url =
            env::var("DATABASE_URL").unwrap_or_else(|_| "sqlite://data/bot.sqlite".to_string());
        let rust_log = env::var("RUST_LOG").unwrap_or_else(|_| "info".to_string());

        Ok(Self {
            telegram_bot_token,
            telegram_main_channel_id,
            admin_user_ids,
            deepseek_api_key,
            deepseek_model,
            steam_country,
            steam_language,
            steam_appdetails_batch_size,
            steam_appdetails_batch_delay_ms,
            check_interval_minutes,
            run_startup_check,
            database_url,
            rust_log,
        })
    }

    pub fn is_admin(&self, user_id: i64) -> bool {
        self.admin_user_ids.contains(&user_id)
    }
}

fn required(key: &str) -> AppResult<String> {
    env::var(key).map_err(|_| AppError::Config(format!("{key} is required")))
}

fn optional(key: &str) -> Option<String> {
    env::var(key)
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
}

fn parse_admin_user_ids(raw: Option<String>) -> AppResult<HashSet<i64>> {
    let mut user_ids = HashSet::new();

    if let Some(raw) = raw {
        for item in raw
            .split(',')
            .map(str::trim)
            .filter(|item| !item.is_empty())
        {
            let parsed = item
                .parse::<i64>()
                .map_err(|_| AppError::Config(format!("invalid ADMIN_USER_IDS value: {item}")))?;
            user_ids.insert(parsed);
        }
    }

    Ok(user_ids)
}

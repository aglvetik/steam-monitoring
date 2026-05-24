use thiserror::Error;

pub type AppResult<T> = Result<T, AppError>;

#[derive(Debug, Error)]
pub enum AppError {
    #[error("configuration error: {0}")]
    Config(String),
    #[error("request failed: {0}")]
    Request(#[from] reqwest::Error),
    #[error("database failed: {0}")]
    Database(#[from] sqlx::Error),
    #[error("database migration failed: {0}")]
    Migration(#[from] sqlx::migrate::MigrateError),
    #[error("telegram request failed: {0}")]
    Telegram(#[from] teloxide::RequestError),
    #[error("json failed: {0}")]
    Json(#[from] serde_json::Error),
    #[error("io failed: {0}")]
    Io(#[from] std::io::Error),
    #[error("url parse failed: {0}")]
    Url(#[from] url::ParseError),
    #[error("other error: {0}")]
    Other(String),
}

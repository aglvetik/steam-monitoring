#![allow(dead_code)]

use serde::{Deserialize, Serialize};
use sqlx::FromRow;

#[derive(Debug, Clone, FromRow)]
pub struct ChatRecord {
    pub chat_id: String,
    pub chat_type: String,
    pub title: Option<String>,
    pub enabled: bool,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Clone, FromRow)]
pub struct GameRecord {
    pub appid: i64,
    pub name: String,
    pub steam_url: String,
    pub kind: Option<String>,
    pub is_free_to_play: bool,
    pub header_image: Option<String>,
    pub capsule_image: Option<String>,
    pub short_description: Option<String>,
    pub genres_json: Option<String>,
    pub categories_json: Option<String>,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Clone, FromRow)]
pub struct PriceEventRecord {
    pub id: i64,
    pub appid: i64,
    pub currency: Option<String>,
    pub regular_price_cents: Option<i64>,
    pub final_price_cents: Option<i64>,
    pub discount_percent: Option<i64>,
    pub free_until: Option<String>,
    pub source: Option<String>,
    pub detected_at: String,
    pub ended_at: Option<String>,
}

#[derive(Debug, Clone, FromRow)]
pub struct PublishedPostRecord {
    pub id: i64,
    pub appid: i64,
    pub chat_id: String,
    pub message_id: Option<i64>,
    pub price_event_id: Option<i64>,
    pub published_at: String,
}

#[derive(Debug, Clone, FromRow, Serialize, Deserialize)]
pub struct AiDescriptionRecord {
    pub appid: i64,
    pub language: String,
    pub short_description: String,
    pub why_play: String,
    pub tags_line: Option<String>,
    pub model: Option<String>,
    pub created_at: String,
}

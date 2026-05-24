#![allow(dead_code)]

use chrono::{DateTime, Utc};
use serde::Deserialize;

#[derive(Debug, Clone, Default)]
pub struct SteamCandidate {
    pub appid: i64,
    pub source: String,
    pub free_until: Option<DateTime<Utc>>,
    pub currency: Option<String>,
    pub regular_price_cents: Option<i64>,
    pub final_price_cents: Option<i64>,
    pub discount_percent: Option<i64>,
    pub name: Option<String>,
    pub header_image: Option<String>,
    pub capsule_image: Option<String>,
}

impl SteamCandidate {
    pub fn merge_from(&mut self, other: SteamCandidate) {
        if self.free_until.is_none() {
            self.free_until = other.free_until;
        }
        if self.currency.is_none() {
            self.currency = other.currency;
        }
        if self.regular_price_cents.is_none() {
            self.regular_price_cents = other.regular_price_cents;
        }
        if self.final_price_cents.is_none() {
            self.final_price_cents = other.final_price_cents;
        }
        if self.discount_percent.is_none() {
            self.discount_percent = other.discount_percent;
        }
        if self.name.is_none() {
            self.name = other.name;
        }
        if self.header_image.is_none() {
            self.header_image = other.header_image;
        }
        if self.capsule_image.is_none() {
            self.capsule_image = other.capsule_image;
        }
    }
}

#[derive(Debug, Clone)]
pub struct SteamGameData {
    pub appid: i64,
    pub name: String,
    pub steam_url: String,
    pub kind: Option<String>,
    pub is_free_to_play: bool,
    pub header_image: Option<String>,
    pub capsule_image: Option<String>,
    pub short_description: Option<String>,
    pub genres: Vec<String>,
    pub categories: Vec<String>,
}

#[derive(Debug, Clone)]
pub struct FreePromotion {
    pub appid: i64,
    pub currency: Option<String>,
    pub regular_price_cents: Option<i64>,
    pub final_price_cents: Option<i64>,
    pub discount_percent: Option<i64>,
    pub free_until: Option<DateTime<Utc>>,
    pub source: String,
}

impl FreePromotion {
    pub fn free_until_rfc3339(&self) -> Option<String> {
        self.free_until.map(|value| value.to_rfc3339())
    }
}

#[derive(Debug, Clone, Deserialize)]
pub struct SteamAppDetailsEnvelope {
    pub success: bool,
    pub data: Option<SteamAppData>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct SteamAppData {
    #[serde(rename = "type")]
    pub kind: Option<String>,
    pub name: String,
    pub steam_appid: Option<i64>,
    pub is_free: Option<bool>,
    pub price_overview: Option<PriceOverview>,
    pub header_image: Option<String>,
    pub capsule_image: Option<String>,
    pub short_description: Option<String>,
    pub genres: Option<Vec<SteamDescriptor>>,
    pub categories: Option<Vec<SteamDescriptor>>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct PriceOverview {
    pub currency: String,
    pub initial: i64,
    pub r#final: i64,
    pub discount_percent: i64,
    pub initial_formatted: Option<String>,
    pub final_formatted: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct SteamDescriptor {
    pub description: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct SearchResultsResponse {
    pub success: i32,
    pub results_html: String,
    pub total_count: i64,
    pub start: i64,
}

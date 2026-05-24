use regex::Regex;
use serde_json::Value;

use crate::{error::AppResult, utils::time::unix_timestamp_to_utc};

use super::{client::SteamClient, SteamCandidate};

#[derive(Debug, Default, Clone, Copy)]
pub struct FeaturedCategoriesSource;

impl FeaturedCategoriesSource {
    pub fn name(&self) -> &'static str {
        "featured_categories"
    }

    pub fn url(&self, client: &SteamClient) -> String {
        format!(
            "https://store.steampowered.com/api/featuredcategories?cc={}&l={}",
            client.country(),
            client.language()
        )
    }

    pub async fn fetch(&self, client: &SteamClient) -> AppResult<Vec<SteamCandidate>> {
        let url = self.url(client);
        let payload = client.get_json::<Value>(&url).await?;
        Ok(self.extract_candidates(&payload))
    }

    pub fn extract_candidates(&self, payload: &Value) -> Vec<SteamCandidate> {
        let mut candidates = Vec::new();

        if let Some(root) = payload.as_object() {
            for value in root.values() {
                if let Some(items) = value.get("items").and_then(Value::as_array) {
                    for item in items {
                        if let Some(candidate) = candidate_from_featured_item(item, self.name()) {
                            candidates.push(candidate);
                        }
                    }
                }
            }
        }

        candidates
    }
}

fn candidate_from_featured_item(item: &Value, source: &str) -> Option<SteamCandidate> {
    let appid = item.get("id").and_then(Value::as_i64).or_else(|| {
        item.get("url")
            .and_then(Value::as_str)
            .and_then(parse_appid_from_url)
    })?;

    let item_type = item.get("type").and_then(Value::as_i64);
    if matches!(item_type, Some(value) if value != 0) {
        return None;
    }

    Some(SteamCandidate {
        appid,
        source: source.to_string(),
        free_until: item
            .get("discount_expiration")
            .and_then(Value::as_i64)
            .and_then(unix_timestamp_to_utc),
        currency: item
            .get("currency")
            .and_then(Value::as_str)
            .map(ToString::to_string),
        regular_price_cents: item.get("original_price").and_then(Value::as_i64),
        final_price_cents: item.get("final_price").and_then(Value::as_i64),
        discount_percent: item.get("discount_percent").and_then(Value::as_i64),
        name: item
            .get("name")
            .and_then(Value::as_str)
            .map(ToString::to_string),
        header_image: item
            .get("header_image")
            .and_then(Value::as_str)
            .map(ToString::to_string),
        capsule_image: item
            .get("large_capsule_image")
            .or_else(|| item.get("small_capsule_image"))
            .and_then(Value::as_str)
            .map(ToString::to_string),
    })
}

fn parse_appid_from_url(url: &str) -> Option<i64> {
    let regex = Regex::new(r"/app/(?P<id>\d+)").expect("static regex for Steam app URL is valid");
    regex
        .captures(url)
        .and_then(|captures| captures.name("id"))
        .and_then(|value| value.as_str().parse::<i64>().ok())
}

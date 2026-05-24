use regex::Regex;
use serde_json::Value;

use crate::{error::AppResult, utils::time::unix_timestamp_to_utc};

use super::{client::SteamClient, models::SearchResultsResponse, SteamCandidate};

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

#[derive(Debug, Clone)]
pub struct SearchSpecialsSource {
    max_pages: usize,
    page_size: usize,
}

impl SearchSpecialsSource {
    pub fn new(max_pages: usize, page_size: usize) -> Self {
        Self {
            max_pages,
            page_size,
        }
    }

    pub fn name(&self) -> &'static str {
        "search_specials"
    }

    pub async fn fetch(&self, client: &SteamClient) -> AppResult<Vec<SteamCandidate>> {
        let regex = Regex::new(r#"data-ds-appid="(?P<id>\d+)""#)
            .expect("static regex for Steam app ids is valid");
        let mut candidates = Vec::new();

        for page in 0..self.max_pages {
            let start = page * self.page_size;
            let url = format!(
                "https://store.steampowered.com/search/results/?specials=1&category1=998&hidef2p=1&ndl=1&infinite=1&start={start}&count={count}&cc={cc}&l={lang}",
                count = self.page_size,
                cc = client.country(),
                lang = client.language(),
            );
            let payload = client.get_json::<SearchResultsResponse>(&url).await?;
            if payload.success != 1 {
                break;
            }

            let mut page_hits = 0usize;
            for capture in regex.captures_iter(&payload.results_html) {
                if let Some(raw_id) = capture.name("id") {
                    if let Ok(appid) = raw_id.as_str().parse::<i64>() {
                        candidates.push(SteamCandidate {
                            appid,
                            source: self.name().to_string(),
                            ..SteamCandidate::default()
                        });
                        page_hits += 1;
                    }
                }
            }

            if page_hits == 0 || (payload.start + self.page_size as i64) >= payload.total_count {
                break;
            }
        }

        Ok(candidates)
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

use std::{collections::HashMap, time::Duration};

use reqwest::Client;
use serde::de::DeserializeOwned;
use serde_json::Value;
use tokio::time::sleep;
use tracing::{debug, error, warn};

use crate::error::{AppError, AppResult};

use super::{
    detector,
    sources::{FeaturedCategoriesSource, SearchSpecialsSource},
    PromotionEvaluation, SteamAppData, SteamCandidate, SteamGameData,
};

#[derive(Debug, Clone)]
pub struct SteamHttpDebugReport {
    pub url: String,
    pub http_success: bool,
    pub response_bytes: Option<usize>,
    pub json_parse_success: bool,
    pub candidate_app_ids: Option<usize>,
    pub stage: &'static str,
    pub error: Option<String>,
}

#[derive(Debug, Clone)]
pub enum AppDetailsResult {
    Available(Box<SteamAppData>),
    Unavailable,
    RateLimited,
}

pub struct SteamClient {
    country: String,
    language: String,
    http: Client,
    featured_source: FeaturedCategoriesSource,
    search_specials_source: SearchSpecialsSource,
}

impl SteamClient {
    pub fn new(country: String, language: String) -> AppResult<Self> {
        let http = Client::builder()
            .user_agent("steam-free-games-bot/0.1")
            .no_proxy()
            .build()?;

        Ok(Self {
            country,
            language,
            http,
            featured_source: FeaturedCategoriesSource,
            search_specials_source: SearchSpecialsSource::new(4, 50),
        })
    }

    pub fn country(&self) -> &str {
        &self.country
    }

    pub fn language(&self) -> &str {
        &self.language
    }

    pub async fn fetch_candidate_free_promotions(&self) -> AppResult<Vec<SteamCandidate>> {
        let mut deduped: HashMap<i64, SteamCandidate> = HashMap::new();
        let mut had_success = false;
        let mut source_errors = Vec::new();

        match self.featured_source.fetch(self).await {
            Ok(items) => {
                had_success = true;
                for item in items {
                    deduped
                        .entry(item.appid)
                        .and_modify(|existing| existing.merge_from(item.clone()))
                        .or_insert(item);
                }
            }
            Err(error) => {
                error!(
                    "Steam source {} failed: {error}",
                    self.featured_source.name()
                );
                source_errors.push(format!("{}: {error}", self.featured_source.name()));
            }
        }

        match self.search_specials_source.fetch(self).await {
            Ok(items) => {
                had_success = true;
                for item in items {
                    deduped
                        .entry(item.appid)
                        .and_modify(|existing| existing.merge_from(item.clone()))
                        .or_insert(item);
                }
            }
            Err(error) => {
                error!(
                    "Steam source {} failed: {error}",
                    self.search_specials_source.name()
                );
                source_errors.push(format!("{}: {error}", self.search_specials_source.name()));
            }
        }

        if !had_success && !source_errors.is_empty() {
            return Err(AppError::Other(format!(
                "all Steam candidate sources failed: {}",
                source_errors.join(" | ")
            )));
        }

        let mut values = deduped.into_values().collect::<Vec<_>>();
        values.sort_by_key(|item| item.appid);
        Ok(values)
    }

    pub async fn fetch_app_details(&self, appid: i64) -> AppResult<Option<SteamAppData>> {
        let appid_u32 = u32::try_from(appid)
            .map_err(|_| AppError::Other(format!("invalid Steam appid for lookup: {appid}")))?;
        let results = self.fetch_app_details_batch(&[appid_u32]).await?;

        match results.get(&appid_u32) {
            Some(AppDetailsResult::Available(details)) => Ok(Some((**details).clone())),
            Some(AppDetailsResult::Unavailable) | None => Ok(None),
            Some(AppDetailsResult::RateLimited) => Err(AppError::Other(
                "Steam temporarily rate limited appdetails (429)".to_string(),
            )),
        }
    }

    pub async fn fetch_app_details_batch(
        &self,
        appids: &[u32],
    ) -> AppResult<HashMap<u32, AppDetailsResult>> {
        if appids.is_empty() {
            return Ok(HashMap::new());
        }

        let joined = appids
            .iter()
            .map(u32::to_string)
            .collect::<Vec<_>>()
            .join(",");
        let url = format!(
            "https://store.steampowered.com/api/appdetails?appids={joined}&cc={}&l={}",
            self.country, self.language
        );
        let retry_delays = [Duration::from_secs(5), Duration::from_secs(15)];

        for attempt in 0..=retry_delays.len() {
            debug!(url = %url, batch_size = appids.len(), attempt, "Steam HTTP request starting");
            let response = self.http.get(&url).send().await?;

            if response.status() == reqwest::StatusCode::TOO_MANY_REQUESTS {
                if let Some(delay) = retry_delays.get(attempt) {
                    warn!(
                        batch_size = appids.len(),
                        retry_in_seconds = delay.as_secs(),
                        "Steam appdetails batch hit 429, retrying"
                    );
                    sleep(*delay).await;
                    continue;
                }

                warn!(
                    batch_size = appids.len(),
                    "Steam appdetails batch stayed rate limited after retries"
                );
                return Ok(appids
                    .iter()
                    .copied()
                    .map(|appid| (appid, AppDetailsResult::RateLimited))
                    .collect());
            }

            let response = response.error_for_status()?;
            let text = response.text().await?;
            debug!(
                url = %url,
                bytes = text.len(),
                "Steam HTTP response text received"
            );
            debug!(url = %url, "Steam JSON parse starting");
            let payload = serde_json::from_str::<Value>(&text)?;
            debug!(url = %url, "Steam JSON parse finished");

            return Ok(parse_app_details_batch_payload(appids, &payload));
        }

        Ok(appids
            .iter()
            .copied()
            .map(|appid| (appid, AppDetailsResult::RateLimited))
            .collect())
    }

    pub fn evaluate_free_promotion(
        &self,
        details: &SteamAppData,
        candidate: Option<&SteamCandidate>,
    ) -> PromotionEvaluation {
        detector::evaluate_free_promotion(details, candidate)
    }

    pub fn build_game_data(
        &self,
        details: &SteamAppData,
        candidate: Option<&SteamCandidate>,
    ) -> SteamGameData {
        detector::build_game_data(details, candidate)
    }

    pub async fn debug_featured_categories_http(&self) -> SteamHttpDebugReport {
        let url = self.featured_source.url(self);

        let text = match self.fetch_text(&url).await {
            Ok(text) => text,
            Err(error) => {
                return SteamHttpDebugReport {
                    url,
                    http_success: false,
                    response_bytes: None,
                    json_parse_success: false,
                    candidate_app_ids: None,
                    stage: "http",
                    error: Some(error.to_string()),
                };
            }
        };

        let bytes = text.len();
        debug!(url = %url, "Steam JSON parse starting");
        let payload = match serde_json::from_str::<Value>(&text) {
            Ok(payload) => {
                debug!(url = %url, "Steam JSON parse finished");
                payload
            }
            Err(error) => {
                return SteamHttpDebugReport {
                    url,
                    http_success: true,
                    response_bytes: Some(bytes),
                    json_parse_success: false,
                    candidate_app_ids: None,
                    stage: "json_parse",
                    error: Some(error.to_string()),
                };
            }
        };

        let candidate_count = self.featured_source.extract_candidates(&payload).len();

        SteamHttpDebugReport {
            url,
            http_success: true,
            response_bytes: Some(bytes),
            json_parse_success: true,
            candidate_app_ids: Some(candidate_count),
            stage: "done",
            error: None,
        }
    }

    pub(crate) async fn get_json<T>(&self, url: &str) -> AppResult<T>
    where
        T: DeserializeOwned,
    {
        let text = self.fetch_text(url).await?;
        debug!(url = %url, "Steam JSON parse starting");
        let parsed = serde_json::from_str::<T>(&text)?;
        debug!(url = %url, "Steam JSON parse finished");
        Ok(parsed)
    }

    async fn fetch_text(&self, url: &str) -> AppResult<String> {
        debug!(url = %url, "Steam HTTP request starting");
        let response = self.http.get(url).send().await?;
        let response = response.error_for_status()?;
        let text = response.text().await?;
        debug!(
            url = %url,
            bytes = text.len(),
            "Steam HTTP response text received"
        );
        Ok(text)
    }
}

fn parse_app_details_batch_payload(
    appids: &[u32],
    payload: &Value,
) -> HashMap<u32, AppDetailsResult> {
    let mut results = HashMap::with_capacity(appids.len());

    for appid in appids {
        let key = appid.to_string();
        let result = match payload.get(&key) {
            Some(envelope) => match envelope.get("success").and_then(Value::as_bool) {
                Some(true) => match envelope.get("data").cloned() {
                    Some(data) if !data.is_null() => {
                        match serde_json::from_value::<SteamAppData>(data) {
                            Ok(details) => AppDetailsResult::Available(Box::new(details)),
                            Err(_) => AppDetailsResult::Unavailable,
                        }
                    }
                    _ => AppDetailsResult::Unavailable,
                },
                _ => AppDetailsResult::Unavailable,
            },
            None => AppDetailsResult::Unavailable,
        };

        results.insert(*appid, result);
    }

    results
}

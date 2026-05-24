use std::collections::HashMap;

use reqwest::Client;
use serde::de::DeserializeOwned;
use serde_json::Value;
use tracing::{debug, error};

use crate::error::{AppError, AppResult};

use super::{
    detector,
    models::SteamAppDetailsEnvelope,
    sources::{FeaturedCategoriesSource, SearchSpecialsSource},
    FreePromotion, PromotionEvaluation, SteamAppData, SteamCandidate, SteamGameData,
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
        let url = format!(
            "https://store.steampowered.com/api/appdetails?appids={appid}&cc={}&l={}",
            self.country, self.language
        );
        let payload = self
            .get_json::<HashMap<String, SteamAppDetailsEnvelope>>(&url)
            .await?;

        Ok(payload
            .get(&appid.to_string())
            .filter(|envelope| envelope.success)
            .and_then(|envelope| envelope.data.clone()))
    }

    pub fn detect_free_promotion(
        &self,
        details: &SteamAppData,
        candidate: Option<&SteamCandidate>,
    ) -> Option<FreePromotion> {
        detector::detect_free_promotion(details, candidate)
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

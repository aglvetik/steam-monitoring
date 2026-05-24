use std::collections::HashMap;
use std::time::Duration;

use reqwest::Client;
use serde::de::DeserializeOwned;
use serde_json::Value;
use tracing::{debug, warn};

use crate::error::{AppError, AppResult};

use super::{
    detector,
    models::SteamAppDetailsEnvelope,
    sources::FeaturedCategoriesSource,
    steamdb::{SteamDbFreePromotionsReport, SteamDbFreePromotionsSource},
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
    steamdb_enabled: bool,
    steamdb_http: Client,
    steamdb_source: SteamDbFreePromotionsSource,
}

impl SteamClient {
    pub fn new(
        country: String,
        language: String,
        steamdb_enabled: bool,
        steamdb_url: String,
        steamdb_user_agent: String,
        steamdb_timeout_seconds: u64,
    ) -> AppResult<Self> {
        let http = Client::builder()
            .user_agent("steam-free-games-bot/0.1")
            .no_proxy()
            .build()?;
        let steamdb_http = Client::builder()
            .user_agent(steamdb_user_agent)
            .no_proxy()
            .timeout(Duration::from_secs(steamdb_timeout_seconds.max(1)))
            .build()?;

        Ok(Self {
            country,
            language,
            http,
            featured_source: FeaturedCategoriesSource,
            steamdb_enabled,
            steamdb_http,
            steamdb_source: SteamDbFreePromotionsSource::new(steamdb_url),
        })
    }

    pub fn country(&self) -> &str {
        &self.country
    }

    pub fn language(&self) -> &str {
        &self.language
    }

    pub async fn fetch_candidate_free_promotions(&self) -> AppResult<Vec<SteamCandidate>> {
        let mut values = self.featured_source.fetch(self).await?;
        values.sort_by_key(|item| item.appid);
        Ok(values)
    }

    pub async fn fetch_steamdb_free_promotions(&self) -> SteamDbFreePromotionsReport {
        if !self.steamdb_enabled {
            return SteamDbFreePromotionsReport {
                url: self.steamdb_source.url().to_string(),
                error: Some("SteamDB source is disabled by config.".to_string()),
                ..SteamDbFreePromotionsReport::default()
            };
        }

        self.steamdb_source.fetch(&self.steamdb_http).await
    }

    pub async fn fetch_app_details(&self, appid: i64) -> AppResult<Option<SteamAppData>> {
        let appid_u32 = u32::try_from(appid)
            .map_err(|_| AppError::Other(format!("invalid Steam appid for lookup: {appid}")))?;

        match self.fetch_single_app_details_result(appid_u32).await? {
            AppDetailsResult::Available(details) => Ok(Some((*details).clone())),
            AppDetailsResult::Unavailable => Ok(None),
            AppDetailsResult::RateLimited => Err(AppError::Other(
                "Steam temporarily rate limited appdetails (429)".to_string(),
            )),
        }
    }

    pub async fn fetch_single_app_details_result(&self, appid: u32) -> AppResult<AppDetailsResult> {
        let url = format!(
            "https://store.steampowered.com/api/appdetails?appids={appid}&cc={}&l={}",
            self.country, self.language
        );

        debug!(url = %url, appid, "Steam HTTP request starting");
        let response = self.http.get(&url).send().await?;
        let status = response.status();
        let text = response.text().await?;
        debug!(
            url = %url,
            bytes = text.len(),
            appid,
            "Steam HTTP response text received"
        );

        if status == reqwest::StatusCode::TOO_MANY_REQUESTS {
            warn!(appid, "Steam appdetails request hit 429");
            return Ok(AppDetailsResult::RateLimited);
        }

        if !status.is_success() {
            return Err(AppError::Other(format!(
                "Steam returned unexpected HTTP status {status} for appdetails {appid}"
            )));
        }

        debug!(url = %url, appid, "Steam JSON parse starting");
        let mut payload = serde_json::from_str::<HashMap<String, SteamAppDetailsEnvelope>>(&text)?;
        debug!(url = %url, appid, "Steam JSON parse finished");

        let result = match payload.remove(&appid.to_string()) {
            Some(envelope) if envelope.success => match envelope.data {
                Some(details) => AppDetailsResult::Available(Box::new(details)),
                None => AppDetailsResult::Unavailable,
            },
            Some(_) | None => AppDetailsResult::Unavailable,
        };

        Ok(result)
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

    pub async fn debug_steamdb_free_promotions(&self) -> SteamDbFreePromotionsReport {
        self.fetch_steamdb_free_promotions().await
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

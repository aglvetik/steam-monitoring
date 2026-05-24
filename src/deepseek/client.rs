use std::process::Command;

use serde::{Deserialize, Serialize};
use tracing::warn;

use crate::{
    error::{AppError, AppResult},
    steam::{FreePromotion, SteamGameData},
    utils::html::{strip_html_tags, truncate_chars},
};

use super::{prompts::build_prompt, AiDescription};

#[derive(Clone)]
pub struct DeepSeekClient {
    api_key: Option<String>,
    model: String,
}

impl DeepSeekClient {
    pub fn new(api_key: Option<String>, model: String) -> Self {
        Self { api_key, model }
    }

    pub async fn generate_description(
        &self,
        game: &SteamGameData,
        promotion: &FreePromotion,
    ) -> AppResult<AiDescription> {
        let api_key = self
            .api_key
            .as_ref()
            .ok_or_else(|| AppError::Config("DEEPSEEK_API_KEY is not configured".to_string()))?;
        let payload = DeepSeekRequest {
            model: self.model.clone(),
            stream: false,
            temperature: Some(0.2),
            messages: vec![
                ChatMessage {
                    role: "system".to_string(),
                    content: "Return valid JSON only. No markdown, no explanations.".to_string(),
                },
                ChatMessage {
                    role: "user".to_string(),
                    content: build_prompt(game, promotion),
                },
            ],
        };

        let response = self.send_with_retry(api_key, &payload).await?;
        let content = response
            .choices
            .first()
            .map(|choice| choice.message.content.trim().to_string())
            .filter(|content| !content.is_empty())
            .ok_or_else(|| AppError::Other("DeepSeek returned an empty message".to_string()))?;
        let parsed = parse_response_json(&content)?;

        Ok(AiDescription {
            appid: game.appid,
            language: "russian".to_string(),
            short_description: truncate_chars(parsed.short_description.trim(), 260),
            why_play: truncate_chars(parsed.why_play.trim(), 240),
            tags_line: parsed
                .tags_line
                .map(|value| truncate_chars(value.trim(), 140))
                .or_else(|| default_tags(game)),
            model: Some(self.model.clone()),
        })
    }

    pub fn fallback_description(&self, game: &SteamGameData) -> AiDescription {
        let fallback_short = game
            .short_description
            .as_deref()
            .map(strip_html_tags)
            .filter(|value| !value.is_empty())
            .unwrap_or_else(|| {
                "Steam did not provide much detail, but the game can be claimed for free during the current promotion."
                    .to_string()
            });
        let fallback_why = if !game.genres.is_empty() {
            format!(
                "May appeal to players who like: {}.",
                game.genres.join(", ")
            )
        } else {
            "May be interesting if you follow temporary free Steam promotions.".to_string()
        };

        AiDescription {
            appid: game.appid,
            language: "russian".to_string(),
            short_description: truncate_chars(&fallback_short, 260),
            why_play: truncate_chars(&fallback_why, 220),
            tags_line: default_tags(game),
            model: None,
        }
    }

    async fn send_with_retry(
        &self,
        api_key: &str,
        payload: &DeepSeekRequest,
    ) -> AppResult<DeepSeekResponse> {
        let api_key = api_key.to_string();
        let payload = payload.clone();

        send_with_retry_blocking(&api_key, &payload)
    }
}

fn default_tags(game: &SteamGameData) -> Option<String> {
    if !game.genres.is_empty() {
        Some(game.genres.join(" | "))
    } else if !game.categories.is_empty() {
        Some(game.categories.join(" | "))
    } else {
        None
    }
}

fn parse_response_json(content: &str) -> AppResult<AiResponsePayload> {
    let cleaned = content
        .trim()
        .trim_start_matches("```json")
        .trim_start_matches("```")
        .trim_end_matches("```")
        .trim();
    let start = cleaned
        .find('{')
        .ok_or_else(|| AppError::Other("DeepSeek response did not contain JSON".to_string()))?;
    let end = cleaned
        .rfind('}')
        .ok_or_else(|| AppError::Other("DeepSeek response did not contain JSON end".to_string()))?;
    let json = &cleaned[start..=end];
    let parsed: AiResponsePayload = serde_json::from_str(json)?;

    if parsed.short_description.trim().is_empty() || parsed.why_play.trim().is_empty() {
        return Err(AppError::Other(
            "DeepSeek response JSON was missing required fields".to_string(),
        ));
    }

    Ok(parsed)
}

fn send_with_retry_blocking(
    api_key: &str,
    payload: &DeepSeekRequest,
) -> AppResult<DeepSeekResponse> {
    let body = serde_json::to_string(payload)?;
    let headers = vec![
        format!("Authorization: Bearer {api_key}"),
        "Content-Type: application/json".to_string(),
    ];
    let bytes = curl_request(
        "https://api.deepseek.com/chat/completions",
        &headers,
        Some(body),
    )?;
    Ok(serde_json::from_slice::<DeepSeekResponse>(&bytes)?)
}

fn curl_request(url: &str, headers: &[String], body: Option<String>) -> AppResult<Vec<u8>> {
    let mut command = Command::new(curl_program());
    command
        .arg("--silent")
        .arg("--show-error")
        .arg("--fail")
        .arg("--location")
        .arg("--max-time")
        .arg("30")
        .arg("--user-agent")
        .arg("steam-free-games-bot/0.1");

    for header in headers {
        command.arg("-H").arg(header);
    }

    if let Some(body) = body {
        command.arg("--data").arg(body);
    }

    command.arg(url);
    let output = command.output()?;

    if output.status.success() {
        return Ok(output.stdout);
    }

    let stderr = String::from_utf8_lossy(&output.stderr);
    warn!("curl request failed for {url}: {stderr}");
    Err(AppError::Other(format!(
        "curl request failed for {url}: {}",
        stderr.trim()
    )))
}

fn curl_program() -> &'static str {
    if cfg!(windows) {
        r"C:\Windows\System32\curl.exe"
    } else {
        "curl"
    }
}

#[derive(Debug, Clone, Serialize)]
struct DeepSeekRequest {
    model: String,
    messages: Vec<ChatMessage>,
    stream: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    temperature: Option<f32>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct ChatMessage {
    role: String,
    content: String,
}

#[derive(Debug, Deserialize)]
struct DeepSeekResponse {
    choices: Vec<DeepSeekChoice>,
}

#[derive(Debug, Deserialize)]
struct DeepSeekChoice {
    message: ChatMessage,
}

#[derive(Debug, Deserialize)]
struct AiResponsePayload {
    short_description: String,
    why_play: String,
    tags_line: Option<String>,
}

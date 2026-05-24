pub mod client;
pub mod prompts;

#[derive(Debug, Clone)]
pub struct AiDescription {
    pub appid: i64,
    pub language: String,
    pub short_description: String,
    pub why_play: String,
    pub tags_line: Option<String>,
    pub model: Option<String>,
}

use std::str::FromStr;

use teloxide::{
    payloads::{SendMessageSetters, SendPhotoSetters},
    prelude::Requester,
    types::{ChatId, InputFile, Message, ParseMode},
    Bot,
};
use tracing::warn;
use url::Url;

use crate::error::AppResult;

use super::formatting::FormattedPost;

#[derive(Clone)]
pub struct TelegramPublisher {
    bot: Bot,
}

impl TelegramPublisher {
    pub fn new(bot: Bot) -> Self {
        Self { bot }
    }

    pub async fn publish_to_chat(
        &self,
        chat_id: &str,
        post: &FormattedPost,
    ) -> AppResult<Option<i64>> {
        if let (Some(image_url), Some(caption_html)) = (&post.image_url, &post.caption_html) {
            match self.send_photo(chat_id, image_url, caption_html).await {
                Ok(message) => return Ok(Some(message.id.0 as i64)),
                Err(error) => {
                    warn!("sendPhoto failed for chat {chat_id}: {error}");
                }
            }
        }

        let message = self.send_message(chat_id, &post.message_html).await?;
        Ok(Some(message.id.0 as i64))
    }

    async fn send_photo(
        &self,
        chat_id: &str,
        image_url: &str,
        caption_html: &str,
    ) -> AppResult<Message> {
        let url = Url::from_str(image_url)?;

        if let Ok(chat_numeric_id) = chat_id.parse::<i64>() {
            let message = self
                .bot
                .send_photo(ChatId(chat_numeric_id), InputFile::url(url))
                .caption(caption_html.to_string())
                .parse_mode(ParseMode::Html)
                .await?;
            Ok(message)
        } else {
            let message = self
                .bot
                .send_photo(chat_id.to_string(), InputFile::url(url))
                .caption(caption_html.to_string())
                .parse_mode(ParseMode::Html)
                .await?;
            Ok(message)
        }
    }

    async fn send_message(&self, chat_id: &str, message_html: &str) -> AppResult<Message> {
        if let Ok(chat_numeric_id) = chat_id.parse::<i64>() {
            let message = self
                .bot
                .send_message(ChatId(chat_numeric_id), message_html.to_string())
                .parse_mode(ParseMode::Html)
                .await?;
            Ok(message)
        } else {
            let message = self
                .bot
                .send_message(chat_id.to_string(), message_html.to_string())
                .parse_mode(ParseMode::Html)
                .await?;
            Ok(message)
        }
    }
}

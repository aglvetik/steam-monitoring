use std::collections::HashSet;

use sqlx::{query, query_as, query_scalar, SqlitePool};

use crate::{
    deepseek::AiDescription,
    error::AppResult,
    steam::{FreePromotion, SteamGameData},
    utils::time::now_rfc3339,
};

use super::models::{AiDescriptionRecord, ChatRecord, PriceEventRecord};

#[derive(Clone)]
pub struct Repository {
    pool: SqlitePool,
}

impl Repository {
    pub fn new(pool: SqlitePool) -> Self {
        Self { pool }
    }

    pub async fn upsert_chat(
        &self,
        chat_id: &str,
        chat_type: &str,
        title: Option<&str>,
        enabled: bool,
    ) -> AppResult<()> {
        let now = now_rfc3339();

        query(
            r#"
            INSERT INTO chats (chat_id, chat_type, title, enabled, created_at, updated_at)
            VALUES (?1, ?2, ?3, ?4, ?5, ?5)
            ON CONFLICT(chat_id) DO UPDATE SET
                chat_type = excluded.chat_type,
                title = COALESCE(excluded.title, chats.title),
                enabled = excluded.enabled,
                updated_at = excluded.updated_at
            "#,
        )
        .bind(chat_id)
        .bind(chat_type)
        .bind(title)
        .bind(enabled)
        .bind(&now)
        .execute(&self.pool)
        .await?;

        Ok(())
    }

    pub async fn set_chat_enabled(
        &self,
        chat_id: &str,
        chat_type: &str,
        title: Option<&str>,
        enabled: bool,
    ) -> AppResult<()> {
        self.upsert_chat(chat_id, chat_type, title, enabled).await
    }

    pub async fn get_chat(&self, chat_id: &str) -> AppResult<Option<ChatRecord>> {
        let chat = query_as::<_, ChatRecord>(
            r#"
            SELECT
                chat_id,
                chat_type,
                title,
                enabled,
                created_at,
                updated_at
            FROM chats
            WHERE chat_id = ?1
            "#,
        )
        .bind(chat_id)
        .fetch_optional(&self.pool)
        .await?;

        Ok(chat)
    }

    pub async fn list_enabled_chats(&self) -> AppResult<Vec<ChatRecord>> {
        let chats = query_as::<_, ChatRecord>(
            r#"
            SELECT
                chat_id,
                chat_type,
                title,
                enabled,
                created_at,
                updated_at
            FROM chats
            WHERE enabled = 1
            ORDER BY updated_at DESC
            "#,
        )
        .fetch_all(&self.pool)
        .await?;

        Ok(chats)
    }

    pub async fn resolve_publish_target_chat_ids(
        &self,
        main_channel_id: Option<&str>,
    ) -> AppResult<Vec<String>> {
        let enabled_chats = self.list_enabled_chats().await?;
        let mut seen = HashSet::new();
        let mut chat_ids = Vec::new();

        for chat in enabled_chats {
            if seen.insert(chat.chat_id.clone()) {
                chat_ids.push(chat.chat_id);
            }
        }

        if let Some(main_channel_id) = main_channel_id {
            let main_channel_id = main_channel_id.trim();
            if !main_channel_id.is_empty() && seen.insert(main_channel_id.to_string()) {
                chat_ids.push(main_channel_id.to_string());
            }
        }

        Ok(chat_ids)
    }

    pub async fn upsert_game(&self, game: &SteamGameData) -> AppResult<()> {
        let now = now_rfc3339();
        let genres_json = serde_json::to_string(&game.genres)?;
        let categories_json = serde_json::to_string(&game.categories)?;

        query(
            r#"
            INSERT INTO games (
                appid,
                name,
                steam_url,
                type,
                is_free_to_play,
                header_image,
                capsule_image,
                short_description,
                genres_json,
                categories_json,
                created_at,
                updated_at
            )
            VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?11)
            ON CONFLICT(appid) DO UPDATE SET
                name = excluded.name,
                steam_url = excluded.steam_url,
                type = excluded.type,
                is_free_to_play = excluded.is_free_to_play,
                header_image = excluded.header_image,
                capsule_image = excluded.capsule_image,
                short_description = excluded.short_description,
                genres_json = excluded.genres_json,
                categories_json = excluded.categories_json,
                updated_at = excluded.updated_at
            "#,
        )
        .bind(game.appid)
        .bind(&game.name)
        .bind(&game.steam_url)
        .bind(game.kind.as_deref())
        .bind(game.is_free_to_play)
        .bind(game.header_image.as_deref())
        .bind(game.capsule_image.as_deref())
        .bind(game.short_description.as_deref())
        .bind(genres_json)
        .bind(categories_json)
        .bind(&now)
        .execute(&self.pool)
        .await?;

        Ok(())
    }

    pub async fn list_active_price_events(&self) -> AppResult<Vec<PriceEventRecord>> {
        let items = query_as::<_, PriceEventRecord>(
            r#"
            SELECT
                id,
                appid,
                currency,
                regular_price_cents,
                final_price_cents,
                discount_percent,
                free_until,
                source,
                detected_at,
                ended_at
            FROM price_events
            WHERE ended_at IS NULL
            ORDER BY detected_at DESC
            "#,
        )
        .fetch_all(&self.pool)
        .await?;

        Ok(items)
    }

    pub async fn create_or_reuse_active_price_event(
        &self,
        promotion: &FreePromotion,
    ) -> AppResult<PriceEventRecord> {
        if let Some(free_until) = promotion.free_until_rfc3339() {
            if let Some(existing) = query_as::<_, PriceEventRecord>(
                r#"
                SELECT
                    id,
                    appid,
                    currency,
                    regular_price_cents,
                    final_price_cents,
                    discount_percent,
                    free_until,
                    source,
                    detected_at,
                    ended_at
                FROM price_events
                WHERE appid = ?1
                  AND ended_at IS NULL
                  AND COALESCE(free_until, '') = ?2
                ORDER BY detected_at DESC
                LIMIT 1
                "#,
            )
            .bind(promotion.appid)
            .bind(&free_until)
            .fetch_optional(&self.pool)
            .await?
            {
                return Ok(existing);
            }
        }

        if let Some(existing) = query_as::<_, PriceEventRecord>(
            r#"
            SELECT
                id,
                appid,
                currency,
                regular_price_cents,
                final_price_cents,
                discount_percent,
                free_until,
                source,
                detected_at,
                ended_at
            FROM price_events
            WHERE appid = ?1
              AND ended_at IS NULL
              AND COALESCE(currency, '') = COALESCE(?2, '')
              AND COALESCE(regular_price_cents, -1) = COALESCE(?3, -1)
              AND COALESCE(final_price_cents, -1) = COALESCE(?4, -1)
              AND COALESCE(discount_percent, -1) = COALESCE(?5, -1)
              AND COALESCE(free_until, '') = COALESCE(?6, '')
            ORDER BY detected_at DESC
            LIMIT 1
            "#,
        )
        .bind(promotion.appid)
        .bind(promotion.currency.as_deref())
        .bind(promotion.regular_price_cents)
        .bind(promotion.final_price_cents)
        .bind(promotion.discount_percent)
        .bind(promotion.free_until_rfc3339())
        .fetch_optional(&self.pool)
        .await?
        {
            return Ok(existing);
        }

        let now = now_rfc3339();
        query("UPDATE price_events SET ended_at = ?1 WHERE appid = ?2 AND ended_at IS NULL")
            .bind(&now)
            .bind(promotion.appid)
            .execute(&self.pool)
            .await?;

        let inserted = query_as::<_, PriceEventRecord>(
            r#"
            INSERT INTO price_events (
                appid,
                currency,
                regular_price_cents,
                final_price_cents,
                discount_percent,
                free_until,
                source,
                detected_at,
                ended_at
            )
            VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, NULL)
            RETURNING
                id,
                appid,
                currency,
                regular_price_cents,
                final_price_cents,
                discount_percent,
                free_until,
                source,
                detected_at,
                ended_at
            "#,
        )
        .bind(promotion.appid)
        .bind(promotion.currency.as_deref())
        .bind(promotion.regular_price_cents)
        .bind(promotion.final_price_cents)
        .bind(promotion.discount_percent)
        .bind(promotion.free_until_rfc3339())
        .bind(&promotion.source)
        .bind(&now)
        .fetch_one(&self.pool)
        .await?;

        Ok(inserted)
    }

    pub async fn end_active_price_events_for_app(&self, appid: i64) -> AppResult<u64> {
        let now = now_rfc3339();
        let result =
            query("UPDATE price_events SET ended_at = ?1 WHERE appid = ?2 AND ended_at IS NULL")
                .bind(&now)
                .bind(appid)
                .execute(&self.pool)
                .await?;

        Ok(result.rows_affected())
    }

    pub async fn has_published_post(
        &self,
        appid: i64,
        chat_id: &str,
        price_event_id: i64,
    ) -> AppResult<bool> {
        let row = query_scalar::<_, i64>(
            r#"
            SELECT 1
            FROM published_posts
            WHERE appid = ?1 AND chat_id = ?2 AND price_event_id = ?3
            LIMIT 1
            "#,
        )
        .bind(appid)
        .bind(chat_id)
        .bind(price_event_id)
        .fetch_optional(&self.pool)
        .await?;

        Ok(row.is_some())
    }

    pub async fn save_published_post(
        &self,
        appid: i64,
        chat_id: &str,
        message_id: Option<i64>,
        price_event_id: i64,
    ) -> AppResult<()> {
        let now = now_rfc3339();

        query(
            r#"
            INSERT OR IGNORE INTO published_posts (
                appid,
                chat_id,
                message_id,
                price_event_id,
                published_at
            )
            VALUES (?1, ?2, ?3, ?4, ?5)
            "#,
        )
        .bind(appid)
        .bind(chat_id)
        .bind(message_id)
        .bind(price_event_id)
        .bind(&now)
        .execute(&self.pool)
        .await?;

        Ok(())
    }

    pub async fn get_ai_description(
        &self,
        appid: i64,
        language: &str,
    ) -> AppResult<Option<AiDescriptionRecord>> {
        let item = query_as::<_, AiDescriptionRecord>(
            r#"
            SELECT
                appid,
                language,
                short_description,
                why_play,
                tags_line,
                model,
                created_at
            FROM ai_descriptions
            WHERE appid = ?1 AND language = ?2
            "#,
        )
        .bind(appid)
        .bind(language)
        .fetch_optional(&self.pool)
        .await?;

        Ok(item)
    }

    pub async fn upsert_ai_description(&self, description: &AiDescription) -> AppResult<()> {
        let now = now_rfc3339();

        query(
            r#"
            INSERT INTO ai_descriptions (
                appid,
                language,
                short_description,
                why_play,
                tags_line,
                model,
                created_at
            )
            VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)
            ON CONFLICT(appid) DO UPDATE SET
                language = excluded.language,
                short_description = excluded.short_description,
                why_play = excluded.why_play,
                tags_line = excluded.tags_line,
                model = excluded.model,
                created_at = excluded.created_at
            "#,
        )
        .bind(description.appid)
        .bind(&description.language)
        .bind(&description.short_description)
        .bind(&description.why_play)
        .bind(description.tags_line.as_deref())
        .bind(description.model.as_deref())
        .bind(&now)
        .execute(&self.pool)
        .await?;

        Ok(())
    }
}

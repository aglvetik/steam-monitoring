use crate::{
    deepseek::AiDescription,
    steam::{FreePromotion, SteamGameData},
    utils::{
        html::{escape_html, truncate_chars},
        time::format_berlin_datetime,
    },
};

const PHOTO_CAPTION_LIMIT: usize = 900;
const TELEGRAM_MESSAGE_LIMIT: usize = 3900;

#[derive(Debug, Clone)]
pub struct FormattedPost {
    pub image_url: Option<String>,
    pub caption_html: Option<String>,
    pub message_html: String,
}

pub fn build_post(
    game: &SteamGameData,
    promotion: &FreePromotion,
    ai: &AiDescription,
) -> FormattedPost {
    let image_url = game
        .header_image
        .clone()
        .or_else(|| game.capsule_image.clone());
    let message_html = fit_message(game, promotion, ai, TELEGRAM_MESSAGE_LIMIT);
    let caption_html = fit_caption(game, promotion, ai);

    FormattedPost {
        image_url,
        caption_html,
        message_html,
    }
}

pub(crate) fn format_price(cents: i64, currency: Option<&str>) -> String {
    let normalized_currency = currency.unwrap_or("EUR").to_ascii_uppercase();
    let label = match normalized_currency.as_str() {
        "EUR" => "€",
        "USD" => "$",
        "GBP" => "£",
        "RUB" => "₽",
        code => code,
    };

    if cents == 0 {
        return format!("0 {label}");
    }

    let whole = cents / 100;
    let remainder = (cents % 100).abs();
    format!("{whole},{remainder:02} {label}")
}

fn fit_caption(
    game: &SteamGameData,
    promotion: &FreePromotion,
    ai: &AiDescription,
) -> Option<String> {
    let variants = [
        (260usize, 220usize, 140usize),
        (190usize, 140usize, 100usize),
        (130usize, 95usize, 80usize),
    ];

    for (short_limit, why_limit, tags_limit) in variants {
        let candidate = compose_post(game, promotion, ai, short_limit, why_limit, tags_limit);
        if candidate.chars().count() <= PHOTO_CAPTION_LIMIT {
            return Some(candidate);
        }
    }

    None
}

fn fit_message(
    game: &SteamGameData,
    promotion: &FreePromotion,
    ai: &AiDescription,
    limit: usize,
) -> String {
    let variants = [
        (320usize, 260usize, 180usize),
        (240usize, 180usize, 140usize),
        (180usize, 140usize, 100usize),
    ];

    for (short_limit, why_limit, tags_limit) in variants {
        let candidate = compose_post(game, promotion, ai, short_limit, why_limit, tags_limit);
        if candidate.chars().count() <= limit {
            return candidate;
        }
    }

    compose_post(game, promotion, ai, 160, 120, 90)
}

fn compose_post(
    game: &SteamGameData,
    promotion: &FreePromotion,
    ai: &AiDescription,
    short_limit: usize,
    why_limit: usize,
    tags_limit: usize,
) -> String {
    let title = escape_html(&game.name);
    let regular_price = escape_html(&format_price(
        promotion.regular_price_cents.unwrap_or_default(),
        promotion.currency.as_deref(),
    ));
    let final_price = escape_html(&format_price(
        promotion.final_price_cents.unwrap_or_default(),
        promotion.currency.as_deref(),
    ));
    let free_until_line = format_free_until_line(promotion);
    let short_description = escape_html(&truncate_chars(ai.short_description.trim(), short_limit));
    let why_play = escape_html(&truncate_chars(ai.why_play.trim(), why_limit));
    let tags_line = escape_html(&truncate_chars(
        ai.tags_line
            .as_deref()
            .filter(|value| !value.trim().is_empty())
            .unwrap_or("Жанры и теги не указаны"),
        tags_limit,
    ));
    let steam_url = escape_html(&game.steam_url);

    format!(
        "🎮 <b>{title}</b>\n\n💸 <s>{regular_price}</s> → <b>{final_price}</b>\n⏳ {free_until_line}\n\n🧠 <b>Коротко:</b>\n{short_description}\n\n✨ <b>Почему может понравиться:</b>\n{why_play}\n\n🏷 {tags_line}\n\n🔗 <a href=\"{steam_url}\">Забрать в Steam</a>"
    )
}

fn format_free_until_line(promotion: &FreePromotion) -> String {
    match promotion.free_until {
        Some(value) => {
            let formatted = escape_html(&format_berlin_datetime(&value));
            format!("Бесплатно до: <b>{formatted}</b>")
        }
        None => "Бесплатно сейчас, дата окончания акции не указана Steam.".to_string(),
    }
}

#[cfg(test)]
mod tests {
    use chrono::Utc;

    use super::*;

    #[test]
    fn build_post_contains_expected_large_html() {
        let game = SteamGameData {
            appid: 1,
            name: "Game <Test>".to_string(),
            steam_url: "https://store.steampowered.com/".to_string(),
            kind: Some("game".to_string()),
            is_free_to_play: false,
            header_image: None,
            capsule_image: None,
            short_description: None,
            genres: Vec::new(),
            categories: Vec::new(),
        };
        let promotion = FreePromotion {
            appid: 1,
            currency: Some("EUR".to_string()),
            regular_price_cents: Some(1999),
            final_price_cents: Some(0),
            discount_percent: Some(100),
            free_until: Some(Utc::now()),
            source: "test".to_string(),
        };
        let ai = AiDescription {
            appid: 1,
            language: "russian".to_string(),
            short_description: "Описание".to_string(),
            why_play: "Почему стоит попробовать".to_string(),
            tags_line: Some("Тест / Steam / Бесплатно".to_string()),
            model: None,
        };

        let post = build_post(&game, &promotion, &ai);

        assert!(post.message_html.contains("<b>Game &lt;Test&gt;</b>"));
        assert!(post.message_html.contains("<s>19,99 €</s>"));
        assert!(post.message_html.contains("<b>0 €</b>"));
        assert!(post.message_html.contains("Бесплатно до: <b>"));
        assert!(post.message_html.contains("🧠 <b>Коротко:</b>"));
        assert!(post
            .message_html
            .contains("✨ <b>Почему может понравиться:</b>"));
        assert!(post
            .message_html
            .contains("<a href=\"https://store.steampowered.com/\">Забрать в Steam</a>"));
    }
}

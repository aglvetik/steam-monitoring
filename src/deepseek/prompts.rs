use crate::steam::{FreePromotion, SteamGameData};

pub fn build_prompt(game: &SteamGameData, promotion: &FreePromotion) -> String {
    let genres = if game.genres.is_empty() {
        "не указаны".to_string()
    } else {
        game.genres.join(", ")
    };
    let categories = if game.categories.is_empty() {
        "не указаны".to_string()
    } else {
        game.categories.join(", ")
    };
    let free_until = promotion
        .free_until
        .map(|value| value.to_rfc3339())
        .unwrap_or_else(|| "unknown".to_string());
    let short_description = game
        .short_description
        .clone()
        .unwrap_or_else(|| "Описание от Steam отсутствует.".to_string());
    let regular_price = promotion
        .regular_price_cents
        .map(|value| value.to_string())
        .unwrap_or_else(|| "unknown".to_string());
    let final_price = promotion
        .final_price_cents
        .map(|value| value.to_string())
        .unwrap_or_else(|| "unknown".to_string());
    let discount_percent = promotion
        .discount_percent
        .map(|value| value.to_string())
        .unwrap_or_else(|| "unknown".to_string());

    let source_instructions = if promotion.source == "steamdb_free_to_keep" {
        "- Источник акции: SteamDB Free to Keep. Можно написать, что игру можно забрать в библиотеку в течение акции.\n\
         - Не пиши, что игра бесплатна навсегда сама по себе. Корректно только: её можно успеть забрать во время акции.\n\
         - Если обычная цена неизвестна, не упоминай её и не выдумывай."
    } else {
        "- Источник акции: Steam Store. Описывай только подтверждённые данные из Steam."
    };

    format!(
        r#"Ты помогаешь вести Telegram-канал о временно бесплатных играх в Steam.
Верни строго JSON без markdown и без пояснений:
{{
  "short_description": "...",
  "why_play": "...",
  "tags_line": "..."
}}

Правила:
- Пиши только по-русски.
- Текст должен быть коротким, аккуратным и подходящим для Telegram.
- Не выдумывай дату окончания акции, дату релиза, оценки, онлайн, кооператив, режимы, награды или особенности, которых нет во входных данных.
- Если данных мало, честно опирайся только на доступное описание, жанры и категории.
- В tags_line дай короткую строку тегов или жанров без решёток.
{source_instructions}

Данные игры:
- Название: {name}
- AppID: {appid}
- Тип: {kind}
- Описание Steam: {short_description}
- Жанры: {genres}
- Категории: {categories}
- Обычная цена в центах: {regular_price}
- Текущая цена в центах: {final_price}
- Скидка в процентах: {discount_percent}
- Бесплатно до (RFC3339 или unknown): {free_until}
"#,
        source_instructions = source_instructions,
        name = game.name,
        appid = game.appid,
        kind = game.kind.clone().unwrap_or_else(|| "unknown".to_string()),
        short_description = short_description,
        genres = genres,
        categories = categories,
        regular_price = regular_price,
        final_price = final_price,
        discount_percent = discount_percent,
        free_until = free_until,
    )
}

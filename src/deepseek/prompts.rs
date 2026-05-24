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
- Если данных мало, честно опирайся только на доступное описание и жанры.
- В tags_line дай короткую строку тегов/жанров через точку или middot, без решетки.

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
        name = game.name,
        appid = game.appid,
        kind = game.kind.clone().unwrap_or_else(|| "unknown".to_string()),
        short_description = short_description,
        genres = genres,
        categories = categories,
        regular_price = promotion.regular_price_cents.unwrap_or_default(),
        final_price = promotion.final_price_cents.unwrap_or_default(),
        discount_percent = promotion.discount_percent.unwrap_or_default(),
        free_until = free_until,
    )
}

use std::sync::OnceLock;

use chrono::{
    DateTime, Datelike, Duration as ChronoDuration, LocalResult, NaiveDate, TimeZone, Utc,
};
use chrono_tz::Europe::Berlin;
use regex::Regex;
use scraper::{Html, Selector};

#[derive(Debug, Clone)]
pub struct StorePageFreeUntilExtraction {
    pub free_until: Option<DateTime<Utc>>,
    pub diagnostic: String,
    pub matched_text: Option<String>,
}

impl StorePageFreeUntilExtraction {
    fn found(
        free_until: DateTime<Utc>,
        diagnostic: impl Into<String>,
        matched_text: impl Into<String>,
    ) -> Self {
        Self {
            free_until: Some(free_until),
            diagnostic: diagnostic.into(),
            matched_text: Some(matched_text.into()),
        }
    }

    fn not_found(diagnostic: impl Into<String>, matched_text: Option<String>) -> Self {
        Self {
            free_until: None,
            diagnostic: diagnostic.into(),
            matched_text,
        }
    }
}

#[derive(Debug, Clone)]
pub struct StorePageFreeUntilReport {
    pub appid: u32,
    pub url: String,
    pub response_bytes: usize,
    pub free_until: Option<DateTime<Utc>>,
    pub diagnostic: String,
    pub matched_text: Option<String>,
}

pub fn extract_free_until_from_store_page_html(
    html: &str,
    now: DateTime<Utc>,
) -> StorePageFreeUntilExtraction {
    if let Some(result) = extract_from_discount_quantity(html, now) {
        return result;
    }

    if let Some(result) = extract_from_structured_timestamp(html) {
        return result;
    }

    if let Some(result) = extract_from_embedded_promotion_text(html, now) {
        return result;
    }

    if let Some(result) = extract_from_raw_html_promotion_fragments(html, now) {
        return result;
    }

    StorePageFreeUntilExtraction::not_found(
        "Steam store page did not expose a parsable promotion end date.",
        None,
    )
}

fn extract_from_discount_quantity(
    html: &str,
    now: DateTime<Utc>,
) -> Option<StorePageFreeUntilExtraction> {
    let document = Html::parse_document(html);
    let selector = quantity_selector();

    for element in document.select(selector) {
        let text = normalize_whitespace(&element.text().collect::<Vec<_>>().join(" "));
        if text.is_empty() {
            continue;
        }

        if let Some(parsed) = parse_free_until_text(&text, now) {
            return Some(StorePageFreeUntilExtraction::found(
                parsed,
                "Extracted from Steam discount countdown block.",
                text,
            ));
        }
    }

    None
}

fn extract_from_structured_timestamp(html: &str) -> Option<StorePageFreeUntilExtraction> {
    let regex = structured_timestamp_regex();
    let captures = regex.captures(html)?;
    let timestamp = captures.name("ts")?.as_str().parse::<i64>().ok()?;
    let free_until = crate::utils::time::unix_timestamp_to_utc(timestamp)?;
    let matched_text = captures
        .get(0)
        .map(|value| truncate_for_debug(value.as_str(), 220))
        .unwrap_or_default();

    Some(StorePageFreeUntilExtraction::found(
        free_until,
        "Extracted from structured timestamp on the Steam store page.",
        matched_text,
    ))
}

fn extract_from_embedded_promotion_text(
    html: &str,
    now: DateTime<Utc>,
) -> Option<StorePageFreeUntilExtraction> {
    for decoded in extract_candidate_embedded_texts(html) {
        let cleaned = clean_embedded_text(&decoded);
        if cleaned.is_empty() || !looks_like_promotion_context(&cleaned) {
            continue;
        }

        if let Some(parsed) = parse_free_until_text(&cleaned, now) {
            return Some(StorePageFreeUntilExtraction::found(
                parsed,
                "Extracted from Steam embedded promotion text.",
                truncate_for_debug(&cleaned, 260),
            ));
        }
    }

    None
}

fn extract_from_raw_html_promotion_fragments(
    html: &str,
    now: DateTime<Utc>,
) -> Option<StorePageFreeUntilExtraction> {
    for captures in raw_promotion_fragment_regex().captures_iter(html) {
        let Some(fragment) = captures.name("fragment").map(|value| value.as_str()) else {
            continue;
        };

        let cleaned = decode_raw_fragment(fragment);
        if cleaned.is_empty() {
            continue;
        }

        if let Some(parsed) = parse_free_until_text(&cleaned, now) {
            return Some(StorePageFreeUntilExtraction::found(
                parsed,
                "Extracted from raw Steam promotion fragment.",
                truncate_for_debug(&cleaned, 220),
            ));
        }
    }

    None
}

fn extract_candidate_embedded_texts(html: &str) -> Vec<String> {
    let mut values = Vec::new();

    for captures in jsondata_regex().captures_iter(html) {
        let Some(raw_value) = captures.name("value").map(|value| value.as_str()) else {
            continue;
        };

        if let Some(decoded_jsondata) = decode_json_string_fragment(raw_value) {
            if let Ok(json) = serde_json::from_str::<serde_json::Value>(&decoded_jsondata) {
                if let Some(items) = json
                    .get("localized_subtitle")
                    .and_then(|value| value.as_array())
                {
                    for item in items {
                        if let Some(text) = item.as_str() {
                            values.push(text.to_string());
                        }
                    }
                }
            }
        }
    }

    for captures in announcement_body_regex().captures_iter(html) {
        let Some(raw_value) = captures.name("value").map(|value| value.as_str()) else {
            continue;
        };

        if let Some(decoded) = decode_json_string_fragment(raw_value) {
            values.push(decoded);
        }
    }

    values
}

fn decode_json_string_fragment(raw: &str) -> Option<String> {
    serde_json::from_str::<String>(&format!("\"{raw}\"")).ok()
}

fn clean_embedded_text(raw: &str) -> String {
    let without_tags = bbcode_regex().replace_all(raw, " ");
    normalize_whitespace(without_tags.as_ref())
}

fn decode_raw_fragment(raw: &str) -> String {
    let value = raw
        .replace(r#"\u2013"#, "–")
        .replace(r#"\u2014"#, "—")
        .replace(r#"\u00a0"#, " ")
        .replace(r#"\/"#, "/")
        .replace(r#"\""#, "\"");

    normalize_whitespace(&value)
}

fn looks_like_promotion_context(text: &str) -> bool {
    let lower = text.to_lowercase();
    [
        "free-to-keep",
        "free forever",
        "for free",
        "free from",
        "offer ends",
        "special promotion",
        "бесплат",
        "акция",
        "предложение",
        "kostenlos",
        "angebot endet",
        "free to keep",
    ]
    .iter()
    .any(|needle| lower.contains(needle))
}

fn parse_free_until_text(text: &str, now: DateTime<Utc>) -> Option<DateTime<Utc>> {
    parse_until_phrase(text, now)
        .or_else(|| parse_free_range_phrase(text, now))
        .or_else(|| parse_numeric_datetime(text, now))
        .or_else(|| parse_day_month_phrase(text, now))
        .or_else(|| parse_month_day_phrase(text, now))
}

fn parse_until_phrase(text: &str, now: DateTime<Utc>) -> Option<DateTime<Utc>> {
    let lower = text.to_lowercase();
    let anchor = [
        "until",
        "offer ends",
        "ends",
        "до",
        "заканчивается",
        "предложение заканчивается",
        "angebot endet",
        "endet am",
    ]
    .iter()
    .filter_map(|needle| lower.find(needle).map(|index| index + needle.len()))
    .min()?;

    let tail = text.get(anchor..)?.trim();
    parse_numeric_datetime(tail, now)
        .or_else(|| parse_day_month_phrase(tail, now))
        .or_else(|| parse_month_day_phrase(tail, now))
}

fn parse_numeric_datetime(text: &str, now: DateTime<Utc>) -> Option<DateTime<Utc>> {
    let captures = numeric_datetime_regex().captures(text)?;
    let day = captures.name("day")?.as_str().parse::<u32>().ok()?;
    let month = captures.name("month")?.as_str().parse::<u32>().ok()?;
    let year = captures
        .name("year")
        .and_then(|value| parse_year(value.as_str(), now.year()));
    let hour = captures
        .name("hour")
        .and_then(|value| value.as_str().parse::<u32>().ok())
        .unwrap_or(23);
    let minute = captures
        .name("minute")
        .and_then(|value| value.as_str().parse::<u32>().ok())
        .unwrap_or(59);

    build_candidate_datetime(
        year.unwrap_or_else(|| guess_year(now, month, day)),
        month,
        day,
        hour,
        minute,
    )
}

fn parse_free_range_phrase(text: &str, now: DateTime<Utc>) -> Option<DateTime<Utc>> {
    let captures = free_range_regex().captures(text)?;
    let day = captures.name("day_end")?.as_str().parse::<u32>().ok()?;
    let month = parse_month_name(captures.name("month")?.as_str())?;
    let year = captures
        .name("year")
        .and_then(|value| parse_year(value.as_str(), now.year()))
        .unwrap_or_else(|| guess_year(now, month, day));

    build_candidate_datetime(year, month, day, 23, 59)
}

fn parse_day_month_phrase(text: &str, now: DateTime<Utc>) -> Option<DateTime<Utc>> {
    let captures = day_month_regex().captures(text)?;
    let day = captures
        .name("day_end")
        .or_else(|| captures.name("day"))?
        .as_str()
        .parse::<u32>()
        .ok()?;
    let month = parse_month_name(captures.name("month")?.as_str())?;
    let year = captures
        .name("year")
        .and_then(|value| parse_year(value.as_str(), now.year()))
        .unwrap_or_else(|| guess_year(now, month, day));
    let hour = captures
        .name("hour")
        .and_then(|value| value.as_str().parse::<u32>().ok())
        .unwrap_or(23);
    let minute = captures
        .name("minute")
        .and_then(|value| value.as_str().parse::<u32>().ok())
        .unwrap_or(59);

    build_candidate_datetime(year, month, day, hour, minute)
}

fn parse_month_day_phrase(text: &str, now: DateTime<Utc>) -> Option<DateTime<Utc>> {
    let captures = month_day_regex().captures(text)?;
    let day = captures
        .name("day_end")
        .or_else(|| captures.name("day"))?
        .as_str()
        .parse::<u32>()
        .ok()?;
    let month = parse_month_name(captures.name("month")?.as_str())?;
    let year = captures
        .name("year")
        .and_then(|value| parse_year(value.as_str(), now.year()))
        .unwrap_or_else(|| guess_year(now, month, day));
    let hour = captures
        .name("hour")
        .and_then(|value| value.as_str().parse::<u32>().ok())
        .unwrap_or(23);
    let minute = captures
        .name("minute")
        .and_then(|value| value.as_str().parse::<u32>().ok())
        .unwrap_or(59);

    build_candidate_datetime(year, month, day, hour, minute)
}

fn parse_year(raw: &str, current_year: i32) -> Option<i32> {
    let parsed = raw.parse::<i32>().ok()?;
    if (0..=99).contains(&parsed) {
        Some((current_year / 100) * 100 + parsed)
    } else {
        Some(parsed)
    }
}

fn guess_year(now: DateTime<Utc>, month: u32, day: u32) -> i32 {
    let berlin_now = now.with_timezone(&Berlin);
    let mut year = berlin_now.year();

    if let Some(candidate) = NaiveDate::from_ymd_opt(year, month, day) {
        if candidate < berlin_now.date_naive() - ChronoDuration::days(7) {
            year += 1;
        }
    }

    year
}

fn build_candidate_datetime(
    year: i32,
    month: u32,
    day: u32,
    hour: u32,
    minute: u32,
) -> Option<DateTime<Utc>> {
    let naive = NaiveDate::from_ymd_opt(year, month, day)?.and_hms_opt(hour, minute, 0)?;

    match Berlin.from_local_datetime(&naive) {
        LocalResult::Single(value) => Some(value.with_timezone(&Utc)),
        LocalResult::Ambiguous(value, _) => Some(value.with_timezone(&Utc)),
        LocalResult::None => None,
    }
}

fn parse_month_name(raw: &str) -> Option<u32> {
    let normalized = raw
        .trim_matches(|char: char| !char.is_alphanumeric())
        .to_lowercase()
        .replace('ё', "е");

    match normalized.as_str() {
        "january" | "jan" | "januar" | "января" | "январь" | "янв" => Some(1),
        "february" | "feb" | "februar" | "февраля" | "февраль" | "фев" => Some(2),
        "march" | "mar" | "marz" | "maerz" | "märz" | "марта" | "март" => Some(3),
        "april" | "apr" | "апреля" | "апрель" | "апр" => Some(4),
        "may" | "mai" | "мая" | "май" => Some(5),
        "june" | "jun" | "juni" | "июня" | "июнь" | "июн" => Some(6),
        "july" | "jul" | "juli" | "июля" | "июль" | "июл" => Some(7),
        "august" | "aug" | "августа" | "август" | "авг" => Some(8),
        "september" | "sept" | "sep" | "september." | "сентября" | "сентябрь" | "сен" | "сент" => {
            Some(9)
        }
        "october" | "oct" | "oktober" | "okt" | "октября" | "октябрь" | "окт" => {
            Some(10)
        }
        "november" | "nov" | "ноября" | "ноябрь" | "ноя" => Some(11),
        "december" | "dec" | "dezember" | "dez" | "декабря" | "декабрь" | "дек" => {
            Some(12)
        }
        _ => None,
    }
}

fn normalize_whitespace(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn truncate_for_debug(text: &str, max_chars: usize) -> String {
    let mut value = String::new();
    for (index, char) in text.chars().enumerate() {
        if index >= max_chars {
            value.push('…');
            break;
        }
        value.push(char);
    }
    value
}

fn quantity_selector() -> &'static Selector {
    static SELECTOR: OnceLock<Selector> = OnceLock::new();
    SELECTOR.get_or_init(|| {
        Selector::parse(".game_purchase_discount_quantity")
            .expect("static selector for Steam discount quantity must be valid")
    })
}

fn structured_timestamp_regex() -> &'static Regex {
    static REGEX: OnceLock<Regex> = OnceLock::new();
    REGEX.get_or_init(|| {
        Regex::new(
            r#"(?i)(?:discount_expiration|discount_end|sale_end|promotion_end|data-discount-end|data-sale-end|rtime32_discount_end)[^0-9]{0,20}(?P<ts>1\d{9})"#,
        )
        .expect("static structured timestamp regex must be valid")
    })
}

fn jsondata_regex() -> &'static Regex {
    static REGEX: OnceLock<Regex> = OnceLock::new();
    REGEX.get_or_init(|| {
        Regex::new(r#""jsondata":"(?P<value>(?:\\.|[^"\\])*)""#)
            .expect("static jsondata regex must be valid")
    })
}

fn announcement_body_regex() -> &'static Regex {
    static REGEX: OnceLock<Regex> = OnceLock::new();
    REGEX.get_or_init(|| {
        Regex::new(r#""body":"(?P<value>(?:\\.|[^"\\])*)""#)
            .expect("static announcement body regex must be valid")
    })
}

fn raw_promotion_fragment_regex() -> &'static Regex {
    static REGEX: OnceLock<Regex> = OnceLock::new();
    REGEX.get_or_init(|| {
        Regex::new(
            r#"(?is)(?P<fragment>FREE from.{0,160}?|free-to-keep.{0,220}?|Offer ends.{0,140}?|Предложение заканчивается.{0,140}?|Акция заканчивается.{0,140}?|Angebot endet.{0,140}?)"#,
        )
        .expect("static raw promotion fragment regex must be valid")
    })
}

fn bbcode_regex() -> &'static Regex {
    static REGEX: OnceLock<Regex> = OnceLock::new();
    REGEX.get_or_init(|| Regex::new(r#"\[[^\]]+\]"#).expect("static bbcode regex must be valid"))
}

fn numeric_datetime_regex() -> &'static Regex {
    static REGEX: OnceLock<Regex> = OnceLock::new();
    REGEX.get_or_init(|| {
        Regex::new(
            r#"(?xi)
            (?P<day>\d{1,2})
            [./]
            (?P<month>\d{1,2})
            (?:[./](?P<year>\d{2,4}))?
            (?:[^0-9]{0,12}(?P<hour>\d{1,2})[:.](?P<minute>\d{2}))?
        "#,
        )
        .expect("static numeric datetime regex must be valid")
    })
}

fn free_range_regex() -> &'static Regex {
    static REGEX: OnceLock<Regex> = OnceLock::new();
    REGEX.get_or_init(|| {
        Regex::new(
            r#"(?xi)
            free
            (?:
                \s+to\s+keep |
                \s+forever |
                \s+from
            )?
            .*?
            (?P<month>[\p{L}.]+)
            \s+
            (?P<day>\d{1,2})(?:st|nd|rd|th)?
            \s*[-–—]\s*
            (?P<day_end>\d{1,2})(?:st|nd|rd|th)?
            (?:,?\s*(?P<year>\d{4}))?
        "#,
        )
        .expect("static free-range regex must be valid")
    })
}

fn day_month_regex() -> &'static Regex {
    static REGEX: OnceLock<Regex> = OnceLock::new();
    REGEX.get_or_init(|| {
        Regex::new(
            r#"(?xi)
            (?P<day>\d{1,2})(?:st|nd|rd|th)?
            (?:\s*[-–—]\s*(?P<day_end>\d{1,2})(?:st|nd|rd|th)?)?
            \s+
            (?P<month>[\p{L}.]+)
            (?:\s+(?P<year>\d{4}))?
            (?:[^0-9]{0,20}(?P<hour>\d{1,2})[:.](?P<minute>\d{2}))?
        "#,
        )
        .expect("static day-month regex must be valid")
    })
}

fn month_day_regex() -> &'static Regex {
    static REGEX: OnceLock<Regex> = OnceLock::new();
    REGEX.get_or_init(|| {
        Regex::new(
            r#"(?xi)
            (?:[\p{L}.]+,\s+)?
            (?P<month>[\p{L}.]+)
            \s+
            (?P<day>\d{1,2})(?:st|nd|rd|th)?
            (?:\s*[-–—]\s*(?P<day_end>\d{1,2})(?:st|nd|rd|th)?)?
            (?:,?\s*(?P<year>\d{4}))?
            (?:[^0-9]{0,20}(?P<hour>\d{1,2})[:.](?P<minute>\d{2}))?
        "#,
        )
        .expect("static month-day regex must be valid")
    })
}

#[cfg(test)]
mod tests {
    use chrono::{Datelike, TimeZone, Timelike, Utc};
    use chrono_tz::Europe::Berlin;

    use super::*;

    #[test]
    fn extracts_free_until_from_discount_quantity_block() {
        let html = r#"
            <div class="game_area_purchase_game">
                <p class="game_purchase_discount_quantity">
                    Предложение заканчивается 27 мая в 10:00.
                </p>
            </div>
        "#;
        let now = Utc
            .with_ymd_and_hms(2026, 5, 25, 12, 0, 0)
            .single()
            .unwrap();

        let result = extract_free_until_from_store_page_html(html, now);

        assert!(result.free_until.is_some(), "{result:?}");
        let parsed = result.free_until.unwrap().with_timezone(&Berlin);
        assert_eq!(parsed.year(), 2026);
        assert_eq!(parsed.month(), 5);
        assert_eq!(parsed.day(), 27);
        assert_eq!(parsed.hour(), 10);
        assert_eq!(parsed.minute(), 0);
    }

    #[test]
    fn parses_free_until_from_free_range_text() {
        let now = Utc
            .with_ymd_and_hms(2026, 5, 25, 12, 0, 0)
            .single()
            .unwrap();

        let parsed =
            parse_free_until_text("Car Mechanic Simulator 2018 is FREE from May 21–27!", now)
                .unwrap()
                .with_timezone(&Berlin);
        assert_eq!(parsed.year(), 2026);
        assert_eq!(parsed.month(), 5);
        assert_eq!(parsed.day(), 27);
        assert_eq!(parsed.hour(), 23);
        assert_eq!(parsed.minute(), 59);
    }
}

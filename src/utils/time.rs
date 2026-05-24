use chrono::{DateTime, LocalResult, TimeZone, Utc};
use chrono_tz::Europe::Berlin;

pub fn unix_timestamp_to_utc(timestamp: i64) -> Option<DateTime<Utc>> {
    match Utc.timestamp_opt(timestamp, 0) {
        LocalResult::Single(value) => Some(value),
        _ => None,
    }
}

pub fn format_berlin_datetime(datetime: &DateTime<Utc>) -> String {
    datetime
        .with_timezone(&Berlin)
        .format("%d.%m.%Y %H:%M %Z")
        .to_string()
}

pub fn parse_rfc3339_utc(value: &str) -> Option<DateTime<Utc>> {
    chrono::DateTime::parse_from_rfc3339(value)
        .ok()
        .map(|datetime| datetime.with_timezone(&Utc))
}

pub fn now_rfc3339() -> String {
    Utc::now().to_rfc3339()
}

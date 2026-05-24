use chrono::{DateTime, NaiveDateTime, Utc};
use regex::Regex;
use reqwest::Client;
use scraper::{ElementRef, Html, Selector};
use tracing::{debug, warn};

use crate::steam::{looks_like_excluded_title, SteamCandidate};

pub const STEAMDB_FREE_TO_KEEP_SOURCE_NAME: &str = "steamdb_free_to_keep";

#[derive(Debug, Clone)]
pub struct SteamDbPromotionEntry {
    pub appid: i64,
    pub name: String,
    pub store_url: String,
    pub image_url: Option<String>,
    pub started_at: Option<DateTime<Utc>>,
    pub expires_at: Option<DateTime<Utc>>,
}

impl SteamDbPromotionEntry {
    pub fn to_candidate(&self) -> SteamCandidate {
        SteamCandidate {
            appid: self.appid,
            source: STEAMDB_FREE_TO_KEEP_SOURCE_NAME.to_string(),
            free_until: self.expires_at,
            currency: None,
            regular_price_cents: None,
            final_price_cents: Some(0),
            discount_percent: Some(100),
            name: Some(self.name.clone()),
            header_image: self.image_url.clone(),
            capsule_image: self.image_url.clone(),
        }
    }
}

#[derive(Debug, Clone, Default)]
pub struct SteamDbFreePromotionsReport {
    pub url: String,
    pub http_status: Option<u16>,
    pub response_bytes: Option<usize>,
    pub entries_parsed: usize,
    pub free_to_keep_accepted: usize,
    pub play_for_free_skipped: usize,
    pub missing_appid_skipped: usize,
    pub expired_skipped: usize,
    pub parse_errors: usize,
    pub obvious_non_game_skipped: usize,
    pub accepted_entries: Vec<SteamDbPromotionEntry>,
    pub error: Option<String>,
}

#[derive(Debug, Clone)]
pub struct SteamDbFreePromotionsSource {
    url: String,
}

impl SteamDbFreePromotionsSource {
    pub fn new(url: String) -> Self {
        Self { url }
    }

    pub fn url(&self) -> &str {
        &self.url
    }

    pub async fn fetch(&self, http: &Client) -> SteamDbFreePromotionsReport {
        let mut report = SteamDbFreePromotionsReport {
            url: self.url.clone(),
            ..SteamDbFreePromotionsReport::default()
        };

        debug!(url = %self.url, "SteamDB fetch started");
        let response = match http.get(self.url()).send().await {
            Ok(response) => response,
            Err(error) => {
                report.error = Some(format!("SteamDB request failed: {error}"));
                return report;
            }
        };

        let status = response.status();
        report.http_status = Some(status.as_u16());
        let html = match response.text().await {
            Ok(html) => html,
            Err(error) => {
                report.error = Some(format!("failed to read SteamDB response: {error}"));
                return report;
            }
        };

        report.response_bytes = Some(html.len());
        debug!(
            url = %self.url,
            bytes = html.len(),
            status = %status,
            "SteamDB fetch finished"
        );

        if !status.is_success() {
            report.error = Some(format!("SteamDB returned HTTP status {status}"));
            return report;
        }

        parse_steamdb_html(&html, &mut report);
        debug!(
            url = %self.url,
            entries_parsed = report.entries_parsed,
            free_to_keep_accepted = report.free_to_keep_accepted,
            play_for_free_skipped = report.play_for_free_skipped,
            missing_appid_skipped = report.missing_appid_skipped,
            expired_skipped = report.expired_skipped,
            parse_errors = report.parse_errors,
            "SteamDB parse finished"
        );
        report
    }
}

fn parse_steamdb_html(html: &str, report: &mut SteamDbFreePromotionsReport) {
    let challenge_text = html.to_ascii_lowercase();
    if challenge_text.contains("checking your browser")
        || challenge_text.contains("enable javascript and cookies to continue")
        || challenge_text.contains("stop. do not make any further requests to this domain")
    {
        report.parse_errors += 1;
        report.error = Some(
            "SteamDB returned a browser challenge page instead of promotions data.".to_string(),
        );
        return;
    }

    let document = Html::parse_document(html);
    let row_selector = selector("table tbody tr");
    let cell_selector = selector("td");
    let anchor_selector = selector("a");
    let image_selector = selector("img");

    let mut saw_candidate_rows = false;

    for row in document.select(&row_selector) {
        let cells = row.select(&cell_selector).collect::<Vec<_>>();
        if cells.is_empty() {
            continue;
        }

        let row_text = normalized_text(row.text().collect::<Vec<_>>().join(" "));
        if !row_text.contains("Free to Keep") && !row_text.contains("Play For Free") {
            continue;
        }

        saw_candidate_rows = true;
        report.entries_parsed += 1;

        let promotion_type = if row_text.contains("Free to Keep") {
            "Free to Keep"
        } else {
            "Play For Free"
        };

        if promotion_type != "Free to Keep" {
            report.play_for_free_skipped += 1;
            continue;
        }

        let store_url = extract_store_url(&cells, &row, &anchor_selector);
        let Some(store_url) = store_url else {
            report.missing_appid_skipped += 1;
            continue;
        };

        let Some(appid) = parse_appid_from_store_url(&store_url) else {
            report.missing_appid_skipped += 1;
            continue;
        };

        let title = extract_title(&cells, &row);
        let Some(name) = title.filter(|value| !value.is_empty()) else {
            report.parse_errors += 1;
            continue;
        };

        if looks_like_excluded_title(&name) {
            report.obvious_non_game_skipped += 1;
            continue;
        }

        let started_at = extract_started_at(&cells, &row_text);
        let expires_at = extract_expires_at(&cells, &row_text);

        if row_text.contains("Expires:") && expires_at.is_none() {
            report.parse_errors += 1;
            continue;
        }

        if let Some(expires_at) = expires_at {
            if expires_at <= Utc::now() {
                report.expired_skipped += 1;
                continue;
            }
        }

        let image_url = extract_image_url(&cells, &row, &image_selector);

        report.free_to_keep_accepted += 1;
        report.accepted_entries.push(SteamDbPromotionEntry {
            appid,
            name,
            store_url,
            image_url,
            started_at,
            expires_at,
        });
    }

    if !saw_candidate_rows {
        report.error = Some(
            "SteamDB parser did not find promotion rows with View Store / Free to Keep."
                .to_string(),
        );
    } else if report.free_to_keep_accepted == 0
        && report.error.is_none()
        && report.play_for_free_skipped == 0
    {
        warn!(
            entries_parsed = report.entries_parsed,
            "SteamDB rows were found, but no Free to Keep promotions were accepted"
        );
    }
}

fn selector(value: &str) -> Selector {
    Selector::parse(value).expect("static CSS selector must be valid")
}

fn extract_store_url(
    cells: &[ElementRef<'_>],
    row: &ElementRef<'_>,
    anchor_selector: &Selector,
) -> Option<String> {
    let preferred = cells
        .first()
        .and_then(|cell| find_store_url(*cell, anchor_selector))
        .or_else(|| find_store_url(*row, anchor_selector))?;

    Some(strip_url_query(&preferred))
}

fn find_store_url(scope: ElementRef<'_>, anchor_selector: &Selector) -> Option<String> {
    scope
        .select(anchor_selector)
        .filter_map(|anchor| anchor.value().attr("href"))
        .find(|href| href.contains("store.steampowered.com/app/"))
        .map(ToString::to_string)
}

fn extract_image_url(
    cells: &[ElementRef<'_>],
    row: &ElementRef<'_>,
    image_selector: &Selector,
) -> Option<String> {
    let image = cells
        .first()
        .and_then(|cell| find_image_url(*cell, image_selector))
        .or_else(|| find_image_url(*row, image_selector))?;

    Some(strip_url_query(&image))
}

fn find_image_url(scope: ElementRef<'_>, image_selector: &Selector) -> Option<String> {
    scope.select(image_selector).find_map(|image| {
        image
            .value()
            .attr("src")
            .or_else(|| image.value().attr("data-src"))
            .or_else(|| image.value().attr("data-original"))
            .map(ToString::to_string)
    })
}

fn extract_title(cells: &[ElementRef<'_>], row: &ElementRef<'_>) -> Option<String> {
    if let Some(title_cell) = cells.get(1).copied() {
        let text = extract_title_from_scope(title_cell);
        if !text.is_empty() {
            return Some(text);
        }
    }

    let text = extract_title_from_scope(*row);
    if text.is_empty() {
        None
    } else {
        Some(text)
    }
}

fn extract_title_from_scope(scope: ElementRef<'_>) -> String {
    let title_selector = selector("a b, a strong, h4 a, h4, strong, b");

    for node in scope.select(&title_selector) {
        let text = normalized_text(node.text().collect::<Vec<_>>().join(" "));
        if !text.is_empty()
            && text != "View Store"
            && text != "Install"
            && text != "Free to Keep"
            && text != "Play For Free"
            && !text.starts_with("Started:")
            && !text.starts_with("Expires:")
        {
            return text;
        }
    }

    let raw = normalized_text(scope.text().collect::<Vec<_>>().join(" "));
    extract_best_title_from_row_text(&raw)
}

fn extract_best_title_from_row_text(raw: &str) -> String {
    let without_labels = raw
        .replace("View Store", "")
        .replace("Install", "")
        .replace("Free to Keep", "")
        .replace("Play For Free", "");

    let started_index = without_labels
        .find("Started:")
        .unwrap_or(without_labels.len());
    let candidate = without_labels[..started_index].trim();
    candidate
        .lines()
        .map(str::trim)
        .find(|line| !line.is_empty())
        .unwrap_or(candidate)
        .to_string()
}

fn extract_started_at(cells: &[ElementRef<'_>], row_text: &str) -> Option<DateTime<Utc>> {
    cells
        .get(3)
        .map(|cell| normalized_text(cell.text().collect::<Vec<_>>().join(" ")))
        .and_then(|text| extract_utc_datetime(&text, "Started:"))
        .or_else(|| extract_utc_datetime(row_text, "Started:"))
}

fn extract_expires_at(cells: &[ElementRef<'_>], row_text: &str) -> Option<DateTime<Utc>> {
    cells
        .get(4)
        .map(|cell| normalized_text(cell.text().collect::<Vec<_>>().join(" ")))
        .and_then(|text| extract_utc_datetime(&text, "Expires:"))
        .or_else(|| extract_utc_datetime(row_text, "Expires:"))
}

fn extract_utc_datetime(text: &str, label: &str) -> Option<DateTime<Utc>> {
    let pattern = format!(
        r"{}\s*(\d{{1,2}}\s+[A-Za-z]{{3}}\s+\d{{4}}\s+[–-]\s+\d{{2}}:\d{{2}}:\d{{2}}\s+UTC)",
        regex::escape(label)
    );
    let regex = Regex::new(&pattern).ok()?;
    let captures = regex.captures(text)?;
    let value = captures.get(1)?.as_str().trim();
    let normalized = value
        .replace(" – ", " ")
        .replace(" - ", " ")
        .replace('–', " ")
        .replace(" UTC", "");

    parse_naive_utc(&normalized)
}

fn parse_naive_utc(value: &str) -> Option<DateTime<Utc>> {
    ["%d %b %Y %H:%M:%S", "%e %b %Y %H:%M:%S"]
        .iter()
        .find_map(|format| NaiveDateTime::parse_from_str(value, format).ok())
        .map(|naive| DateTime::from_naive_utc_and_offset(naive, Utc))
}

fn parse_appid_from_store_url(url: &str) -> Option<i64> {
    let regex = Regex::new(r"/app/(?P<id>\d+)").ok()?;
    regex
        .captures(url)
        .and_then(|captures| captures.name("id"))
        .and_then(|value| value.as_str().parse::<i64>().ok())
}

fn strip_url_query(url: &str) -> String {
    match url::Url::parse(url) {
        Ok(mut parsed) => {
            parsed.set_query(None);
            parsed.set_fragment(None);
            parsed.to_string()
        }
        Err(_) => url.to_string(),
    }
}

fn normalized_text(text: String) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_free_to_keep_rows() {
        let html = r#"
            <html>
              <body>
                <div class="footer-wrap">
                  <div class="body-content">
                    <div class="container">
                      <table>
                        <tbody>
                          <tr>
                            <td>
                              <a href="https://store.steampowered.com/app/12345/Test_Game/?snr=1_5_9__300">
                                <img src="https://cdn.example/test.jpg?size=small" />
                              </a>
                              <a href="steam://install/12345">Install</a>
                            </td>
                            <td><a href="https://store.steampowered.com/app/12345/Test_Game/"><b>Test Game</b></a></td>
                            <td><b>Free to Keep</b></td>
                            <td>Started: 24 May 2026 – 00:00:00 UTC</td>
                            <td>Expires: 26 May 2099 – 00:00:00 UTC</td>
                          </tr>
                          <tr>
                            <td><a href="https://store.steampowered.com/app/67890/Weekend/"><img src="https://cdn.example/weekend.jpg" /></a></td>
                            <td><a href="https://store.steampowered.com/app/67890/Weekend/"><b>Weekend Game</b></a></td>
                            <td><b>Play For Free</b></td>
                            <td>Started: 24 May 2026 – 00:00:00 UTC</td>
                            <td>Expires: 26 May 2099 – 00:00:00 UTC</td>
                          </tr>
                        </tbody>
                      </table>
                    </div>
                  </div>
                </div>
              </body>
            </html>
        "#;

        let mut report = SteamDbFreePromotionsReport::default();
        parse_steamdb_html(html, &mut report);

        assert_eq!(report.entries_parsed, 2);
        assert_eq!(report.free_to_keep_accepted, 1);
        assert_eq!(report.play_for_free_skipped, 1);
        assert_eq!(report.accepted_entries.len(), 1);
        assert_eq!(report.accepted_entries[0].appid, 12345);
        assert_eq!(report.accepted_entries[0].name, "Test Game");
        assert_eq!(
            report.accepted_entries[0].store_url,
            "https://store.steampowered.com/app/12345/Test_Game/"
        );
        assert_eq!(
            report.accepted_entries[0].image_url.as_deref(),
            Some("https://cdn.example/test.jpg")
        );
    }

    #[test]
    fn detects_challenge_page() {
        let html = r#"
            <html>
              <head><title>SteamDB</title></head>
              <body>
                <h1>Checking your browser…</h1>
                <p>Enable JavaScript and cookies to continue</p>
              </body>
            </html>
        "#;

        let mut report = SteamDbFreePromotionsReport::default();
        parse_steamdb_html(html, &mut report);

        assert_eq!(report.entries_parsed, 0);
        assert_eq!(report.parse_errors, 1);
        assert!(report.error.is_some());
    }
}

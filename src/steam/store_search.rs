use regex::Regex;
use reqwest::Client;
use scraper::{ElementRef, Html, Selector};
use tracing::debug;

use crate::steam::{looks_like_excluded_title, SearchResultsResponse, SteamCandidate};

pub const STEAM_STORE_SEARCH_SOURCE_NAME: &str = "steam_store_search_free_specials";

#[derive(Debug, Clone)]
pub struct SteamStoreSearchEntry {
    pub appid: i64,
    pub name: String,
    pub store_url: String,
    pub image_url: Option<String>,
    pub currency: Option<String>,
    pub regular_price_cents: i64,
    pub final_price_cents: i64,
    pub discount_percent: i64,
}

impl SteamStoreSearchEntry {
    pub fn to_candidate(&self) -> SteamCandidate {
        SteamCandidate {
            appid: self.appid,
            source: STEAM_STORE_SEARCH_SOURCE_NAME.to_string(),
            free_until: None,
            currency: self.currency.clone(),
            regular_price_cents: Some(self.regular_price_cents),
            final_price_cents: Some(self.final_price_cents),
            discount_percent: Some(self.discount_percent),
            name: Some(self.name.clone()),
            header_image: self.image_url.clone(),
            capsule_image: self.image_url.clone(),
        }
    }
}

#[derive(Debug, Clone, Default)]
pub struct SteamStoreSearchReport {
    pub url: String,
    pub http_status: Option<u16>,
    pub response_bytes: Option<usize>,
    pub parsed_entries: usize,
    pub accepted_candidates: usize,
    pub skipped_count: usize,
    pub missing_appid_skipped: usize,
    pub missing_price_data: usize,
    pub accepted_entries: Vec<SteamStoreSearchEntry>,
    pub error: Option<String>,
}

#[derive(Debug, Clone)]
pub struct SteamStoreSearchFreeSpecialsSource {
    base_url: String,
    count: u32,
}

impl SteamStoreSearchFreeSpecialsSource {
    pub fn new(base_url: String, count: u32) -> Self {
        Self {
            base_url,
            count: count.max(1),
        }
    }

    pub fn url(&self, country: &str, language: &str) -> String {
        let base_url = if self.base_url.ends_with('/') {
            self.base_url.clone()
        } else {
            format!("{}/", self.base_url)
        };

        format!(
            "{}?query&start=0&count={}&dynamic_data=&sort_by=_ASC&specials=1&maxprice=free&infinite=1&cc={}&l={}",
            base_url,
            self.count,
            country,
            language
        )
    }

    pub async fn fetch(
        &self,
        http: &Client,
        country: &str,
        language: &str,
    ) -> SteamStoreSearchReport {
        let url = self.url(country, language);
        let mut report = SteamStoreSearchReport {
            url: url.clone(),
            ..SteamStoreSearchReport::default()
        };

        debug!(url = %url, "Steam Store Search fetch started");
        let response = match http.get(&url).send().await {
            Ok(response) => response,
            Err(error) => {
                report.error = Some(format!("Steam Store Search request failed: {error}"));
                return report;
            }
        };

        let status = response.status();
        report.http_status = Some(status.as_u16());

        let text = match response.text().await {
            Ok(text) => text,
            Err(error) => {
                report.error = Some(format!(
                    "failed to read Steam Store Search response: {error}"
                ));
                return report;
            }
        };
        report.response_bytes = Some(text.len());

        debug!(
            url = %url,
            bytes = text.len(),
            status = %status,
            "Steam Store Search fetch finished"
        );

        if !status.is_success() {
            report.error = Some(format!("Steam Store Search returned HTTP status {status}"));
            return report;
        }

        let payload = match serde_json::from_str::<SearchResultsResponse>(&text) {
            Ok(payload) => payload,
            Err(error) => {
                report.error = Some(format!("Steam Store Search JSON parse failed: {error}"));
                return report;
            }
        };

        if payload.success != 1 {
            report.error = Some(format!(
                "Steam Store Search returned success={} instead of 1",
                payload.success
            ));
            return report;
        }

        parse_search_results_html(&payload.results_html, &mut report);
        debug!(
            url = %url,
            parsed_entries = report.parsed_entries,
            accepted_candidates = report.accepted_candidates,
            skipped_count = report.skipped_count,
            missing_appid_skipped = report.missing_appid_skipped,
            missing_price_data = report.missing_price_data,
            "Steam Store Search parse finished"
        );

        report
    }
}

fn parse_search_results_html(html: &str, report: &mut SteamStoreSearchReport) {
    let fragment = Html::parse_fragment(html);
    let row_selector = selector("a.search_result_row");
    let title_selector = selector(".title");
    let image_selector = selector("img");
    let price_container_selector = selector(".search_price_discount_combined, .discount_block");
    let discount_block_selector = selector(".discount_block");
    let discount_pct_selector = selector(".discount_pct");
    let original_price_selector = selector(".discount_original_price");
    let final_price_selector = selector(".discount_final_price, .search_price");

    for row in fragment.select(&row_selector) {
        report.parsed_entries += 1;

        let appid = extract_appid(&row);
        let Some(appid) = appid else {
            report.missing_appid_skipped += 1;
            report.skipped_count += 1;
            continue;
        };

        let title = row
            .select(&title_selector)
            .next()
            .map(|item| normalized_text(item.text().collect::<Vec<_>>().join(" ")))
            .filter(|value| !value.is_empty());
        let Some(name) = title else {
            report.skipped_count += 1;
            continue;
        };

        if looks_like_excluded_title(&name) {
            report.skipped_count += 1;
            continue;
        }

        let store_url = row
            .value()
            .attr("href")
            .map(strip_url_query)
            .unwrap_or_else(|| format!("https://store.steampowered.com/app/{appid}/"));
        let image_url = row
            .select(&image_selector)
            .next()
            .and_then(|image| image.value().attr("src"))
            .map(strip_url_query);

        let price_scope = row
            .select(&discount_block_selector)
            .next()
            .or_else(|| row.select(&price_container_selector).next());

        let Some(price_scope) = price_scope else {
            report.missing_price_data += 1;
            report.skipped_count += 1;
            continue;
        };

        let discount_percent = price_scope
            .value()
            .attr("data-discount")
            .and_then(|value| value.parse::<i64>().ok())
            .or_else(|| {
                price_scope
                    .select(&discount_pct_selector)
                    .next()
                    .map(|item| normalized_text(item.text().collect::<Vec<_>>().join(" ")))
                    .and_then(|value| parse_discount_percent(&value))
            });
        let original_text = price_scope
            .select(&original_price_selector)
            .next()
            .map(|item| normalized_text(item.text().collect::<Vec<_>>().join(" ")))
            .unwrap_or_default();
        let final_text = price_scope
            .select(&final_price_selector)
            .next()
            .map(|item| normalized_text(item.text().collect::<Vec<_>>().join(" ")))
            .unwrap_or_default();

        let regular_price_cents = parse_price_text_to_cents(&original_text);
        let final_price_cents = price_scope
            .value()
            .attr("data-price-final")
            .and_then(|value| value.parse::<i64>().ok())
            .or_else(|| parse_price_text_to_cents(&final_text));
        let final_is_zero = final_price_cents == Some(0) || is_free_price_text(&final_text);

        let Some(discount_percent) = discount_percent.map(i64::abs) else {
            report.missing_price_data += 1;
            report.skipped_count += 1;
            continue;
        };
        let Some(regular_price_cents) = regular_price_cents else {
            report.missing_price_data += 1;
            report.skipped_count += 1;
            continue;
        };
        if final_price_cents.is_none() && !final_is_zero {
            report.missing_price_data += 1;
            report.skipped_count += 1;
            continue;
        }

        if discount_percent != 100 || regular_price_cents <= 0 || !final_is_zero {
            report.skipped_count += 1;
            continue;
        }

        report.accepted_candidates += 1;
        report.accepted_entries.push(SteamStoreSearchEntry {
            appid,
            name,
            store_url,
            image_url,
            currency: detect_currency_from_price_text(&original_text)
                .or_else(|| detect_currency_from_price_text(&final_text)),
            regular_price_cents,
            final_price_cents: 0,
            discount_percent,
        });
    }
}

fn selector(value: &str) -> Selector {
    Selector::parse(value).expect("static CSS selector must be valid")
}

fn extract_appid(row: &ElementRef<'_>) -> Option<i64> {
    row.value()
        .attr("data-ds-appid")
        .and_then(parse_appid_like_value)
        .or_else(|| {
            row.value()
                .attr("href")
                .and_then(parse_appid_from_store_url)
        })
}

fn parse_appid_like_value(raw: &str) -> Option<i64> {
    raw.split(',').find_map(|part| {
        let digits = part
            .chars()
            .filter(|char| char.is_ascii_digit())
            .collect::<String>();
        if digits.is_empty() {
            None
        } else {
            digits.parse::<i64>().ok()
        }
    })
}

fn parse_discount_percent(text: &str) -> Option<i64> {
    let digits = text
        .chars()
        .filter(|char| char.is_ascii_digit() || *char == '-')
        .collect::<String>();
    digits.parse::<i64>().ok()
}

fn parse_price_text_to_cents(text: &str) -> Option<i64> {
    if is_free_price_text(text) {
        return Some(0);
    }

    let filtered = text
        .chars()
        .filter(|char| char.is_ascii_digit() || matches!(char, ',' | '.'))
        .collect::<String>();

    if filtered.is_empty() {
        return None;
    }

    if let Some(position) = filtered.rfind([',', '.']) {
        let whole = filtered[..position]
            .chars()
            .filter(|char| char.is_ascii_digit())
            .collect::<String>();
        let fractional = filtered[position + 1..]
            .chars()
            .filter(|char| char.is_ascii_digit())
            .collect::<String>();

        if whole.is_empty() && fractional.is_empty() {
            return None;
        }

        let whole = if whole.is_empty() {
            0
        } else {
            whole.parse::<i64>().ok()?
        };
        let fractional = match fractional.len() {
            0 => 0,
            1 => fractional.parse::<i64>().ok()? * 10,
            _ => fractional[..2].parse::<i64>().ok()?,
        };

        return Some(whole * 100 + fractional);
    }

    filtered.parse::<i64>().ok().map(|whole| whole * 100)
}

fn is_free_price_text(text: &str) -> bool {
    let normalized = text.trim().to_lowercase();
    normalized == "free"
        || normalized == "бесплатно"
        || normalized == "kostenlos"
        || normalized == "gratuit"
        || normalized == "gratuito"
        || normalized == "0,00€"
        || normalized == "0.00€"
}

fn detect_currency_from_price_text(text: &str) -> Option<String> {
    if text.contains('€') {
        Some("EUR".to_string())
    } else if text.contains('$') {
        Some("USD".to_string())
    } else if text.contains('£') {
        Some("GBP".to_string())
    } else if text.contains('₽') || text.to_lowercase().contains("руб") {
        Some("RUB".to_string())
    } else if text.contains('¥') {
        Some("JPY".to_string())
    } else if text.contains("zł") {
        Some("PLN".to_string())
    } else {
        None
    }
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
    fn parses_search_results_html_and_skips_soundtrack() {
        let html = r#"
            <a href="https://store.steampowered.com/app/489630/Warhammer_40000_Gladius__Relics_of_War/?snr=1_7_7_2300_150_1"
               data-ds-appid="489630"
               class="search_result_row">
              <div class="search_capsule"><img src="https://cdn.example/gladius.jpg?size=small"></div>
              <span class="title">Warhammer 40,000: Gladius - Relics of War</span>
              <div class="search_price_discount_combined" data-price-final="0">
                <div class="discount_block search_discount_block" data-discount="100" data-price-final="0">
                  <div class="discount_pct">-100%</div>
                  <div class="discount_prices">
                    <div class="discount_original_price">40,86€</div>
                    <div class="discount_final_price">0,00€</div>
                  </div>
                </div>
              </div>
            </a>
            <a href="https://store.steampowered.com/app/3502050/AquaDream_Soundtrack/?snr=1_7_7_2300_150_1"
               data-ds-appid="3502050"
               class="search_result_row">
              <span class="title">AquaDream Soundtrack</span>
              <div class="search_price_discount_combined" data-price-final="0">
                <div class="discount_block search_discount_block" data-discount="100" data-price-final="0">
                  <div class="discount_pct">-100%</div>
                  <div class="discount_prices">
                    <div class="discount_original_price">0,99€</div>
                    <div class="discount_final_price">0,00€</div>
                  </div>
                </div>
              </div>
            </a>
        "#;

        let mut report = SteamStoreSearchReport::default();
        parse_search_results_html(html, &mut report);

        assert_eq!(report.parsed_entries, 2);
        assert_eq!(report.accepted_candidates, 1);
        assert_eq!(report.skipped_count, 1);
        assert_eq!(report.accepted_entries.len(), 1);
        assert_eq!(report.accepted_entries[0].appid, 489630);
        assert_eq!(
            report.accepted_entries[0].store_url,
            "https://store.steampowered.com/app/489630/Warhammer_40000_Gladius__Relics_of_War/"
        );
        assert_eq!(report.accepted_entries[0].regular_price_cents, 4086);
        assert_eq!(report.accepted_entries[0].final_price_cents, 0);
        assert_eq!(report.accepted_entries[0].discount_percent, 100);
    }

    #[test]
    fn parses_euro_price_to_cents() {
        assert_eq!(parse_price_text_to_cents("40,86€"), Some(4086));
        assert_eq!(parse_price_text_to_cents("1.234,56€"), Some(123456));
        assert_eq!(parse_price_text_to_cents("Free"), Some(0));
    }
}

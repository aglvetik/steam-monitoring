use crate::utils::html::strip_html_tags;

use super::models::{FreePromotion, SteamAppData, SteamCandidate, SteamGameData};

const SKIP_TYPES: &[&str] = &[
    "demo",
    "dlc",
    "episode",
    "hardware",
    "movie",
    "music",
    "series",
    "software",
    "soundtrack",
    "tool",
    "video",
];

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PromotionSkipReason {
    MissingPriceData,
    AppIsFreeToPlay,
    AppTypeIsNotGame,
    DiscountIsNot100,
    FinalPriceIsNot0,
    InitialPriceIsNotGreaterThan0,
}

impl PromotionSkipReason {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::MissingPriceData => "missing price data",
            Self::AppIsFreeToPlay => "app is free-to-play",
            Self::AppTypeIsNotGame => "app type is not game",
            Self::DiscountIsNot100 => "discount is not 100%",
            Self::FinalPriceIsNot0 => "final price is not 0",
            Self::InitialPriceIsNotGreaterThan0 => "initial price is not greater than 0",
        }
    }
}

#[derive(Debug, Clone)]
pub enum PromotionEvaluation {
    Publishable(FreePromotion),
    Skipped(PromotionSkipReason),
}

pub fn evaluate_free_promotion(
    details: &SteamAppData,
    candidate: Option<&SteamCandidate>,
) -> PromotionEvaluation {
    if !is_game_type(details.kind.as_deref()) {
        return PromotionEvaluation::Skipped(PromotionSkipReason::AppTypeIsNotGame);
    }

    let Some(price) = details.price_overview.as_ref() else {
        return PromotionEvaluation::Skipped(PromotionSkipReason::MissingPriceData);
    };

    if price.initial <= 0 {
        if details.is_free.unwrap_or(false) {
            return PromotionEvaluation::Skipped(PromotionSkipReason::AppIsFreeToPlay);
        }
        return PromotionEvaluation::Skipped(PromotionSkipReason::InitialPriceIsNotGreaterThan0);
    }

    if price.r#final != 0 {
        return PromotionEvaluation::Skipped(PromotionSkipReason::FinalPriceIsNot0);
    }

    if price.discount_percent != 100 {
        return PromotionEvaluation::Skipped(PromotionSkipReason::DiscountIsNot100);
    }

    PromotionEvaluation::Publishable(FreePromotion {
        appid: details
            .steam_appid
            .unwrap_or_else(|| candidate.map(|item| item.appid).unwrap_or_default()),
        currency: Some(price.currency.clone()),
        regular_price_cents: Some(price.initial),
        final_price_cents: Some(price.r#final),
        discount_percent: Some(price.discount_percent),
        free_until: candidate.and_then(|item| item.free_until),
        source: candidate
            .map(|item| item.source.clone())
            .unwrap_or_else(|| "steam_appdetails".to_string()),
    })
}

pub fn detect_free_promotion(
    details: &SteamAppData,
    candidate: Option<&SteamCandidate>,
) -> Option<FreePromotion> {
    match evaluate_free_promotion(details, candidate) {
        PromotionEvaluation::Publishable(promotion) => Some(promotion),
        PromotionEvaluation::Skipped(_) => None,
    }
}

pub fn build_game_data(
    details: &SteamAppData,
    candidate: Option<&SteamCandidate>,
) -> SteamGameData {
    let appid = details
        .steam_appid
        .unwrap_or_else(|| candidate.map(|item| item.appid).unwrap_or_default());
    let genres = details
        .genres
        .as_ref()
        .map(|items| {
            items
                .iter()
                .map(|item| strip_html_tags(&item.description))
                .filter(|value| !value.is_empty())
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    let categories = details
        .categories
        .as_ref()
        .map(|items| {
            items
                .iter()
                .map(|item| strip_html_tags(&item.description))
                .filter(|value| !value.is_empty())
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    let price_initial = details
        .price_overview
        .as_ref()
        .map(|price| price.initial)
        .unwrap_or_default();
    let is_free_to_play = details.is_free.unwrap_or(false) && price_initial == 0;

    SteamGameData {
        appid,
        name: strip_html_tags(&details.name),
        steam_url: format!("https://store.steampowered.com/app/{appid}/"),
        kind: details.kind.clone(),
        is_free_to_play,
        header_image: details
            .header_image
            .clone()
            .or_else(|| candidate.and_then(|item| item.header_image.clone())),
        capsule_image: details
            .capsule_image
            .clone()
            .or_else(|| candidate.and_then(|item| item.capsule_image.clone())),
        short_description: details
            .short_description
            .as_ref()
            .map(|value| strip_html_tags(value))
            .filter(|value| !value.is_empty()),
        genres,
        categories,
    }
}

fn is_game_type(kind: Option<&str>) -> bool {
    let Some(kind) = kind else {
        return true;
    };

    let normalized = kind.trim().to_ascii_lowercase();
    normalized == "game" && !SKIP_TYPES.contains(&normalized.as_str())
}

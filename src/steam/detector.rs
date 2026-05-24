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

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum PromotionSkipReason {
    AppDetailsUnavailable,
    UnsupportedAppType,
    FreeToPlay,
    MissingPriceOverview,
    InitialPriceNotPositive,
    FinalPriceNotZero,
    DiscountNot100,
    MissingRequiredFields,
}

impl PromotionSkipReason {
    pub fn preview_reason_ru(&self) -> &'static str {
        match self {
            Self::AppDetailsUnavailable => {
                "Steam не вернул данные для этого appid или приложение недоступно в выбранном регионе."
            }
            Self::UnsupportedAppType => "тип приложения не подходит для публикации.",
            Self::FreeToPlay => "игра уже free-to-play.",
            Self::MissingPriceOverview => "Steam не вернул данные о цене.",
            Self::InitialPriceNotPositive => "обычная цена отсутствует или не больше нуля.",
            Self::FinalPriceNotZero => "текущая цена не равна 0.",
            Self::DiscountNot100 => "скидка не равна 100%.",
            Self::MissingRequiredFields => "в данных Steam не хватает обязательных полей.",
        }
    }

    pub fn breakdown_label_ru(&self) -> &'static str {
        match self {
            Self::AppDetailsUnavailable => "данные приложения недоступны",
            Self::UnsupportedAppType => "не игра/DLC/demo/software",
            Self::FreeToPlay => "уже free-to-play",
            Self::MissingPriceOverview => "нет данных цены",
            Self::InitialPriceNotPositive => "обычная цена не больше 0",
            Self::FinalPriceNotZero => "текущая цена не равна 0",
            Self::DiscountNot100 => "скидка не 100%",
            Self::MissingRequiredFields => "другое",
        }
    }
}

#[derive(Debug, Clone)]
pub enum PromotionEvaluation {
    Publishable(FreePromotion),
    Skipped(PromotionSkipReason),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CandidatePrefilterDecision {
    Passed,
    MissingPriceData,
    Skipped,
}

pub fn prefilter_candidate(candidate: &SteamCandidate) -> CandidatePrefilterDecision {
    let (Some(regular_price), Some(final_price), Some(discount_percent)) = (
        candidate.regular_price_cents,
        candidate.final_price_cents,
        candidate.discount_percent,
    ) else {
        return CandidatePrefilterDecision::MissingPriceData;
    };

    if regular_price > 0 && final_price == 0 && discount_percent == 100 {
        CandidatePrefilterDecision::Passed
    } else {
        CandidatePrefilterDecision::Skipped
    }
}

pub fn looks_like_excluded_title(title: &str) -> bool {
    let normalized = title.to_ascii_lowercase();
    let blocked_keywords = [
        "soundtrack",
        "ost",
        "dlc",
        "demo",
        "software",
        "tool",
        "editor",
    ];

    blocked_keywords
        .iter()
        .any(|keyword| normalized.contains(keyword))
}

pub fn evaluate_free_promotion(
    details: &SteamAppData,
    candidate: Option<&SteamCandidate>,
) -> PromotionEvaluation {
    if details.name.trim().is_empty() {
        return PromotionEvaluation::Skipped(PromotionSkipReason::MissingRequiredFields);
    }

    if !is_supported_game_type(details.kind.as_deref()) {
        return PromotionEvaluation::Skipped(PromotionSkipReason::UnsupportedAppType);
    }

    if is_free_to_play(details) {
        return PromotionEvaluation::Skipped(PromotionSkipReason::FreeToPlay);
    }

    let Some(price) = details.price_overview.as_ref() else {
        return PromotionEvaluation::Skipped(PromotionSkipReason::MissingPriceOverview);
    };

    if price.initial <= 0 {
        return PromotionEvaluation::Skipped(PromotionSkipReason::InitialPriceNotPositive);
    }

    if price.r#final != 0 {
        return PromotionEvaluation::Skipped(PromotionSkipReason::FinalPriceNotZero);
    }

    if price.discount_percent != 100 {
        return PromotionEvaluation::Skipped(PromotionSkipReason::DiscountNot100);
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

fn is_supported_game_type(kind: Option<&str>) -> bool {
    let Some(kind) = kind else {
        return true;
    };

    let normalized = kind.trim().to_ascii_lowercase();
    normalized == "game" && !SKIP_TYPES.contains(&normalized.as_str())
}

fn is_free_to_play(details: &SteamAppData) -> bool {
    let initial_price = details
        .price_overview
        .as_ref()
        .map(|price| price.initial)
        .unwrap_or_default();

    details.is_free.unwrap_or(false) && initial_price <= 0
}

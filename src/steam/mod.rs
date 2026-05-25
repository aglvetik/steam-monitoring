pub mod client;
pub mod detector;
pub mod models;
pub mod sources;
pub mod steamdb;
pub mod store_search;

pub use client::{AppDetailsResult, SteamClient, SteamClientConfig, SteamHttpDebugReport};
pub use detector::{
    looks_like_excluded_title, prefilter_candidate, validate_metadata_for_trusted_free_candidate,
    CandidatePrefilterDecision, PromotionEvaluation, PromotionSkipReason,
};
pub use models::{
    FreePromotion, SearchResultsResponse, SteamAppData, SteamCandidate, SteamGameData,
};
pub use steamdb::{
    SteamDbFreePromotionsReport, SteamDbPromotionEntry, STEAMDB_FREE_TO_KEEP_SOURCE_NAME,
};
pub use store_search::{
    SteamStoreSearchEntry, SteamStoreSearchReport, STEAM_STORE_SEARCH_SOURCE_NAME,
};

pub mod client;
pub mod detector;
pub mod models;
pub mod sources;
pub mod steamdb;

pub use client::{AppDetailsResult, SteamClient, SteamHttpDebugReport};
pub use detector::{
    looks_like_excluded_title, prefilter_candidate, CandidatePrefilterDecision,
    PromotionEvaluation, PromotionSkipReason,
};
pub use models::{FreePromotion, SteamAppData, SteamCandidate, SteamGameData};
pub use steamdb::{
    SteamDbFreePromotionsReport, SteamDbPromotionEntry, STEAMDB_FREE_TO_KEEP_SOURCE_NAME,
};

pub mod client;
pub mod detector;
pub mod models;
pub mod sources;

pub use client::{AppDetailsResult, SteamClient, SteamHttpDebugReport};
pub use detector::{PromotionEvaluation, PromotionSkipReason};
pub use models::{FreePromotion, SteamAppData, SteamCandidate, SteamGameData};

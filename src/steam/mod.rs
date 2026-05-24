pub mod client;
pub mod detector;
pub mod models;
pub mod sources;

pub use client::{SteamClient, SteamHttpDebugReport};
pub use detector::PromotionEvaluation;
pub use models::{FreePromotion, SteamAppData, SteamCandidate, SteamGameData};

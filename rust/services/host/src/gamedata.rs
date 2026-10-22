//! Game data (`legacy/ReforgedHost/src/gamedata.ts`): which files the backend and the Director need,
//! whether they are real or the shipped examples, and the generators that make them.
//!
//! Filled by RH4. `start` (RH2) and `doctor` (RH3) use [`game_data_state`].

use std::path::Path;

use crate::error::{HostError, not_implemented};

pub const BACKEND_GAME_DATA: [&str; 7] = [
    "progression_config.json",
    "slayers_path.json",
    "slayers_path_definitions.json",
    "ladyluck_store.json",
    "platinum_store.json",
    "reward_cache_store.json",
    "inventory_storage_policy.json",
];

pub const DIRECTOR_GAME_DATA: [&str; 7] = [
    "player_hunts_table.json",
    "matchmaker_hunts_table.json",
    "arena_easy_matchmaker_hunts.json",
    "arena_hard_matchmaker_hunts.json",
    "arena_elite_matchmaker_hunts.json",
    "arena_hard_matchmaker_hunts_new.json",
    "arena_elite_matchmaker_hunts_new.json",
];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GameDataKind {
    /// Every file there and none of them is its shipped example.
    Real,
    /// All there, some are the examples (start-up tests only).
    Synthetic,
    /// At least one file is not there.
    Missing,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GameDataState {
    pub state: GameDataKind,
    pub missing: Vec<String>,
    pub synthetic: Vec<String>,
}

pub fn game_data_state(_dir: &Path, _files: &[&str]) -> Result<GameDataState, HostError> {
    not_implemented("checking the game data", "RH4")
}

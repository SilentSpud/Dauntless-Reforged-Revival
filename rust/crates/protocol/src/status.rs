//! The live server status (shared contract 3, `GET /undaunted/api/ServerStatus`) as the launcher shows
//! it: `legacy/UndauntedLauncher/src/shared/status.ts`. These are the cleaned values; reading them from
//! a server's answer, where every field is checked, comes with L12.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[serde(rename_all = "lowercase")]
pub enum PlayerWhere {
    Menu,
    City,
    Hunt,
    Dojo,
    Tutorial,
    Unknown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[serde(rename_all = "lowercase")]
pub enum InstanceKind {
    City,
    Hunt,
    Dojo,
    Tutorial,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[serde(rename_all = "UPPERCASE")]
pub enum RegistrationMode {
    Open,
    Invitecode,
    None,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct StatusPlayer {
    pub name: String,
    #[serde(rename = "where")]
    pub location: PlayerWhere,
    pub instance: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[serde(rename_all = "camelCase")]
pub struct StatusInstance {
    pub id: String,
    pub kind: InstanceKind,
    pub title: String,
    pub map: String,
    pub behemoth: Option<String>,
    pub players: u32,
    pub max_players: u32,
    /// ISO string, or null when the server sent something unreadable
    pub started_at: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[serde(rename_all = "camelCase")]
pub struct ServerStatus {
    pub name: String,
    pub online: bool,
    pub version: String,
    pub commit: String,
    /// Only https URLs survive parsing
    pub source_url: Option<String>,
    pub registration: Option<RegistrationMode>,
    pub players_online: u32,
    pub players: Vec<StatusPlayer>,
    pub instances: Vec<StatusInstance>,
    pub content_port: Option<u16>,
    pub uptime_seconds: u32,
    /// The server hid who is online (playersOnline 0, no players, no instances): its answer to a
    /// launcher without an account key there, or with one it did not accept.
    pub limited: bool,
}

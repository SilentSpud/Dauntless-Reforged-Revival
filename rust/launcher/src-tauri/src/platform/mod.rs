//! The platform layer (rows L26 to L37): folders, settings, the key store, the log, the game folder,
//! launching, links, dialogs and the art protocol.

pub mod art;
pub mod client_mods;
pub mod dialogs;
pub mod game_config;
pub mod game_folder;
pub mod invite_link;
pub mod keystore;
pub mod launch;
pub mod links;
pub mod linux_runtime;
pub mod log;
pub mod paths;
pub mod settings;
pub mod system;
pub mod updater;

#[cfg(test)]
mod game_config_tests;

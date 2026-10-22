//! The bridge between the page and the backend (rows L6 to L9): the types that cross it, the fixed list
//! of commands, and the commands themselves.

pub mod backend;
pub mod commands;
pub mod progress;
pub mod spec;
pub mod types;

#[cfg(test)]
mod generate;

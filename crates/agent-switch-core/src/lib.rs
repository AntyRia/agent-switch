//! Core library for Agent Switch: profile storage, validation, isolated
//! Codex/Claude runtimes and the launcher. Both the CLI and the GUI build
//! on this crate so they always share the same profile files and behavior.

pub mod claude;
pub mod codex;
pub mod engine;
pub mod error;
pub mod health;
pub mod launcher;
pub mod logging;
pub mod models;
pub mod profile;
pub mod profile_store;
pub mod runtime;
pub mod sessions;
pub mod settings;
pub mod terminal;
pub mod validation;

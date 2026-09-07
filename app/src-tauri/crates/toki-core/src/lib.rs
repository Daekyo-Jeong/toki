//! Toki Engine — pure, headless game logic.
//!
//! ## Strict separation
//! This crate is the **canonical** game model. It runs without Tauri, tokio,
//! sqlite, or any IO. CPZ port (Phase 4) translates this module to Python.
//!
//! Any attempt to add an IO/UI dependency must be rejected at review.

pub mod game;

pub use game::*;

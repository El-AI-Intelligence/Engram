// Engram daemon — public API surface for integration tests.
//
// The main binary entry point is main.rs. This file exists to expose
// internal modules for integration testing (cargo test).

pub mod app_state;
pub mod auth;
pub mod errors;
pub mod routes;
pub mod sync_client;

pub use app_state::{AppState, CachedStore, LiveEvent};

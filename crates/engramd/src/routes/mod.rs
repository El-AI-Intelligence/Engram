pub mod health;
pub mod memories;
pub mod context;
pub mod consolidation;
pub mod analytics;
pub mod config;
pub mod export_import;
pub mod events;
pub mod annotations;
pub mod saved_searches;
pub mod privacy;
pub mod sync_status;
pub mod teams;
pub mod digest;
pub mod key_handoff;

// Re-export structured error helpers from the central errors module
pub use crate::errors::err_json;

//! Engram Memory Vault — standalone encrypted memory server.
//!
//! Start the server:
//! ```sh
//! engramd                          # local-only, default vault
//! engramd --port 8787              # custom port
//! engramd --passphrase "secret"    # user-provided encryption
//! ```

use clap::Parser;
use std::path::PathBuf;
use std::sync::Arc;
use tokio::net::TcpListener;

/// Engram Memory Vault — local-first encrypted memory for AI agents.
#[derive(Parser, Debug)]
#[command(version, about)]
struct Args {
    /// Port to listen on
    #[arg(short, long, default_value = "8787")]
    port: u16,

    /// Path to the vault directory
    #[arg(short, long, default_value = "~/.engram/vaults/default")]
    vault: PathBuf,

    /// Passphrase for vault encryption (overrides machine-id key)
    #[arg(short, long)]
    passphrase: Option<String>,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt::init();
    let args = Args::parse();

    // Resolve ~ in vault path
    let vault_path = if args.vault.starts_with("~/") {
        let home = std::env::var("HOME").unwrap_or_else(|_| "/tmp".to_string());
        PathBuf::from(home).join(args.vault.strip_prefix("~/").unwrap())
    } else {
        args.vault.clone()
    };
    std::fs::create_dir_all(&vault_path)?;

    // Open the encrypted vault
    let store = match &args.passphrase {
        Some(pw) => engram_core::EngramStore::open_with_passphrase(&vault_path, pw).await?,
        None => engram_core::EngramStore::open(&vault_path).await?,
    };
    let store = Arc::new(store);

    let addr = format!("127.0.0.1:{}", args.port);
    let listener = TcpListener::bind(&addr).await?;
    tracing::info!("Engram Memory Vault listening on http://{}", addr);
    tracing::info!("Vault: {}", vault_path.display());
    if args.passphrase.is_some() {
        tracing::info!("Encryption: passphrase-derived key");
    } else {
        tracing::info!("Encryption: machine-id-derived key");
    }

    // TODO: build axum router with all REST endpoints
    // For now, serve a health endpoint to verify the binary works
    let app = axum::Router::new()
        .route("/health", axum::routing::get(|| async {
            axum::Json(serde_json::json!({
                "status": "ok",
                "version": env!("CARGO_PKG_VERSION"),
                "vault": "default",
            }))
        }));

    axum::serve(listener, app).await?;
    Ok(())
}

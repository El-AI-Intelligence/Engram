//! CLI command handlers for the `engram` binary.
//!
//! When invoked as `engram` (or with subcommands), the binary dispatches here
//! instead of starting the daemon. Each handler opens the vault, performs its
//! operation, prints results, and exits.

use anyhow::{Context, Result};
use axiom_engram::{EngramStore, EngramLayer, EngramSource, Engram};
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine as _};
use clap::Subcommand;
use std::io::{IsTerminal, Write};
use std::path::PathBuf;

// ── CLI definition ─────────────────────────────────────────────────────────

#[derive(Subcommand, Debug)]
pub enum Commands {
    /// Start the engramd daemon (same as running `engramd` directly)
    Daemon {
        /// Path to the vault directory
        #[arg(short, long, default_value = "./engram-data", env = "ENGRAM_VAULT")]
        vault: PathBuf,
        /// Listen address
        #[arg(short, long, default_value = "127.0.0.1:8787", env = "ENGRAM_BIND")]
        bind: String,
        /// Passphrase for vault encryption
        #[arg(short, long, env = "ENGRAM_PASSPHRASE")]
        passphrase: Option<String>,
        /// Path to static UI files to serve
        #[arg(long, env = "ENGRAM_UI_DIR")]
        ui_dir: Option<PathBuf>,
        /// Read KEY=VALUE lines from this file into the environment before
        /// parsing (fills gaps only — real env and CLI flags win).
        #[arg(long, env = "ENGRAM_ENV_FILE")]
        env_file: Option<PathBuf>,
    },
    /// Interactive setup wizard — configure your vault
    Init,
    /// Guided 5-minute setup: vault → first memory → running daemon
    Onboarding {
        /// Daemon listen address for the onboarding flow
        #[arg(long, default_value = "127.0.0.1:8787")]
        bind: String,
    },
    /// Join an existing team vault — fresh vault + sync preset
    Join {
        /// Vault directory (defaults to ~/.engram/vault)
        #[arg(long)]
        vault: Option<PathBuf>,
        /// Sync server URL
        #[arg(long, default_value = "http://127.0.0.1:8788")]
        server_url: String,
        /// Sync server API key (omit for loopback servers)
        #[arg(long)]
        api_key: Option<String>,
        /// Shared vault ID (derived from the passphrase when omitted)
        #[arg(long)]
        vault_id: Option<String>,
        /// Team display name (optional)
        #[arg(long)]
        name: Option<String>,
    },
    /// Pair this machine with your Engram account — one-time code from the site
    Pair {
        /// One-time pairing code from the site (e.g. ENG-4F7K-9Q2M-D8T3)
        code: String,
        /// Vault directory (defaults to ~/.engram/vault)
        #[arg(long)]
        vault: Option<PathBuf>,
        /// Sync relay URL (defaults to the public Engram relay)
        #[arg(long, default_value = "https://sync.ellmstack.dev")]
        server_url: String,
        /// Vault site URL (where your vault opens after pairing)
        #[arg(long, default_value = "https://engram.ellmstack.dev/app")]
        site: String,
        /// Device label shown in Account & Sync (optional)
        #[arg(long)]
        name: Option<String>,
    },
    /// Link this machine to your Engram account — opens your browser, one
    /// click to approve (WARP-style). Use `engram pair` for headless setups.
    Link {
        /// Vault directory (defaults to ~/.engram/vault)
        #[arg(long)]
        vault: Option<PathBuf>,
        /// Sync relay URL (defaults to the public Engram relay)
        #[arg(long, default_value = "https://sync.ellmstack.dev")]
        server_url: String,
        /// Vault site URL opened for confirmation
        #[arg(long, default_value = "https://engram.ellmstack.dev/app")]
        site: String,
        /// Device label shown in Account & Sync (optional)
        #[arg(long)]
        name: Option<String>,
        /// Link even if this vault already has a sync key (the old key
        /// stays active until revoked in Account & Sync)
        #[arg(long)]
        force: bool,
    },
    /// Capture a memory into the vault
    Capture {
        /// The memory content to store
        content: Vec<String>,
        /// Comma-separated tags
        #[arg(short, long)]
        tags: Option<String>,
        /// Memory layer (episodic, semantic, imagined)
        #[arg(short, long, default_value = "episodic")]
        layer: String,
        /// Source of the memory
        #[arg(short, long, default_value = "interaction")]
        source: String,
        /// Emotional valence (-1.0 to 1.0)
        #[arg(long, default_value = "0.0")]
        valence: f64,
        /// Project identifier
        #[arg(short, long)]
        project: Option<String>,
        /// Path to vault (overrides config)
        #[arg(long)]
        vault: Option<PathBuf>,
    },
    /// Search memories by content or tags
    Search {
        /// Search query (content or tags)
        query: Vec<String>,
        /// Max results to return
        #[arg(short, long, default_value = "10")]
        limit: usize,
        /// Filter by layer
        #[arg(short, long)]
        layer: Option<String>,
        /// Path to vault (overrides config)
        #[arg(long)]
        vault: Option<PathBuf>,
    },
    /// Show today's captured memories as a timeline
    Today {
        /// Path to vault (overrides config)
        #[arg(long)]
        vault: Option<PathBuf>,
    },
    /// Show estimated environmental savings (CO₂ + tokens)
    Eco {
        /// Path to vault (overrides config)
        #[arg(long)]
        vault: Option<PathBuf>,
    },
    /// Print the vault passphrase from the env file (a secret — save it, never share it)
    ShowPassphrase {
        /// Path to the env file (defaults to ~/.engram/env)
        #[arg(long)]
        env_file: Option<PathBuf>,
    },
    /// Hand the vault sync keys to the browser exactly once: mints a one-time
    /// link that makes this vault "open by default" under the account signed
    /// in on the hosted site. The vault passphrase is never typed or shown.
    Handoff {
        /// Path to vault (overrides config)
        #[arg(long)]
        vault: Option<PathBuf>,
        /// Daemon address to mint the handoff from (defaults to 127.0.0.1:8799)
        #[arg(long, default_value = "127.0.0.1:8799")]
        bind: String,
        /// Site URL the link opens (defaults to https://engram.ellmstack.dev/app — the SPA root; the bare host serves the landing page, which ignores the #/handoff fragment)
        #[arg(long, default_value = "https://engram.ellmstack.dev/app")]
        site: String,
    },
    /// Seed 30 sample memories and simulate a month of decay — the "wow" demo
    Demo {
        /// Path to vault (overrides config)
        #[arg(long)]
        vault: Option<PathBuf>,
    },
    /// Backfill associative links between existing memories from their embeddings
    BackfillLinks {
        /// Path to vault (overrides config)
        #[arg(long)]
        vault: Option<PathBuf>,
        /// Max neighbors per memory
        #[arg(long, default_value = "5")]
        max_links: usize,
        /// Minimum cosine similarity for a link
        #[arg(long, default_value = "0.35")]
        min_similarity: f64,
    },
    /// Install MCP server config for AI editors (Claude Desktop, Cursor, Windsurf)
    Mcp {
        /// "install" or "status"
        #[arg(default_value = "status")]
        command: String,
        /// Engramd API URL the MCP server should talk to
        #[arg(long, default_value = "http://127.0.0.1:8787", env = "ENGRAMD_URL")]
        url: String,
        /// Print what would happen without writing anything
        #[arg(long)]
        dry_run: bool,
        /// Skip all confirmation prompts (non-interactive install)
        #[arg(long)]
        yes: bool,
    },
    /// Show the weekly digest — what your AI learned about you this week
    Digest {
        /// Engramd API URL
        #[arg(long, default_value = "http://127.0.0.1:8787")]
        url: String,
        /// Window length in days (1–90)
        #[arg(short, long, default_value = "7")]
        days: u32,
        /// Generate LLM prose (requires digest.llm in the daemon's config.json)
        #[arg(long)]
        prose: bool,
    },
}

// ── Vault path resolution ──────────────────────────────────────────────────

fn vault_path(cli_opt: Option<PathBuf>) -> PathBuf {
    let path = if let Some(p) = cli_opt {
        p
    } else if let Ok(p) = std::env::var("ENGRAM_VAULT") {
        PathBuf::from(p)
    } else {
        // Check config file from `engram init`
        let config_path = config_file_path();
        let from_config = std::fs::read_to_string(&config_path)
            .ok()
            .and_then(|data| serde_json::from_str::<serde_json::Value>(&data).ok())
            .and_then(|cfg| cfg.get("vault_path").and_then(|v| v.as_str()).map(PathBuf::from));
        if let Some(ref p) = from_config {
            if p.exists() {
                return p.clone();
            }
        }
        // Fallback
        PathBuf::from("./engram-data")
    };
    // Ensure the vault directory exists
    let _ = std::fs::create_dir_all(&path);
    path
}

fn home_dir() -> PathBuf {
    std::env::var("HOME")
        .or_else(|_| std::env::var("USERPROFILE"))
        .map(PathBuf::from)
        .unwrap_or_else(|_| PathBuf::from("."))
}

fn config_file_path() -> PathBuf {
    home_dir().join(".engram").join("config.json")
}

fn config_dir() -> PathBuf {
    home_dir().join(".engram")
}

/// Write ENGRAM_PASSPHRASE to ~/.engram/env (0600 on unix) so a
/// service-installed daemon can sync after reboots without re-typing it.
/// The value is never logged or printed. On Windows the user-profile ACL
/// is the protection (the service installer additionally tightens it).
fn write_env_file(passphrase: &str) -> std::result::Result<PathBuf, String> {
    let dir = config_dir();
    std::fs::create_dir_all(&dir).map_err(|e| format!("cannot create {}: {e}", dir.display()))?;
    let path = dir.join("env");
    std::fs::write(&path, format!("ENGRAM_PASSPHRASE={passphrase}\n"))
        .map_err(|e| format!("cannot write {}: {e}", path.display()))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600))
            .map_err(|e| format!("cannot chmod {}: {e}", path.display()))?;
    }
    Ok(path)
}

fn expand_tilde(path: &str) -> PathBuf {
    if path.starts_with("~/") {
        home_dir().join(&path[2..])
    } else {
        PathBuf::from(path)
    }
}

// ── Command handlers ───────────────────────────────────────────────────────

pub async fn handle_init() -> Result<()> {
    println!();
    println!("  ╔══════════════════════════════════════════╗");
    println!("  ║     Engram Memory Vault — Setup          ║");
    println!("  ║     Your AI deserves a memory.           ║");
    println!("  ╚══════════════════════════════════════════╝");
    println!();

    let home = home_dir();
    let default_path = home.join(".engram").join("vault");

    // Vault path
    println!("Where should your vault live?");
    println!("  [{}]", default_path.display());
    print!("> ");
    let mut input = String::new();
    std::io::stdin().read_line(&mut input)?;
    let vault_path = if input.trim().is_empty() {
        default_path
    } else {
        expand_tilde(input.trim())
    };

    // Passphrase
    println!();
    println!("Encryption passphrase (leave empty for machine-ID key):");
    print!("> ");
    let mut passphrase = String::new();
    std::io::stdin().read_line(&mut passphrase)?;
    let passphrase = passphrase.trim().to_string();
    let has_passphrase = !passphrase.is_empty();

    if has_passphrase {
        // Confirm only on an interactive terminal — piped stdin can't
        // supply a second line, and EOF would abort otherwise.
        if std::io::stdin().is_terminal() {
            println!("Confirm passphrase:");
            print!("> ");
            let mut confirm = String::new();
            std::io::stdin().read_line(&mut confirm)?;
            let confirm = confirm.trim().to_string();
            if passphrase != confirm {
                println!();
                println!("  ❌ Passphrases do not match. Please run `engram init` again.");
                println!();
                return Ok(());
            }
        }
        if passphrase.len() < 8 {
            println!();
            println!("  ⚠️  Passphrase is short (< 8 characters). For better security,");
            println!("     consider using a longer passphrase or a password manager.");
            println!();
        }
    }

    // Create vault
    std::fs::create_dir_all(&vault_path)?;
    let _store = if has_passphrase {
        EngramStore::open_with_passphrase(&vault_path, &passphrase).await?
    } else {
        EngramStore::open(&vault_path).await?
    };

    // Save config
    let cfg_dir = config_dir();
    std::fs::create_dir_all(&cfg_dir)?;
    let config = serde_json::json!({
        "vault_path": vault_path.to_string_lossy(),
        "has_passphrase": has_passphrase,
        "created_at": chrono::Utc::now().to_rfc3339(),
        "default_url": "http://localhost:8787",
        "schedule": {
            "decay_interval_hours": 1,
            "consolidation_interval_hours": 24,
            "auto_decay": true,
            "auto_consolidation": true,
        },
        "sync": {
            "enabled": false,
            "server_url": null,
            "api_key": null,
            "interval_secs": 60,
        },
    });
    std::fs::write(config_file_path(), serde_json::to_string_pretty(&config)?)?;

    println!();
    println!("  ✅ Vault created at {}", vault_path.display());
    println!("  ✅ Config saved to {}", config_file_path().display());
    println!();

    // Persist the passphrase so a service-installed daemon can sync after
    // reboots without anyone re-typing it (the daemon reads it at startup).
    if has_passphrase {
        match write_env_file(&passphrase) {
            Ok(path) => println!("  ✅ Passphrase stored in {} — the daemon reads it on restart", path.display()),
            Err(e) => println!("  ⚠  Could not write env file: {e}"),
        }
    }

    // Offer system service installation
    print!("\n  Install as a background service (start on boot)? [Y/n]: ");
    let mut answer = String::new();
    std::io::stdin().read_line(&mut answer)?;
    if answer.trim().to_lowercase() != "n" && answer.trim().to_lowercase() != "no" {
        match install_service(&vault_path, &home) {
            Ok(msg) => println!("  {msg}"),
            Err(e) => println!("  ⚠  Service installation skipped: {e}"),
        }
    }

    println!();
    println!("  Next steps:");
    println!("    engram daemon          Start the vault server");
    println!("    engram capture \"...\"   Capture your first memory");
    println!("    engram demo            See the demo");
    println!();

    Ok(())
}

/// Join a team vault: fresh vault + passphrase + sync preset written to the
/// vault-local config.json. The passphrase MUST match the team's — sync keys
/// derive from it alone, and the server verifies every blob's HMAC against
/// them. A wrong passphrase means the server rejects (or the vault silently
/// splits) — there is no account recovery, by design.
pub async fn handle_join(
    vault_opt: Option<PathBuf>,
    server_url: String,
    api_key: Option<String>,
    vault_id: Option<String>,
    name: Option<String>,
) -> Result<()> {
    println!();
    println!("  ╔══════════════════════════════════════════╗");
    println!("  ║     Engram — Join a Team Vault           ║");
    println!("  ║     One passphrase. One vault.           ║");
    println!("  ╚══════════════════════════════════════════╝");
    println!();

    let default_path = home_dir().join(".engram").join("vault");
    let vault_path = vault_opt.unwrap_or(default_path);

    // Never touch a vault that already has memories — join is for fresh
    // vaults; existing vaults configure sync via the Settings panel.
    if vault_path.join("engrams.db").exists() {
        anyhow::bail!(
            "{} already contains a vault (engrams.db exists).\n\
             `engram join` is for fresh vaults. To sync an existing vault, configure\n\
             the sync block in its config.json or use Settings → Sync & Team in the UI.",
            vault_path.display()
        );
    }

    // Passphrase — required (machine-keyed vaults cannot sync).
    println!("Team passphrase (the SAME one your teammates use; leave blank to abort):");
    print!("> ");
    let mut passphrase = String::new();
    std::io::stdin().read_line(&mut passphrase)?;
    let passphrase = passphrase.trim().to_string();
    if passphrase.is_empty() {
        println!();
        println!("  ⚠️  Join aborted — sync requires a passphrase.");
        println!();
        return Ok(());
    }
    if std::io::stdin().is_terminal() {
        println!("Confirm passphrase:");
        print!("> ");
        let mut confirm = String::new();
        std::io::stdin().read_line(&mut confirm)?;
        if passphrase != confirm.trim() {
            println!();
            println!("  ❌ Passphrases do not match. Please run `engram join` again.");
            println!();
            return Ok(());
        }
    }
    match write_env_file(&passphrase) {
        Ok(path) => println!("  ✅ Passphrase stored in {} — the daemon reads it on restart", path.display()),
        Err(e) => println!("  ⚠  Could not write env file: {e}"),
    }
    if passphrase.len() < 8 {
        println!();
        println!("  ⚠️  Passphrase is short (< 8 characters). Consider a longer one —");
        println!("     the whole team's vault security rests on it.");
        println!();
    }

    // Create the vault (writes device.json with a fresh device_id)
    std::fs::create_dir_all(&vault_path)?;
    let _store = EngramStore::open_with_passphrase(&vault_path, &passphrase).await?;

    // Sync preset in the vault-local config.json (what the daemon reads).
    // vault_id is omitted when not given — the daemon derives it from the
    // passphrase and pins it on first sync.
    let mut sync = serde_json::json!({
        "enabled": true,
        "server_url": server_url,
        "api_key": api_key,
        "interval_secs": 60,
    });
    if let Some(ref id) = vault_id {
        sync["vault_id"] = serde_json::Value::String(id.clone());
    }
    if let Some(ref n) = name {
        sync["name"] = serde_json::Value::String(n.clone());
    }
    let config = serde_json::json!({ "sync": sync });
    let cfg_path = vault_path.join("config.json");
    std::fs::write(&cfg_path, serde_json::to_string_pretty(&config)?)?;

    // Global CLI config (vault_path pointer) — only if one isn't already set
    // for another vault; it is never required for the daemon.
    let global_cfg = config_file_path();
    if !global_cfg.exists() {
        std::fs::create_dir_all(config_dir())?;
        let global = serde_json::json!({
            "vault_path": vault_path.to_string_lossy(),
            "has_passphrase": true,
            "created_at": chrono::Utc::now().to_rfc3339(),
            "default_url": "http://localhost:8787",
        });
        std::fs::write(&global_cfg, serde_json::to_string_pretty(&global)?)?;
        println!("  ✅ Global config saved to {}", global_cfg.display());
    }

    println!();
    println!("  ✅ Vault created at {}", vault_path.display());
    println!("  ✅ Sync preset written to {}", cfg_path.display());
    if vault_id.is_none() {
        println!("     (vault_id unset — derived from the passphrase on first sync)");
    }
    if api_key.is_some() {
        println!("     (api_key stored — it is masked in all status output)");
    }
    println!();
    println!("  Next steps:");
    println!("    engramd --vault {} --passphrase \"<team passphrase>\"", vault_path.display());
    println!("    curl -X POST http://localhost:8787/sync/now   # force the first sync");
    println!();
    println!("  Teammates' memories appear within one sync interval.");
    println!();

    Ok(())
}

/// Pair this machine with an Engram account using a one-time code minted on
/// the site (Account & Sync → "Pair a device"). The relay exchanges the code
/// for an account API key, which lands in the vault-local config.json
/// exactly like `engram join` — but pair also works on EXISTING vaults.
/// The passphrase is never stored in config.json, and the key is never
/// printed.
/// Shared setup for the machine→account flows (`engram pair` / `engram link`):
/// resolve the vault path, bail on positively-known machine-keyed vaults, and
/// create a fresh passphrase vault when the path doesn't exist yet (sync keys
/// derive from the passphrase — it is required, blank aborts).
///
/// Returns None when the user aborted at the passphrase prompt.
async fn ensure_vault_for_sync(
    vault_opt: Option<PathBuf>,
    verb: &str,
) -> Result<Option<(PathBuf, bool, Option<String>)>> {
    let default_path = home_dir().join(".engram").join("vault");
    let vault_path = vault_opt.unwrap_or(default_path);
    let existing = vault_path.join("engrams.db").exists();

    // Machine-keyed vaults cannot sync: sync keys derive from the passphrase
    // alone (the server verifies every blob's HMAC against them). The global
    // config records how `engram init` created the vault. Only bail when we
    // POSITIVELY know it is machine-keyed — otherwise the daemon surfaces
    // the same requirement at startup.
    if existing {
        let is_machine_keyed = std::fs::read_to_string(config_file_path())
            .ok()
            .and_then(|data| serde_json::from_str::<serde_json::Value>(&data).ok())
            .and_then(|cfg| cfg.get("has_passphrase").and_then(|v| v.as_bool()))
            == Some(false);
        if is_machine_keyed {
            anyhow::bail!(
                "{} is a machine-keyed vault (created without a passphrase).\n\
                 Sync is end-to-end encrypted with the vault passphrase, so machine-keyed\n\
                 vaults cannot sync. Create a passphrase vault (re-run `engram init` with\n\
                 a passphrase, or `engram {verb}` on a fresh vault path) and {verb} that instead.",
                vault_path.display()
            );
        }
    }

    // Fresh vault: prompt for the passphrase (required for sync) and create
    // the vault, same as `engram join`.
    let mut fresh_passphrase: Option<String> = None;
    if !existing {
        println!("Vault passphrase (sync keys derive from it; leave blank to abort):");
        print!("> ");
        std::io::stdout().flush()?;
        let mut passphrase = String::new();
        std::io::stdin().read_line(&mut passphrase)?;
        let passphrase = passphrase.trim().to_string();
        if passphrase.is_empty() {
            println!();
            println!("  ⚠️  {verb} aborted — sync requires a passphrase.");
            println!();
            return Ok(None);
        }
        if std::io::stdin().is_terminal() {
            println!("Confirm passphrase:");
            print!("> ");
            std::io::stdout().flush()?;
            let mut confirm = String::new();
            std::io::stdin().read_line(&mut confirm)?;
            if passphrase != confirm.trim() {
                println!();
                println!("  ❌ Passphrases do not match. Please run `engram {verb}` again.");
                println!();
                return Ok(None);
            }
        }
        if passphrase.len() < 8 {
            println!();
            println!("  ⚠️  Passphrase is short (< 8 characters). Consider a longer one.");
            println!();
        }
        std::fs::create_dir_all(&vault_path)?;
        let _store = EngramStore::open_with_passphrase(&vault_path, &passphrase).await?;
        match write_env_file(&passphrase) {
            Ok(path) => println!("  ✅ Passphrase stored in {} — the daemon reads it on restart", path.display()),
            Err(e) => println!("  ⚠  Could not write env file: {e}"),
        }
        fresh_passphrase = Some(passphrase);
    }

    Ok(Some((vault_path, existing, fresh_passphrase)))
}

/// Persist a freshly issued account API key the way `pair` and `link` both
/// need it: sync preset in vault-local config.json (field-wise merge),
/// device roster label, and a sync_state reset so EXISTING memories re-sync
/// against the new relay.
fn save_sync_credential(
    vault_path: &std::path::Path,
    server_url: &str,
    api_key: &str,
    name: &Option<String>,
) -> Result<()> {
    let cfg_path = vault_path.join("config.json");
    let mut config: serde_json::Value = std::fs::read_to_string(&cfg_path)
        .ok()
        .and_then(|data| serde_json::from_str(&data).ok())
        .unwrap_or(serde_json::json!({}));
    let mut sync = config
        .get("sync")
        .cloned()
        .unwrap_or(serde_json::json!({}));
    sync["enabled"] = serde_json::json!(true);
    sync["server_url"] = serde_json::Value::String(server_url.to_string());
    sync["api_key"] = serde_json::Value::String(api_key.to_string()); // never printed
    sync["interval_secs"] = serde_json::json!(60);
    if let Some(ref n) = name {
        sync["name"] = serde_json::Value::String(n.clone());
    }
    // The roster label comes from vault-local device.json (the sync loop
    // registers it on startup). Older vaults carry the "unknown" placeholder —
    // name the device from --name, or fall back to the sync preset name, so
    // Account & Sync shows who this machine is.
    let label = name
        .clone()
        .or_else(|| sync.get("name").and_then(|v| v.as_str().map(String::from)));
    config["sync"] = sync;
    std::fs::write(&cfg_path, serde_json::to_string_pretty(&config)?)?;

    if let Some(label) = label {
        let dev_path = vault_path.join("device.json");
        let mut dev: serde_json::Value = std::fs::read_to_string(&dev_path)
            .ok()
            .and_then(|data| serde_json::from_str(&data).ok())
            .unwrap_or(serde_json::json!({}));
        let current = dev.get("label").and_then(|v| v.as_str()).unwrap_or("");
        // An explicit --name always wins; otherwise only replace the
        // placeholder so a hand-set label survives re-pairing.
        if name.is_some() || current.is_empty() || current == "unknown" {
            if dev.get("device_id").and_then(|v| v.as_str()).is_none() {
                dev["device_id"] = serde_json::Value::String(uuid::Uuid::new_v4().to_string());
                dev["created_at"] = serde_json::Value::String(chrono::Utc::now().to_rfc3339());
            }
            dev["label"] = serde_json::Value::String(label);
            std::fs::write(&dev_path, serde_json::to_string_pretty(&dev)?)?;
        }
    }

    // Re-pointing a vault at a new relay must re-sync its EXISTING
    // memories: sync_state.json remembers pushes against the old relay,
    // so without a reset the daemon would only ever sync memories
    // captured after the pair. Dropping it makes the next tick a full
    // first sync (push everything local, pull everything remote).
    let sync_state = vault_path.join("sync_state.json");
    if sync_state.exists() {
        std::fs::remove_file(&sync_state)?;
    }

    Ok(())
}

/// The post-pair/link next-steps block — identical for both flows.
fn print_sync_next_steps(fresh_vault: bool, vault_path: &std::path::Path) {
    println!();
    println!("  Next steps:");
    if fresh_vault {
        println!("    1. Start the daemon (the passphrase is your vault key — keep it safe):");
        println!("       ENGRAM_PASSPHRASE=\"<your passphrase>\" engramd --vault {}", vault_path.display());
        println!("    2. The device appears in Account & Sync → Devices after its first sync.");
    } else {
        println!("    1. Restart the daemon so it picks up the sync preset");
        println!("       (with ENGRAM_PASSPHRASE set, as it already runs).");
        println!("    2. The device appears in Account & Sync → Devices after its first sync.");
    }
}

/// The site URL for this vault: a per-vault deep link once the daemon's
/// first sync pins `sync.vault_id` into the vault's config.json, the
/// unlock picker until then (an honest fallback — the per-vault link
/// appears here after the first sync).
fn vault_site_link(vault_path: &std::path::Path, site: &str) -> String {
    let site = site.trim_end_matches('/');
    let pinned = std::fs::read_to_string(vault_path.join("config.json"))
        .ok()
        .and_then(|data| serde_json::from_str::<serde_json::Value>(&data).ok())
        .and_then(|cfg| {
            cfg.get("sync")
                .and_then(|s| s.get("vault_id"))
                .and_then(|v| v.as_str())
                .map(str::to_string)
        });
    match pinned {
        Some(id) if !id.is_empty() => format!("{site}/#/vault/{id}"),
        _ => format!("{site}/#/unlock"),
    }
}

pub async fn handle_pair(
    code: String,
    vault_opt: Option<PathBuf>,
    server_url: String,
    site: String,
    name: Option<String>,
) -> Result<()> {
    println!();
    println!("  ╔══════════════════════════════════════════╗");
    println!("  ║     Engram — Pair This Device            ║");
    println!("  ║     One code. One machine.               ║");
    println!("  ╚══════════════════════════════════════════╝");
    println!();

    let Some((vault_path, _existing, fresh_passphrase)) =
        ensure_vault_for_sync(vault_opt, "pair").await?
    else {
        return Ok(());
    };

    // Redeem the code for an account API key. Codes are single-use and last
    // 10 minutes — the relay is the source of truth for both.
    let code = code.trim().to_ascii_uppercase();
    let mut body = serde_json::json!({ "code": code });
    if let Some(ref n) = name {
        body["device_label"] = serde_json::Value::String(n.clone());
    }
    let client = reqwest::Client::new();
    let url = format!("{}/devices/pair", server_url.trim_end_matches('/'));
    let resp = match client.post(&url).json(&body).send().await {
        Ok(r) => r,
        Err(e) => {
            anyhow::bail!(
                "cannot reach {server_url} ({e}).\n\
                 Custom relays need --server-url <url>."
            );
        }
    };
    if !resp.status().is_success() {
        let status = resp.status();
        let err_body: serde_json::Value = resp.json().await.unwrap_or(serde_json::Value::Null);
        let err_code = err_body
            .get("code")
            .and_then(|v| v.as_str())
            .unwrap_or("unknown_error");
        match (status.as_u16(), err_code) {
            (401, "expired_pairing_code") => anyhow::bail!(
                "pairing code expired (codes last 10 minutes).\n\
                 Mint a new one from the site: Account & Sync → Pair a device."
            ),
            (401, "invalid_pairing_code") => anyhow::bail!(
                "pairing code rejected (unknown or already used).\n\
                 Mint a new one from the site: Account & Sync → Pair a device."
            ),
            (429, _) => anyhow::bail!("too many pairing attempts — wait a moment and try again."),
            _ => anyhow::bail!("pairing failed: {err_code}"),
        }
    }
    let body: serde_json::Value = resp.json().await?;
    let api_key = body
        .get("api_key")
        .and_then(|v| v.as_str())
        .ok_or_else(|| anyhow::anyhow!("relay response missing api_key"))?;

    save_sync_credential(&vault_path, &server_url, api_key, &name)?;

    println!();
    println!("  ✅ Paired! The relay issued a new API key — stored in {}", vault_path.join("config.json").display());
    println!("     (the key is masked in all status output; it is not printed here)");
    if fresh_passphrase.is_some() {
        println!("     (vault created at {} — passphrase NOT stored on disk)", vault_path.display());
    }
    println!("     Your vault on the site: {}", vault_site_link(&vault_path, &site));
    print_sync_next_steps(fresh_passphrase.is_some(), &vault_path);
    println!();

    Ok(())
}

/// `engram link` — WARP-style one-click linking: mint an ephemeral X25519
/// keypair, ask the relay for a link intent, open the confirm URL in the
/// browser, and poll until the signed-in user clicks "Link this machine".
/// The account key arrives sealed to our ephemeral keypair (never plaintext
/// over the wire) and is decrypted exactly once, then stored exactly like
/// `engram pair` stores it.
pub async fn handle_link(
    vault_opt: Option<PathBuf>,
    server_url: String,
    site: String,
    name: Option<String>,
    force: bool,
) -> Result<()> {
    println!();
    println!("  ╔══════════════════════════════════════════╗");
    println!("  ║   Engram — Link This Machine             ║");
    println!("  ║   Your browser opens. One click. Done.   ║");
    println!("  ╚══════════════════════════════════════════╝");
    println!();

    // Already linked? Re-linking orphans the old key (revocable at
    // Account & Sync) — confirm intent unless --force.
    if !force {
        let default_path = home_dir().join(".engram").join("vault");
        let vault_path = vault_opt.clone().unwrap_or(default_path);
        let cfg = std::fs::read_to_string(vault_path.join("config.json"))
            .ok()
            .and_then(|data| serde_json::from_str::<serde_json::Value>(&data).ok());
        if cfg
            .as_ref()
            .and_then(|c| c.get("sync"))
            .and_then(|s| s.get("api_key"))
            .and_then(|v| v.as_str())
            .is_some()
        {
            anyhow::bail!(
                "{} is already linked to an account.\n\
                 Re-run with --force to replace the key (the old key stays active until\n\
                 you revoke it in Account & Sync → API keys).",
                vault_path.display()
            );
        }
    }

    let Some((vault_path, _existing, fresh_passphrase)) =
        ensure_vault_for_sync(vault_opt, "link").await?
    else {
        return Ok(());
    };

    // Ephemeral keypair — the secret exists only for the life of this run
    // and is dropped before the decrypted key is persisted.
    let (sk_cli, pk_cli) = crate::link::generate_ephemeral_keypair();
    let mut sk_cli = Some(sk_cli);
    let pk_b64 = URL_SAFE_NO_PAD.encode(pk_cli.as_bytes());

    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(10))
        .build()?;
    let mut body = serde_json::json!({ "public_key": pk_b64 });
    if let Some(ref n) = name {
        body["device_label"] = serde_json::Value::String(n.clone());
    }
    let relay = server_url.trim_end_matches('/');
    let resp = match client
        .post(format!("{relay}/devices/link-intents"))
        .json(&body)
        .send()
        .await
    {
        Ok(r) => r,
        Err(e) => {
            anyhow::bail!(
                "cannot reach {server_url} ({e}).\n\
                 Custom relays need --server-url <url>."
            );
        }
    };
    let intent: serde_json::Value = match resp.status().is_success() {
        true => resp.json().await?,
        false => {
            let status = resp.status();
            let err_body: serde_json::Value = resp.json().await.unwrap_or(serde_json::Value::Null);
            let err_code = err_body
                .get("code")
                .and_then(|v| v.as_str())
                .unwrap_or("unknown_error");
            match (status.as_u16(), err_code) {
                (429, _) => anyhow::bail!("too many link attempts — wait a moment and try again."),
                _ => anyhow::bail!("link failed: {err_code}"),
            }
        }
    };
    let intent_id = intent
        .get("id")
        .and_then(|v| v.as_str())
        .ok_or_else(|| anyhow::anyhow!("relay response missing id"))?
        .to_string();
    let code = intent
        .get("code")
        .and_then(|v| v.as_str())
        .ok_or_else(|| anyhow::anyhow!("relay response missing code"))?
        .to_string();
    let relay_pk_bytes = URL_SAFE_NO_PAD
        .decode(
            intent
                .get("relay_public_key")
                .and_then(|v| v.as_str())
                .ok_or_else(|| anyhow::anyhow!("relay response missing relay_public_key"))?
                .as_bytes(),
        )
        .map_err(|_| anyhow::anyhow!("relay_public_key is not base64url"))?;
    let relay_pk: x25519_dalek::PublicKey = x25519_dalek::PublicKey::from(
        <[u8; 32]>::try_from(relay_pk_bytes.as_slice())
            .map_err(|_| anyhow::anyhow!("relay_public_key must be 32 bytes"))?,
    );

    let confirm_url = format!(
        "{}/#/link/{}?code={}",
        site.trim_end_matches('/'),
        intent_id,
        code
    );
    println!("  A browser tab is opening to link this machine to your Engram account.");
    println!("  Sign in and click \"Link this machine\" — this window finishes on its own.");
    println!();
    println!("  {}", confirm_url);
    println!();
    crate::link::open_browser(&confirm_url);

    // Poll for the seal: 2s cadence, ~10 minutes (matches the relay TTL).
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(620);
    let mut api_key: Option<String> = None;
    while std::time::Instant::now() < deadline {
        tokio::time::sleep(std::time::Duration::from_secs(2)).await;
        let resp = match client
            .get(format!("{relay}/devices/link-intents/{intent_id}/status"))
            .send()
            .await
        {
            Ok(r) => r,
            Err(_) => continue, // transient network hiccup — keep polling
        };
        match resp.status().as_u16() {
            200 => {
                let body: serde_json::Value = match resp.json().await {
                    Ok(b) => b,
                    Err(_) => continue,
                };
                match crate::link::parse_link_status(&body) {
                    Ok(crate::link::LinkStatus::Confirmed { sealed_key, nonce }) => {
                        let key = crate::link::decrypt_link_key(
                            &intent_id,
                            &relay_pk,
                            sk_cli.as_ref().expect("keypair alive during poll"),
                            &sealed_key,
                            &nonce,
                        )?;
                        api_key = Some(key);
                        break;
                    }
                    Ok(crate::link::LinkStatus::Pending) => continue,
                    Err(_) => continue,
                }
            }
            410 => anyhow::bail!("link expired or already claimed — run `engram link` again."),
            404 => anyhow::bail!("the relay no longer knows this link — run `engram link` again."),
            429 => continue, // polling faster than the bucket — next tick lands later
            _ => continue,
        }
    }
    let api_key = match api_key {
        Some(k) => k,
        None => anyhow::bail!(
            "timed out waiting for the browser (10 minutes).\n\
             The link expired — run `engram link` again."
        ),
    };
    // The ephemeral secret has served its purpose — drop it before the
    // decrypted key is written to disk.
    drop(sk_cli.take());

    save_sync_credential(&vault_path, &server_url, &api_key, &name)?;

    println!();
    println!("  ✅ Linked! The relay issued a new API key — stored in {}", vault_path.join("config.json").display());
    println!("     (the key is masked in all status output; it is not printed here)");
    if fresh_passphrase.is_some() {
        println!("     (vault created at {} — passphrase NOT stored on disk)", vault_path.display());
    }
    println!("     Your vault on the site: {}", vault_site_link(&vault_path, &site));
    print_sync_next_steps(fresh_passphrase.is_some(), &vault_path);
    println!();

    Ok(())
}

/// Install engramd as a system service (systemd on Linux, launchd on macOS).
fn install_service(vault_path: &PathBuf, home: &PathBuf) -> std::result::Result<String, String> {
    #[cfg(target_os = "linux")]
    {
        let svc_dir = home.join(".config/systemd/user");
        std::fs::create_dir_all(&svc_dir)
            .map_err(|e| format!("cannot create systemd dir: {e}"))?;

        let svc_path = svc_dir.join("engramd.service");
        let service = format!(
            "[Unit]\n\
             Description=Engram Memory Vault daemon\n\
             After=network-online.target\n\
             Wants=network-online.target\n\
             \n\
             [Service]\n\
             Type=simple\n\
             ExecStart={bindir}/engramd --vault {vault} --bind 127.0.0.1:8787\n\
             Restart=always\n\
             RestartSec=5\n\
             EnvironmentFile=-{envfile}\n\
             NoNewPrivileges=yes\n\
             PrivateTmp=yes\n\
             LimitNOFILE=4096\n\
             MemoryMax=512M\n\
             \n\
             [Install]\n\
             WantedBy=default.target\n",
            bindir = if home.join(".local/bin/engramd").exists() {
                format!("{}/.local/bin", home.display())
            } else {
                "/usr/local/bin".to_string()
            },
            vault = vault_path.display(),
            envfile = home.join(".engram/env").display(),
        );
        std::fs::write(&svc_path, &service)
            .map_err(|e| format!("cannot write service file: {e}"))?;

        // Try to enable the service
        let output = std::process::Command::new("systemctl")
            .args(["--user", "daemon-reload"])
            .output();
        if let Ok(o) = &output {
            if !o.status.success() {
                return Err(format!("systemctl daemon-reload failed: {}",
                    String::from_utf8_lossy(&o.stderr)));
            }
        }

        let output = std::process::Command::new("systemctl")
            .args(["--user", "enable", "--now", "engramd"])
            .output()
            .map_err(|e| format!("cannot run systemctl: {e}"))?;
        if !output.status.success() {
            return Err(format!("systemctl enable failed: {}",
                String::from_utf8_lossy(&output.stderr)));
        }

        Ok(format!("✅ Systemd service installed and started.\n   Check status: systemctl --user status engramd"))
    }

    #[cfg(target_os = "macos")]
    {
        let svc_dir = home.join("Library/LaunchAgents");
        std::fs::create_dir_all(&svc_dir)
            .map_err(|e| format!("cannot create LaunchAgents dir: {e}"))?;

        let svc_path = svc_dir.join("com.ellmstack.engramd.plist");
        let plist = format!(
            "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n\
             <!DOCTYPE plist PUBLIC \"-//Apple//DTD PLIST 1.0//EN\" \"http://www.apple.com/DTDs/PropertyList-1.0.dtd\">\n\
             <plist version=\"1.0\">\n\
             <dict>\n\
                 <key>Label</key>\n\
                 <string>com.ellmstack.engramd</string>\n\
                 <key>ProgramArguments</key>\n\
                 <array>\n\
                     <string>{bindir}/engramd</string>\n\
                     <string>--vault</string>\n\
                     <string>{vault}</string>\n\
                     <string>--bind</string>\n\
                     <string>127.0.0.1:8787</string>\n\
                     <string>--env-file</string>\n\
                     <string>{envfile}</string>\n\
                 </array>\n\
                 <key>RunAtLoad</key><true/>\n\
                 <key>KeepAlive</key><true/>\n\
                 <key>ThrottleInterval</key><integer>5</integer>\n\
                 <key>StandardOutPath</key>\n\
                 <string>{logfile}</string>\n\
                 <key>StandardErrorPath</key>\n\
                 <string>{logfile}</string>\n\
                 <key>ProcessType</key>\n\
                 <string>Background</string>\n\
                 <key>Nice</key><integer>10</integer>\n\
             </dict>\n\
             </plist>\n",
            bindir = if home.join(".local/bin/engramd").exists() {
                format!("{}/.local/bin", home.display())
            } else {
                "/usr/local/bin".to_string()
            },
            vault = vault_path.display(),
            envfile = home.join(".engram").join("env").display(),
            logfile = home.join(".engram/daemon.log").display(),
        );
        std::fs::write(&svc_path, &plist)
            .map_err(|e| format!("cannot write plist: {e}"))?;

        let output = std::process::Command::new("launchctl")
            .args(["load", svc_path.to_str().unwrap_or("")])
            .output()
            .map_err(|e| format!("cannot run launchctl: {e}"))?;
        if !output.status.success() {
            return Err(format!("launchctl load failed: {}",
                String::from_utf8_lossy(&output.stderr)));
        }

        Ok(format!("✅ LaunchAgent installed and started.\n   Check status: launchctl list | grep engramd"))
    }

    #[cfg(target_os = "windows")]
    {
        // Resolve engramd.exe: normally sits next to engram.exe; fall back
        // to ~/.local/bin (the install.ps1 location).
        let exe_dir = std::env::current_exe()
            .ok()
            .and_then(|p| p.parent().map(|d| d.to_path_buf()))
            .unwrap_or_else(|| home.join(".local/bin"));
        let daemon_exe = if exe_dir.join("engramd.exe").is_file() {
            exe_dir.join("engramd.exe")
        } else {
            home.join(".local/bin").join("engramd.exe")
        };
        if !daemon_exe.is_file() {
            return Err("engramd.exe not found next to engram.exe or in ~/.local/bin".to_string());
        }

        let envfile = home.join(".engram").join("env");
        let logfile = home.join(".engram").join("daemon.log");
        // PowerShell single-quote escaping: wrap in '…' and double embedded '.
        let ps_quote = |s: &str| format!("'{}'", s.replace('\'', "''"));

        // Hidden-console wrapper: blocks until the daemon exits, propagates
        // its exit code (so the Task Scheduler restart policy sees crashes),
        // and appends logs to ~/.engram/daemon.log. Only paths appear here —
        // the passphrase is read by the daemon from the env file.
        let wrapper = format!(
            "& {} --vault {} --bind 127.0.0.1:8787 --env-file {} *>> {}; exit $LASTEXITCODE",
            ps_quote(&daemon_exe.display().to_string()),
            ps_quote(&vault_path.display().to_string()),
            ps_quote(&envfile.display().to_string()),
            ps_quote(&logfile.display().to_string()),
        );
        // -Command strings are not subject to execution policy, so the task
        // works regardless of the machine's PowerShell policy.
        let script = format!(
            "$a = New-ScheduledTaskAction -Execute 'powershell.exe' -Argument '-NoProfile -NonInteractive -WindowStyle Hidden -Command \"{wrapper}\"';\n\
             $t = New-ScheduledTaskTrigger -AtLogOn;\n\
             $s = New-ScheduledTaskSettingsSet -RestartCount 3 -RestartInterval (New-TimeSpan -Minutes 1) -MultipleInstances IgnoreNew -ExecutionTimeLimit ([TimeSpan]::Zero) -Hidden;\n\
             $p = New-ScheduledTaskPrincipal -UserId \"$env:USERNAME\" -LogonType Interactive -RunLevel Limited;\n\
             Register-ScheduledTask -TaskName 'Engramd' -Action $a -Trigger $t -Settings $s -Principal $p -Force | Out-Null;\n\
             Start-ScheduledTask -TaskName 'Engramd';\n\
             icacls {} /inheritance:r /grant:r \"$env:USERNAME:(R)\" 2>$null",
            ps_quote(&envfile.display().to_string()),
        );
        let output = std::process::Command::new("powershell.exe")
            .args(["-NoProfile", "-NonInteractive", "-Command", &script])
            .output()
            .map_err(|e| format!("cannot run powershell: {e}"))?;
        if !output.status.success() {
            return Err(format!(
                "Register-ScheduledTask failed: {}",
                String::from_utf8_lossy(&output.stderr).trim()
            ));
        }
        Ok("✅ Windows service registered (Task Scheduler, starts at logon, no admin needed).\n   Status: Get-ScheduledTask -TaskName Engramd | Select State\n   Logs:   ~/.engram/daemon.log".to_string())
    }

    #[cfg(not(any(target_os = "linux", target_os = "macos", target_os = "windows")))]
    {
        Err("Automatic service installation is not supported on this platform.".to_string())
    }
}

pub async fn handle_capture(
    content_parts: Vec<String>,
    tags: Option<String>,
    layer: String,
    source: String,
    valence: f64,
    project: Option<String>,
    vault_opt: Option<PathBuf>,
) -> Result<()> {
    let content = content_parts.join(" ");
    if content.is_empty() {
        anyhow::bail!("content is required");
    }

    let vp = vault_path(vault_opt);
    let store = EngramStore::open(&vp).await?;
    let layer = EngramLayer::from_str(&layer)
        .ok_or_else(|| anyhow::anyhow!("invalid layer: {layer} (use episodic, semantic, or imagined)"))?;
    let source = EngramSource::from_str(&source)
        .unwrap_or(EngramSource::Interaction);
    let tags: Vec<String> = tags
        .map(|t| t.split(',').map(|s| s.trim().to_string()).collect())
        .unwrap_or_default();

    let mut engram = Engram::new_episodic(content, source, serde_json::json!({}));
    engram.layer = layer;
    engram.tags = tags;
    engram.valence = valence.clamp(-1.0, 1.0);
    if let Some(p) = project {
        engram.project = Some(p);
    }

    match store.write(&engram).await? {
        axiom_engram::WriteOutcome::Inserted => {
            println!("✅ Memory captured: {}", engram.id);
            println!("   {}", &engram.content.chars().take(80).collect::<String>());
        }
        axiom_engram::WriteOutcome::Duplicate { matched_id } => {
            println!("♻ Duplicate of {matched_id} — strengthened instead");
        }
        axiom_engram::WriteOutcome::NoiseSkipped { reason } => {
            println!("⏭ Skipped (noise: {reason})");
        }
    }
    Ok(())
}

pub async fn handle_search(
    query_parts: Vec<String>,
    limit: usize,
    layer_opt: Option<String>,
    vault_opt: Option<PathBuf>,
) -> Result<()> {
    let query = query_parts.join(" ");
    let vp = vault_path(vault_opt);
    let store = EngramStore::open(&vp).await?;

    let results = if let Some(ref layer_str) = layer_opt {
        let layer = EngramLayer::from_str(layer_str)
            .ok_or_else(|| anyhow::anyhow!("invalid layer: {layer_str}"))?;
        store.search_by_layer(layer, limit).await?
    } else if !query.is_empty() {
        store.search_by_content(&query, limit).await?
    } else {
        store.list(limit, 0).await?
    };

    if results.is_empty() {
        println!("No memories found.");
        return Ok(());
    }

    println!("Found {} memories:\n", results.len());
    for (i, m) in results.iter().enumerate() {
        println!(
            "  {}. [{}] {} ({})",
            i + 1,
            m.layer.as_str(),
            m.content.chars().take(100).collect::<String>(),
            m.id,
        );
        if !m.tags.is_empty() {
            println!("     tags: {}", m.tags.join(", "));
        }
        println!();
    }
    Ok(())
}

pub async fn handle_today(vault_opt: Option<PathBuf>) -> Result<()> {
    let vp = vault_path(vault_opt);
    let store = EngramStore::open(&vp).await?;

    let all = store.list(1000, 0).await?;
    let today = chrono::Utc::now().date_naive();
    let todays: Vec<&Engram> = all
        .iter()
        .filter(|e| e.created_at.date_naive() == today)
        .collect();

    if todays.is_empty() {
        println!("No memories captured today.");
        println!("Try: engram capture \"something I learned today\"");
        return Ok(());
    }

    println!("Today's memories ({})\n", todays.len());
    for m in todays.iter() {
        let time = m.created_at.format("%H:%M");
        println!(
            "  {} [{:>5}] {} {}",
            time,
            m.layer.as_str(),
            m.content.chars().take(90).collect::<String>(),
            if m.content.len() > 90 { "…" } else { "" },
        );
        if !m.tags.is_empty() {
            println!("         tags: {}", m.tags.join(", "));
        }
    }
    Ok(())
}

pub async fn handle_eco(vault_opt: Option<PathBuf>) -> Result<()> {
    let vp = vault_path(vault_opt);
    let store = EngramStore::open(&vp).await?;

    let total = store.count().await? as u64;
    let memories = store.list(10_000, 0).await?;
    let total_chars: usize = memories.iter().map(|e| e.content.len()).sum();
    let estimated_tokens_saved = total_chars / 4; // ~4 chars/token
    let estimated_kg_co2 = (estimated_tokens_saved as f64 / 1000.0) * 0.0004; // 0.4g CO2e per 1K tokens

    println!();
    println!("  🌱 Engram Environmental Impact");
    println!("  ═══════════════════════════════");
    println!("  Total memories:     {:>8}", total);
    println!("  Total content:      {:>8} chars", total_chars);
    println!("  Est. tokens saved:  {:>8} (by avoiding re-generation)", estimated_tokens_saved);
    println!("  Est. CO₂e avoided:  {:>8.2} kg", estimated_kg_co2);
    println!();
    println!("  Every memory saved is a thousand tokens not re-generated.");
    println!("  Forgetting is green. 🌿");
    println!();

    Ok(())
}

/// Fetch and pretty-print the weekly digest from a running daemon. The CLI is
/// a thin client of `GET /digest/weekly` — all computation (and any BYO-key
/// prose call) happens in the daemon.
pub async fn handle_digest(url: String, days: u32, prose: bool) -> Result<()> {
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(60))
        .build()?;
    let mut req_url = format!("{}/digest/weekly?days={}", url.trim_end_matches('/'), days.clamp(1, 90));
    if prose {
        req_url.push_str("&prose=1");
    }
    let resp = match client.get(&req_url).send().await {
        Ok(r) => r,
        Err(e) => anyhow::bail!("could not reach engramd at {url}: {e}"),
    };
    let status = resp.status();
    let body: serde_json::Value = resp.json().await.unwrap_or(serde_json::Value::Null);
    if !status.is_success() {
        let msg = body
            .pointer("/error/message")
            .and_then(|m| m.as_str())
            .unwrap_or("unknown error");
        anyhow::bail!("digest failed ({status}): {msg}");
    }

    let stats = &body["stats"];
    println!();
    println!("  🧠 Engram Weekly Digest");
    println!("  ═══════════════════════");
    let start = body["window_start"].as_str().unwrap_or("?");
    let end = body["window_end"].as_str().unwrap_or("?");
    println!("  Window:   {} → {}", start.chars().take(10).collect::<String>(), end.chars().take(10).collect::<String>());
    println!(
        "  Vault:    {} live memories ({} new, {} reinforced, {} fading)",
        stats["live_total"].as_u64().unwrap_or(0),
        stats["new"].as_u64().unwrap_or(0),
        stats["reinforced"].as_u64().unwrap_or(0),
        stats["fading"].as_u64().unwrap_or(0),
    );
    println!(
        "  Hygiene:  {} quarantined ({} new this week)",
        stats["quarantined"].as_u64().unwrap_or(0),
        stats["quarantined_new"].as_u64().unwrap_or(0),
    );

    if let Some(themes) = body["themes"].as_array() {
        if !themes.is_empty() {
            println!();
            println!("  Themes");
            for t in themes {
                println!(
                    "    • {} ({})",
                    t["label"].as_str().unwrap_or("?"),
                    t["count"].as_u64().unwrap_or(0),
                );
            }
        }
    }

    let section = |name: &str, key: &str| {
        if let Some(items) = body[key].as_array() {
            if !items.is_empty() {
                println!();
                println!("  {name}");
                for m in items {
                    let content: String = m["content"]
                        .as_str()
                        .unwrap_or("")
                        .chars()
                        .take(90)
                        .collect();
                    let tags = m["tags"]
                        .as_array()
                        .map(|t| t.iter().filter_map(|v| v.as_str()).collect::<Vec<_>>().join(", "))
                        .filter(|s| !s.is_empty());
                    match tags {
                        Some(t) => println!("    • {content}\n      [{}] [{}]", m["layer"].as_str().unwrap_or(""), t),
                        None => println!("    • {content}\n      [{}]", m["layer"].as_str().unwrap_or("")),
                    }
                }
            }
        }
    };
    section("New memories", "new_memories");
    section("Reinforced (used this week)", "reinforced");
    section("Fading (revisit these)", "fading");

    if let Some(prose_text) = body["prose"].as_str() {
        println!();
        println!("  Digest");
        for line in prose_text.lines() {
            println!("  {line}");
        }
    } else if prose {
        println!();
        println!("  (no prose returned — is digest.llm configured?)");
    } else if body["llm_configured"].as_bool() == Some(true) {
        println!();
        println!("  Tip: engram digest --prose for an AI-written narrative (uses your BYO key).");
    }
    println!();
    Ok(())
}

pub async fn handle_demo(vault_opt: Option<PathBuf>) -> Result<()> {
    let vp = vault_path(vault_opt);
    let store = EngramStore::open(&vp).await?;

    println!();
    println!("  🧬 Engram Demo — The Memory Lifecycle");
    println!("  ══════════════════════════════════════");
    println!();

    // Dedup: if demo data already exists, skip seeding
    let existing = store.list(1000, 0).await?;
    let demo_sentinel = demo_data().first().map(|d| d.0).unwrap_or("");
    let already_seeded = existing.iter().any(|e| e.content == demo_sentinel);

    if already_seeded {
        let total = store.count().await?;
        println!("  ⚠️  Demo data already exists. Skipping seed.");
        println!();
        println!("  Vault status:");
        println!("    Total:     {}", total);
        println!();
        println!("  To re-run the demo, use a fresh vault:");
        println!("    engram --vault ./demo-vault demo");
        println!();
        println!("  Open the UI:");
        println!("    engram daemon");
        println!("    → http://localhost:8787");
        println!();
        return Ok(());
    }

    // Seed sample memories
    let samples = demo_data();
    let sample_count = samples.len();
    println!("  Seeding {} sample memories...", sample_count);
    for (content, layer, source, tags) in samples {
        let mut engram = Engram::new_episodic(
            content.to_string(),
            EngramSource::from_str(source).unwrap_or(EngramSource::Interaction),
            serde_json::json!({}),
        );
        engram.layer = EngramLayer::from_str(layer).unwrap_or(EngramLayer::Episodic);
        engram.tags = tags.iter().map(|s| s.to_string()).collect();
        // Vary strength to simulate age (deterministic per content)
        let hash: u64 = content.bytes().fold(0u64, |a, b| a.wrapping_mul(31).wrapping_add(b as u64));
        engram.strength = 0.3 + (hash as f64 / u64::MAX as f64) * 0.7;
        store.write(&engram).await?;
    }
    println!("  ✅ Seeded {} memories", sample_count);

    // Count
    let total = store.count().await?;
    let episodic = store.search_by_layer(EngramLayer::Episodic, 100_000).await?.len();
    let semantic = store.search_by_layer(EngramLayer::Semantic, 100_000).await?.len();

    println!();
    println!("  Vault status:");
    println!("    Total:     {}", total);
    println!("    Episodic:  {}", episodic);
    println!("    Semantic:  {}", semantic);
    println!();
    println!("  Open the UI to see your memory graph:");
    println!("    engram daemon");
    println!("    → http://localhost:8787");
    println!();

    Ok(())
}

pub async fn handle_backfill_links(
    vault_opt: Option<PathBuf>,
    max_links: usize,
    min_similarity: f64,
) -> Result<()> {
    let vp = vault_path(vault_opt);
    let store = EngramStore::open(&vp).await?;
    let created = store.backfill_semantic_links(max_links, min_similarity).await?;
    println!("🔗 Backfilled {created} link rows (max {max_links}/memory, min similarity {min_similarity})");
    Ok(())
}

// ── MCP install ─────────────────────────────────────────────────────────────

/// A supported MCP client: where its config lives, and where a fresh config
/// may be created (the client is installed but has no MCP config yet).
struct McpClient {
    label: &'static str,
    config_path: PathBuf,
    config_dir: Option<PathBuf>,
}

/// The MCP clients `engram mcp install` knows about, for this platform.
fn mcp_clients() -> Vec<McpClient> {
    let home = home_dir();
    let mut clients = Vec::new();

    #[cfg(target_os = "macos")]
    {
        let dir = home.join("Library/Application Support/Claude");
        clients.push(McpClient {
            label: "Claude Desktop",
            config_path: dir.join("claude_desktop_config.json"),
            config_dir: Some(dir),
        });
    }
    #[cfg(target_os = "linux")]
    {
        let dir = home.join(".config/Claude");
        clients.push(McpClient {
            label: "Claude Desktop",
            config_path: dir.join("claude_desktop_config.json"),
            config_dir: Some(dir),
        });
    }
    #[cfg(target_os = "windows")]
    {
        let dir = std::env::var_os("APPDATA")
            .map(PathBuf::from)
            .map(|p| p.join("Claude"))
            .unwrap_or_default();
        clients.push(McpClient {
            label: "Claude Desktop",
            config_path: dir.join("claude_desktop_config.json"),
            config_dir: Some(dir),
        });
    }

    let cursor_dir = home.join(".cursor");
    clients.push(McpClient {
        label: "Cursor",
        config_path: cursor_dir.join("mcp.json"),
        config_dir: Some(cursor_dir),
    });
    let windsurf_dir = home.join(".codeium/windsurf");
    clients.push(McpClient {
        label: "Windsurf",
        config_path: windsurf_dir.join("mcp_config.json"),
        config_dir: Some(windsurf_dir),
    });
    clients
}

/// Insert (or replace) the `engram` entry under `mcpServers` in a client's
/// MCP config JSON, preserving every other entry and top-level key.
/// `existing` may be "" for a brand-new config. Pure — no I/O.
fn merge_mcp_server(existing: &str, engramd_url: &str) -> Result<String, String> {
    let mut doc: serde_json::Value = if existing.trim().is_empty() {
        serde_json::json!({})
    } else {
        serde_json::from_str(existing).map_err(|e| format!("config is not valid JSON: {e}"))?
    };
    if !doc.is_object() {
        return Err("config root is not a JSON object".into());
    }
    let servers = doc
        .as_object_mut()
        .expect("checked object above")
        .entry("mcpServers")
        .or_insert_with(|| serde_json::json!({}));
    if !servers.is_object() {
        return Err("`mcpServers` is not a JSON object".into());
    }
    // Node spawn does not auto-append .exe on Windows — name it explicitly.
    let mcp_cmd = if cfg!(windows) { "engramd-mcp.exe" } else { "engramd-mcp" };
    servers["engram"] = serde_json::json!({
        "command": mcp_cmd,
        "args": ["--engramd-url", engramd_url],
    });
    serde_json::to_string_pretty(&doc).map_err(|e| e.to_string())
}

/// The snippet a user pastes into an editor's MCP config manually.
fn manual_mcp_snippet(url: &str) -> String {
    let mcp_cmd = if cfg!(windows) { "engramd-mcp.exe" } else { "engramd-mcp" };
    serde_json::to_string_pretty(&serde_json::json!({
        "mcpServers": {
            "engram": {
                "command": mcp_cmd,
                "args": ["--engramd-url", url],
            }
        }
    }))
    .unwrap_or_default()
}

/// Is a file with this name anywhere on PATH?
fn binary_on_path(name: &str) -> bool {
    std::env::var_os("PATH")
        .map(|paths| std::env::split_paths(&paths).any(|dir| dir.join(name).is_file()))
        .unwrap_or(false)
}

/// Is `engramd-mcp` available on PATH?
fn mcp_binary_on_path() -> bool {
    let name = if cfg!(windows) { "engramd-mcp.exe" } else { "engramd-mcp" };
    binary_on_path(name)
}

/// Is the `claude` CLI available (so `claude mcp add` can run)? On Windows
/// the CLI is a `claude.cmd` shim, not `claude.exe`.
fn claude_code_on_path() -> bool {
    binary_on_path("claude") || (cfg!(windows) && binary_on_path("claude.cmd"))
}

/// Is the daemon answering /health at `url`? (2s timeout — install-time
/// probe, not a benchmark.)
async fn daemon_reachable(url: &str) -> bool {
    let Ok(client) = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(2))
        .build()
    else {
        return false;
    };
    client
        .get(format!("{}/health", url.trim_end_matches('/')))
        .send()
        .await
        .map(|r| r.status().is_success())
        .unwrap_or(false)
}

/// Does this config file already carry an `engram` MCP entry?
fn mcp_config_has_engram(path: &PathBuf) -> bool {
    std::fs::read_to_string(path)
        .ok()
        .and_then(|data| serde_json::from_str::<serde_json::Value>(&data).ok())
        .and_then(|doc| doc.get("mcpServers").and_then(|s| s.get("engram")).cloned())
        .is_some()
}

/// One planned `engram mcp install` step. The plan is built without
/// writing or running anything, so it can be printed for `--dry-run`,
/// confirmed step by step, and unit-tested.
#[derive(Debug, PartialEq)]
enum McpAction {
    /// Write (or create) an editor's MCP config file.
    WriteConfig {
        label: String,
        path: PathBuf,
        content: String,
        creates: bool,
    },
    /// Run `claude mcp add` to attach the server to Claude Code.
    RunClaudeCode { url: String },
    /// The editor (or the `claude` CLI) isn't installed — show how to
    /// configure it manually.
    NotDetected { label: String, hint: String },
    /// An existing config could not be merged — leave it untouched.
    WarnMergeFailed { label: String, error: String },
    /// `engramd-mcp` is not on PATH.
    WarnMissingBinary,
    /// The daemon isn't answering /health at the configured URL.
    WarnDaemonUnreachable { url: String },
}

/// Build the full install plan for this machine: warnings, per-editor
/// config writes, the Claude Code step, and manual snippets for editors
/// that aren't installed. Reads existing configs (to merge, never clobber)
/// but writes nothing.
fn mcp_install_plan(
    clients: &[McpClient],
    mcp_binary: bool,
    claude: bool,
    daemon_up: bool,
    url: &str,
) -> Vec<McpAction> {
    let mut plan = Vec::new();
    if !mcp_binary {
        plan.push(McpAction::WarnMissingBinary);
    }
    if !daemon_up {
        plan.push(McpAction::WarnDaemonUnreachable { url: url.to_string() });
    }
    for client in clients {
        if client.config_path.exists() {
            // Merge into the existing config — never clobber other
            // servers the user has configured.
            let existing = std::fs::read_to_string(&client.config_path).unwrap_or_default();
            match merge_mcp_server(&existing, url) {
                Ok(merged) => plan.push(McpAction::WriteConfig {
                    label: client.label.to_string(),
                    path: client.config_path.clone(),
                    content: merged,
                    creates: false,
                }),
                Err(e) => plan.push(McpAction::WarnMergeFailed {
                    label: client.label.to_string(),
                    error: e,
                }),
            }
        } else if client.config_dir.as_ref().map(|d| d.exists()).unwrap_or(false) {
            // The app is installed but has no MCP config yet — create a
            // fresh one rather than making the user hand-write JSON.
            match merge_mcp_server("", url) {
                Ok(merged) => plan.push(McpAction::WriteConfig {
                    label: client.label.to_string(),
                    path: client.config_path.clone(),
                    content: merged,
                    creates: true,
                }),
                Err(e) => plan.push(McpAction::WarnMergeFailed {
                    label: client.label.to_string(),
                    error: e,
                }),
            }
        } else {
            plan.push(McpAction::NotDetected {
                label: client.label.to_string(),
                hint: manual_mcp_snippet(url),
            });
        }
    }
    if claude {
        plan.push(McpAction::RunClaudeCode { url: url.to_string() });
    } else {
        plan.push(McpAction::NotDetected {
            label: "Claude Code".to_string(),
            hint: claude_add_cmdline(url),
        });
    }
    plan
}

/// The program + args that attach the engram MCP server to Claude Code.
/// `--scope user` because Claude Code manages its own config — never
/// hand-edited. On Windows `claude` is a `.cmd` shim, which CreateProcess
/// can't run directly — go through `cmd /C`.
fn claude_add_command(url: &str) -> (String, Vec<String>) {
    #[cfg(windows)]
    {
        (
            "cmd".to_string(),
            vec![
                "/C".to_string(),
                "claude".to_string(),
                "mcp".to_string(),
                "add".to_string(),
                "--scope".to_string(),
                "user".to_string(),
                "engram".to_string(),
                "--".to_string(),
                "engramd-mcp".to_string(),
                "--engramd-url".to_string(),
                url.to_string(),
            ],
        )
    }
    #[cfg(not(windows))]
    {
        (
            "claude".to_string(),
            vec![
                "mcp".to_string(),
                "add".to_string(),
                "--scope".to_string(),
                "user".to_string(),
                "engram".to_string(),
                "--".to_string(),
                "engramd-mcp".to_string(),
                "--engramd-url".to_string(),
                url.to_string(),
            ],
        )
    }
}

/// The same command as a single display string (for hints and fallbacks).
fn claude_add_cmdline(url: &str) -> String {
    let (prog, args) = claude_add_command(url);
    format!("{prog} {}", args.join(" "))
}

/// Yes/no parsing for confirmation prompts: empty defaults to yes.
fn parse_yn(line: &str) -> bool {
    matches!(line.trim().to_lowercase().as_str(), "" | "y" | "yes")
}

/// Prompt on stdout and read one line of stdin. Non-TTY stdin counts as
/// "no" — a piped run should never be prompted into writing.
fn confirm_yn(prompt: &str) -> bool {
    if !std::io::stdin().is_terminal() {
        return false;
    }
    print!("{prompt} ");
    let _ = std::io::stdout().flush();
    let mut line = String::new();
    if std::io::stdin().read_line(&mut line).is_err() {
        return false;
    }
    parse_yn(&line)
}

/// Human-readable plan summary — shared by `--dry-run` and the non-TTY
/// guard, so both show exactly what would happen.
fn print_plan_summary(plan: &[McpAction]) {
    for action in plan {
        match action {
            McpAction::WriteConfig { label, path, creates, .. } => {
                println!(
                    "  write  {label} — {} {}",
                    if *creates { "create" } else { "update" },
                    path.display()
                );
            }
            McpAction::RunClaudeCode { url } => {
                println!("  run    Claude Code — {}", claude_add_cmdline(url));
            }
            McpAction::NotDetected { label, hint } => {
                println!("  manual {label} — not detected; to configure manually:");
                println!("{hint}");
                println!();
            }
            McpAction::WarnMergeFailed { label, error } => {
                println!("  skip   {label} — {error} (left untouched)");
            }
            McpAction::WarnMissingBinary => {
                println!("  warn   `engramd-mcp` is not on your PATH.");
                println!("         Install it first: cargo install --path crates/engramd-mcp");
            }
            McpAction::WarnDaemonUnreachable { url } => {
                println!(
                    "  warn   engramd is not reachable at {url} — MCP tools will fail until it is."
                );
                println!("         Start it with: engram daemon");
            }
        }
    }
}

/// Execute an install plan. `--dry-run` prints it and exits; a piped
/// non-TTY without `--yes` prints it and refuses (exit 1 — nothing is
/// ever written silently); an interactive session confirms each write and
/// the Claude Code step one at a time, default yes.
fn apply_mcp_plan(plan: &[McpAction], dry_run: bool, yes: bool) -> Result<()> {
    if dry_run {
        println!("engram mcp install — plan (nothing was written):");
        print_plan_summary(plan);
        println!();
        println!("Nothing was written. Re-run without --dry-run to install.");
        return Ok(());
    }
    if !std::io::stdin().is_terminal() && !yes {
        println!("engram mcp install would:");
        print_plan_summary(plan);
        println!();
        println!("stdin is not interactive and --yes was not given — nothing was written.");
        println!("Re-run with: engram mcp install --yes");
        std::process::exit(1);
    }
    let mut applied = false;
    for action in plan {
        match action {
            McpAction::WriteConfig { label, path, content, creates } => {
                let ok = yes
                    || confirm_yn(&format!(
                        "[{label}] Write this config to {}? [Y/n]:",
                        path.display()
                    ));
                if ok {
                    std::fs::write(path, content)?;
                    println!(
                        "✅ {label} — {} {}",
                        if *creates { "created" } else { "updated" },
                        path.display()
                    );
                    applied = true;
                } else {
                    println!("  skipped {label}");
                }
            }
            McpAction::RunClaudeCode { url } => {
                let (prog, args) = claude_add_command(url);
                let ok = yes
                    || confirm_yn("[Claude Code] Add the engram MCP server to Claude Code? [Y/n]:");
                if ok {
                    match std::process::Command::new(&prog).args(&args).status() {
                        Ok(s) if s.success() => {
                            println!("✅ Claude Code — engram MCP server added");
                            applied = true;
                        }
                        _ => {
                            // Never claim success on failure — fall back to
                            // the exact command so the user can run it.
                            println!(
                                "⚠️  Claude Code — `{prog}` did not complete successfully; run it manually:"
                            );
                            println!("     {}", claude_add_cmdline(url));
                            println!("     Or add this entry to your editor's MCP config:");
                            println!("{}", manual_mcp_snippet(url));
                        }
                    }
                } else {
                    println!("  skipped Claude Code");
                }
            }
            McpAction::NotDetected { label, hint } => {
                println!("ℹ️  {label} — not detected; to configure it manually:");
                println!("{hint}");
                println!();
            }
            McpAction::WarnMergeFailed { label, error } => {
                println!("⚠️  {label} — {error}; left the existing config untouched");
            }
            McpAction::WarnMissingBinary => {
                println!("⚠️  `engramd-mcp` is not on your PATH.");
                println!("   Install it first:");
                println!("     cargo install --path crates/engramd-mcp   # from the engram repo");
                println!();
            }
            McpAction::WarnDaemonUnreachable { url } => {
                println!("⚠️  engramd is not reachable at {url} — MCP tools will fail until it is.");
                println!("   Start it with: engram daemon");
                println!();
            }
        }
    }
    if applied {
        println!();
        println!("Restart the editor after installing — MCP servers load at startup.");
    }
    println!("Docs: docs/engram-product/MCP.md");
    Ok(())
}

/// `engram mcp install` — write an `engram` entry into the MCP configs of
/// the supported clients installed on this machine (Claude Desktop, Cursor,
/// Windsurf), merging with any servers already configured, and run
/// `claude mcp add` for Claude Code. Every write is confirmed first
/// (skipped with `--yes`, previewed with `--dry-run`); clients that
/// aren't installed get the exact snippet to paste.
///
/// `engram mcp status` — report binary, daemon, and per-client state.
pub async fn handle_mcp(command: String, engramd_url: String, dry_run: bool, yes: bool) -> Result<()> {
    match command.as_str() {
        "install" => {
            let clients = mcp_clients();
            let plan = mcp_install_plan(
                &clients,
                mcp_binary_on_path(),
                claude_code_on_path(),
                daemon_reachable(&engramd_url).await,
                &engramd_url,
            );
            apply_mcp_plan(&plan, dry_run, yes)
        }
        "status" => {
            let binary = if mcp_binary_on_path() { "on PATH ✅" } else { "NOT on PATH ⚠️" };
            let daemon = if daemon_reachable(&engramd_url).await {
                format!("{engramd_url} ✅")
            } else {
                format!("{engramd_url} unreachable ⚠️")
            };
            let claude = claude_code_on_path();
            println!("MCP server status:");
            println!("  Command:  engramd-mcp --engramd-url {engramd_url}");
            println!("  Binary:   {binary}");
            println!("  Daemon:   {daemon}");
            println!("  Tools:    6 (engram_search, engram_capture, engram_get, engram_context, engram_health, engram_decay)");
            println!("  Transport: stdio");
            println!();
            let mut any = claude;
            for client in mcp_clients() {
                let state = if client.config_path.exists() {
                    if mcp_config_has_engram(&client.config_path) {
                        "configured ✅"
                    } else {
                        "present but no engram entry"
                    }
                } else {
                    "not detected"
                };
                println!("  {:<16} {}", client.label, state);
                any |= client.config_path.exists();
            }
            println!(
                "  {:<16} {}",
                "Claude Code",
                if claude { "on PATH ✅" } else { "not detected" }
            );
            if !any {
                println!();
                println!("No supported editors detected. Run: engram mcp install");
            }
            Ok(())
        }
        other => {
            anyhow::bail!("Unknown MCP command: {other}. Usage: engram mcp [install|status] [--url URL]")
        }
    }
}

/// `engram onboarding` — the consumer 5-minute path: create the vault
/// (passphrase or machine-key), capture the first memory, and start the
/// daemon. Existing vaults skip creation and go straight to capture.
///
/// Unlike `init` + `daemon` run separately, this keeps the passphrase in
/// hand long enough to hand it to the daemon via ENGRAM_PASSPHRASE (env,
/// never argv — argv is visible in `ps`).
pub async fn handle_onboarding(bind: String) -> Result<()> {
    println!();
    println!("  ╔══════════════════════════════════════════════╗");
    println!("  ║   Engram — 5-minute onboarding               ║");
    println!("  ║   Your AI deserves a memory.                 ║");
    println!("  ╚══════════════════════════════════════════════╝");
    println!();

    let vault_path = home_dir().join(".engram").join("vault");
    let fresh = !vault_path.join("engrams.db").exists();

    // ── Step 1: vault ──────────────────────────────────────────────────
    let mut passphrase = String::new();
    if fresh {
        std::fs::create_dir_all(&vault_path)?;
        println!("Encryption passphrase (leave empty for a machine-keyed vault):");
        print!("> ");
        std::io::stdout().flush()?;
        std::io::stdin().read_line(&mut passphrase)?;
        passphrase = passphrase.trim().to_string();

        if !passphrase.is_empty() {
            if std::io::stdin().is_terminal() {
                println!("Confirm passphrase:");
                print!("> ");
                std::io::stdout().flush()?;
                let mut confirm = String::new();
                std::io::stdin().read_line(&mut confirm)?;
                if confirm.trim() != passphrase {
                    anyhow::bail!("Passphrases do not match. Run `engram onboarding` again.");
                }
            }
            if passphrase.len() < 8 {
                println!("  ⚠️  Short passphrase (< 8 chars) — a password manager is safer.");
            }
        }
        let _store = if passphrase.is_empty() {
            EngramStore::open(&vault_path).await?
        } else {
            EngramStore::open_with_passphrase(&vault_path, &passphrase).await?
        };
        println!("  ✅ Vault created at {}", vault_path.display());
    } else {
        println!("Vault found at {} — using it.", vault_path.display());
        // Machine-key open first; a passphrase vault fails that, so ask.
        match EngramStore::open(&vault_path).await {
            Ok(_) => {}
            Err(_) => {
                println!("Passphrase for this vault:");
                print!("> ");
                std::io::stdout().flush()?;
                std::io::stdin().read_line(&mut passphrase)?;
                passphrase = passphrase.trim().to_string();
                EngramStore::open_with_passphrase(&vault_path, &passphrase).await?;
            }
        }
    }

    // ── Step 2: first memory ───────────────────────────────────────────
    let store = if passphrase.is_empty() {
        EngramStore::open(&vault_path).await?
    } else {
        EngramStore::open_with_passphrase(&vault_path, &passphrase).await?
    };
    println!();
    println!("What's one thing your AI should remember about you?");
    print!("> ");
    std::io::stdout().flush()?;
    let mut content = String::new();
    std::io::stdin().read_line(&mut content)?;
    let content = content.trim().to_string();
    let content = if content.is_empty() {
        "I just set up Engram — my AI memory vault.".to_string()
    } else {
        content
    };
    let mut engram = Engram::new_episodic(content, EngramSource::Interaction, serde_json::json!({}));
    engram.layer = EngramLayer::Semantic;
    engram.tags = vec!["onboarding".to_string()];
    match store.write(&engram).await? {
        axiom_engram::WriteOutcome::Inserted => println!("  ✅ First memory stored."),
        axiom_engram::WriteOutcome::Duplicate { matched_id } => {
            println!("  ℹ️  Already remembered (matches {matched_id}) — kept the original.")
        }
        axiom_engram::WriteOutcome::NoiseSkipped { reason } => {
            println!("  ⚠️  Skipped as noise ({reason}) — capture something more specific later.")
        }
    }

    // ── Step 3: daemon ─────────────────────────────────────────────────
    let exe = std::env::current_exe()?;
    let log_path = home_dir().join(".engram").join("daemon.log");
    let logfile = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&log_path)?;
    let mut cmd = std::process::Command::new(&exe);
    cmd.arg("daemon")
        .arg("--vault")
        .arg(&vault_path)
        .arg("--bind")
        .arg(&bind)
        .stdout(logfile.try_clone()?)
        .stderr(logfile);
    if !passphrase.is_empty() {
        cmd.env("ENGRAM_PASSPHRASE", &passphrase);
    }
    cmd.spawn()?;
    println!("  ✅ Daemon starting (logs: {})", log_path.display());

    // Wait for the daemon to answer /health (15s budget)
    let url = format!("http://{bind}");
    let client = reqwest::Client::new();
    let mut up = false;
    for _ in 0..30 {
        up = client
            .get(format!("{url}/health"))
            .send()
            .await
            .map(|r| r.status().is_success())
            .unwrap_or(false);
        if up {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(500)).await;
    }

    // ── Step 4: summary ────────────────────────────────────────────────
    let total = store.count().await?;
    println!();
    if up {
        println!("  ✅ Engram is running at {url}");
        println!("     Open the vault: {url}");
    } else {
        println!("  ⚠️  Daemon didn't answer /health within 15s — check {}", log_path.display());
        println!("     Start it manually: engram daemon --vault {} --bind {bind}", vault_path.display());
    }
    println!("     Memories: {total}");
    println!();
    println!("  Next steps:");
    println!("    1. Connect your AI tools:      engram mcp install");
    println!("    2. Sync across devices:        engram link");
    println!("    3. Revisit the setup anytime:  engram init");
    println!();
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn merge_creates_fresh_config() {
        let merged = merge_mcp_server("", "http://127.0.0.1:8787").unwrap();
        let doc: serde_json::Value = serde_json::from_str(&merged).unwrap();
        let expected = if cfg!(windows) { "engramd-mcp.exe" } else { "engramd-mcp" };
        assert_eq!(doc["mcpServers"]["engram"]["command"], expected);
        assert_eq!(doc["mcpServers"]["engram"]["args"][1], "http://127.0.0.1:8787");
    }

    #[test]
    fn merge_preserves_other_servers_and_keys() {
        let existing = r#"{
            "mcpServers": {"other": {"command": "x", "args": []}},
            "extra": true
        }"#;
        let merged = merge_mcp_server(existing, "http://127.0.0.1:8799").unwrap();
        let doc: serde_json::Value = serde_json::from_str(&merged).unwrap();
        assert_eq!(doc["mcpServers"]["other"]["command"], "x");
        assert_eq!(doc["extra"], true);
        assert_eq!(doc["mcpServers"]["engram"]["args"][1], "http://127.0.0.1:8799");
    }

    #[test]
    fn merge_replaces_existing_engram_entry() {
        let existing = r#"{"mcpServers": {"engram": {"command": "engramd-mcp", "args": ["--engramd-url", "http://old:1"]}}}"#;
        let merged = merge_mcp_server(existing, "http://127.0.0.1:8787").unwrap();
        let doc: serde_json::Value = serde_json::from_str(&merged).unwrap();
        assert_eq!(doc["mcpServers"]["engram"]["args"][1], "http://127.0.0.1:8787");
    }

    #[test]
    fn merge_rejects_bad_input() {
        assert!(merge_mcp_server("{not json", "http://127.0.0.1:8787").is_err());
        assert!(merge_mcp_server("[1,2,3]", "http://127.0.0.1:8787").is_err());
        assert!(merge_mcp_server(r#"{"mcpServers": "oops"}"#, "http://127.0.0.1:8787").is_err());
    }

    #[test]
    fn plan_builder_merges_creates_and_detects() {
        let dir = tempfile::tempdir().unwrap();
        let url = "http://127.0.0.1:8787";

        let existing_dir = dir.path().join("existing");
        std::fs::create_dir_all(&existing_dir).unwrap();
        let existing_path = existing_dir.join("mcp.json");
        let existing_json = r#"{"mcpServers":{"other":{"command":"x","args":[]}}}"#;
        std::fs::write(&existing_path, existing_json).unwrap();

        let fresh_dir = dir.path().join("fresh");
        std::fs::create_dir_all(&fresh_dir).unwrap();
        let fresh_path = fresh_dir.join("mcp.json");

        let absent_dir = dir.path().join("absent"); // never created
        let absent_path = absent_dir.join("mcp.json");

        let clients = vec![
            McpClient {
                label: "Existing",
                config_path: existing_path.clone(),
                config_dir: Some(existing_dir),
            },
            McpClient {
                label: "Fresh",
                config_path: fresh_path.clone(),
                config_dir: Some(fresh_dir),
            },
            McpClient {
                label: "Absent",
                config_path: absent_path,
                config_dir: Some(absent_dir),
            },
        ];
        let plan = mcp_install_plan(&clients, true, true, true, url);

        assert_eq!(
            plan,
            vec![
                McpAction::WriteConfig {
                    label: "Existing".into(),
                    path: existing_path,
                    content: merge_mcp_server(existing_json, url).unwrap(),
                    creates: false,
                },
                McpAction::WriteConfig {
                    label: "Fresh".into(),
                    path: fresh_path,
                    content: merge_mcp_server("", url).unwrap(),
                    creates: true,
                },
                McpAction::NotDetected {
                    label: "Absent".into(),
                    hint: manual_mcp_snippet(url),
                },
                McpAction::RunClaudeCode { url: url.into() },
            ]
        );
    }

    #[test]
    fn plan_builder_warns_and_hints_when_things_are_missing() {
        let dir = tempfile::tempdir().unwrap();
        let url = "http://127.0.0.1:8787";
        let absent_dir = dir.path().join("absent");
        let clients = vec![McpClient {
            label: "Claude Desktop",
            config_path: absent_dir.join("claude_desktop_config.json"),
            config_dir: Some(absent_dir),
        }];
        let plan = mcp_install_plan(&clients, false, false, false, url);

        assert_eq!(plan[0], McpAction::WarnMissingBinary);
        assert_eq!(plan[1], McpAction::WarnDaemonUnreachable { url: url.into() });
        assert!(matches!(
            &plan[2],
            McpAction::NotDetected { label, .. } if label == "Claude Desktop"
        ));
        let McpAction::NotDetected { label, hint } = &plan[3] else {
            panic!("expected Claude Code NotDetected, got {:?}", plan[3]);
        };
        assert_eq!(label, "Claude Code");
        assert!(hint.contains("claude mcp add"), "hint should carry the command: {hint}");
    }

    #[test]
    fn plan_builder_merge_failure_warns() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("broken.json");
        std::fs::write(&path, "{not json").unwrap();
        let clients = vec![McpClient { label: "Broken", config_path: path, config_dir: None }];
        let plan = mcp_install_plan(&clients, true, true, true, "http://127.0.0.1:8787");
        assert!(matches!(
            &plan[0],
            McpAction::WarnMergeFailed { label, .. } if label == "Broken"
        ));
    }

    #[test]
    fn claude_add_command_shape() {
        let (prog, args) = claude_add_command("http://127.0.0.1:8787");
        #[cfg(windows)]
        {
            assert_eq!(prog, "cmd");
            assert_eq!(&args[0..2], &["/C", "claude"]);
        }
        #[cfg(not(windows))]
        {
            assert_eq!(prog, "claude");
        }
        let tail = if cfg!(windows) { &args[2..] } else { &args[..] };
        let tail: Vec<&str> = tail.iter().map(String::as_str).collect();
        assert_eq!(
            tail,
            ["mcp", "add", "--scope", "user", "engram", "--", "engramd-mcp", "--engramd-url", "http://127.0.0.1:8787"]
        );
        assert_eq!(
            claude_add_cmdline("http://127.0.0.1:8787"),
            format!("{prog} {}", args.join(" "))
        );
    }

    #[test]
    fn parse_yn_defaults_to_yes() {
        for yes in ["", "y", "Y", "yes", "YES", " y ", "Yes "] {
            assert!(parse_yn(yes), "{yes:?} should be yes");
        }
        for no in ["n", "no", "q", "yep", "maybe"] {
            assert!(!parse_yn(no), "{no:?} should be no");
        }
    }

    #[test]
    fn vault_site_link_pinned_and_fallback() {
        let dir = tempfile::tempdir().unwrap();
        let site = "https://engram.ellmstack.dev/app";
        // No config.json yet → the unlock picker is the honest fallback.
        assert_eq!(vault_site_link(dir.path(), site), format!("{site}/#/unlock"));
        // A pinned sync.vault_id → the per-vault deep link.
        std::fs::write(dir.path().join("config.json"), r#"{"sync":{"vault_id":"eng_abc123"}}"#).unwrap();
        assert_eq!(vault_site_link(dir.path(), site), format!("{site}/#/vault/eng_abc123"));
        // Trailing slashes on the site are trimmed.
        assert_eq!(vault_site_link(dir.path(), &format!("{site}/")), format!("{site}/#/vault/eng_abc123"));
        // Empty pinned id still falls back.
        std::fs::write(dir.path().join("config.json"), r#"{"sync":{"vault_id":""}}"#).unwrap();
        assert_eq!(vault_site_link(dir.path(), site), format!("{site}/#/unlock"));
    }
}

// ── Demo data ──────────────────────────────────────────────────────────────

fn demo_data() -> Vec<(&'static str, &'static str, &'static str, Vec<&'static str>)> {
    vec![
        ("The Q3 deployment uses PostgreSQL 16 with pgvector 0.7 on Hetzner CCX23", "episodic", "interaction", vec!["deployment", "postgres"]),
        ("Alice reported that the login page returns 500 when the email contains a + sign", "episodic", "chat", vec!["bug", "auth"]),
        ("Refactored the memory backend to use ON CONFLICT DO UPDATE instead of INSERT OR REPLACE", "episodic", "interaction", vec!["refactor", "rust"]),
        ("CI pipeline uses GitHub Actions with a custom Rust build cache, takes 12 min", "semantic", "interaction", vec!["ci", "rust"]),
        ("Claude Code's tool calling loop runs with a max of 50 turns per interaction", "semantic", "research", vec!["claude", "tools"]),
        ("To add a new route in engramd: create handler in routes/, call .route() in router()", "semantic", "interaction", vec!["howto", "rust"]),
        ("The Ebbinghaus forgetting curve shows ~70% loss after 24h without reinforcement", "semantic", "research", vec!["memory", "science"]),
        ("Engram uses XOR-folding holographic codes (32-bit) for O(1) associative lookup", "semantic", "interaction", vec!["architecture", "qem"]),
        ("SQLCipher encryption key is derived from SHA-256 hash of machine-ID + app secret", "semantic", "interaction", vec!["security", "encryption"]),
        ("Timeline scrubber idea: drag slider to filter graph by date range, nodes fade in/out", "imagined", "system", vec!["idea", "ui"]),
        ("Imagine a future where AI agents autonomously negotiate memory sharing contracts", "imagined", "system", vec!["future", "agents"]),
        ("A memory economy where users earn tokens for contributing verified semantic memories", "imagined", "system", vec!["idea", "economy"]),
        ("Rust's async traits are stabilizing — we can simplify MemoryBackend significantly", "semantic", "research", vec!["rust", "async"]),
        ("The consolidation run at 3am merged 23 episodic memories into 2 semantic rules", "episodic", "consolidation", vec!["consolidation", "auto"]),
        ("Added health-check timer to systemd: engramd-healthcheck.timer runs every 60s", "semantic", "interaction", vec!["ops", "systemd"]),
        ("Bob asked about adding multi-tenancy to the sync server for team vaults", "episodic", "chat", vec!["feature-request", "sync"]),
        ("The novelty filter window is 100 entries — codes repeat → low surprise → skip capture", "semantic", "interaction", vec!["qem", "filter"]),
        ("MemoryEntry unifies QemEntry, Episode, and Engram into one 20-field struct", "semantic", "interaction", vec!["architecture", "refactor"]),
        ("engramd serves the REST API on port 8787, Caddy proxies to it from :443", "semantic", "interaction", vec!["ops", "deployment"]),
        ("Today's daily hygiene strengthened 3 memories (+0.1) and decayed 12 (-0.05 each)", "episodic", "consolidation", vec!["hygiene", "decay"]),
        ("Hex color #E7150D1C for episodic layer salt is 'EPISODIC' in hexspeak", "semantic", "research", vec!["qem", "fun"]),
        ("The Founder tier ($199/lifetime) sold out its first 50 seats in 8 hours", "episodic", "system", vec!["business", "launch"]),
        ("Zero-Knowledge proofs could let users prove memories are real without revealing content", "imagined", "system", vec!["idea", "crypto"]),
        ("kernel panic on startup if machine-id is empty string — needs fallback to random key", "episodic", "system", vec!["bug", "kernel"]),
        ("Seasonal affective pattern detected: creativity scores dip 18% in winter months", "semantic", "consolidation", vec!["pattern", "analytics"]),
    ]
}

// ── Show passphrase (migration helper) ──────────────────────────────────────

/// Print the vault passphrase read from an env file. The daemon needs
/// ENGRAM_PASSPHRASE on every restart; this file is its only home, so the
/// migration flow ("link this server's vault") reads it back from here.
pub fn handle_show_passphrase(env_file: Option<PathBuf>) -> Result<()> {
    let path = env_file.unwrap_or_else(|| config_dir().join("env"));
    let content = std::fs::read_to_string(&path)
        .with_context(|| format!("reading env file {}", path.display()))?;
    let passphrase = content
        .lines()
        .find_map(|line| line.strip_prefix("ENGRAM_PASSPHRASE="))
        .map(str::trim)
        .map(|v| v.trim_matches('"').trim_matches('\''))
        .filter(|v| !v.is_empty())
        .with_context(|| format!("no ENGRAM_PASSPHRASE= line in {}", path.display()))?;
    println!("Vault passphrase from {}:", path.display());
    println!("{passphrase}");
    eprintln!("Treat this as a secret: save it, never share it, never commit it.");
    Ok(())
}

/// Mint a one-time vault-key handoff: the browser pulls the sync keys from
/// THIS machine's daemon and wraps them under the signed-in account key —
/// no passphrase typing, no passphrase display. Single-use, 300s TTL.
pub async fn handle_handoff(vault: Option<PathBuf>, bind: String, site: String) -> Result<()> {
    let vault_path = vault.unwrap_or_else(|| home_dir().join(".engram").join("vault"));
    if !vault_path.join("engrams.db").exists() {
        anyhow::bail!(
            "no vault at {} — pair this machine first (`engram pair`)",
            vault_path.display()
        );
    }
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(10))
        .build()?;
    let resp = client
        .post(format!("http://{bind}/sync/key-handoff/start"))
        .send()
        .await
        .with_context(|| format!("cannot reach the daemon at {bind} — is engramd running?"))?;
    let status = resp.status();
    let body: serde_json::Value = resp.json().await.unwrap_or(serde_json::json!({}));
    if !status.is_success() {
        let msg = body
            .get("error")
            .and_then(|v| v.as_str())
            .unwrap_or("unknown error");
        anyhow::bail!("handoff refused ({status}): {msg}");
    }
    let token = body
        .get("token")
        .and_then(|v| v.as_str())
        .ok_or_else(|| anyhow::anyhow!("handoff response missing token"))?;
    println!();
    println!("  Vault keys ready for a one-time handoff (expires in 15 minutes).");
    println!();
    println!("  Open this link in the browser where you're signed in to your");
    println!("  Engram account:");
    println!();
    println!(
        "    {}/#/handoff/{}?daemon={}",
        site.trim_end_matches('/'),
        token,
        bind
    );
    println!();
    println!("  The site pulls the vault keys from this machine's daemon and wraps");
    println!("  them under your account key — the vault then opens with your");
    println!("  account password. Your vault passphrase is never typed or shown.");
    println!();
    Ok(())
}

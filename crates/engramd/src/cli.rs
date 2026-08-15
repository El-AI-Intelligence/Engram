//! CLI command handlers for the `engram` binary.
//!
//! When invoked as `engram` (or with subcommands), the binary dispatches here
//! instead of starting the daemon. Each handler opens the vault, performs its
//! operation, prints results, and exits.

use anyhow::Result;
use axiom_engram::{EngramStore, EngramLayer, EngramSource, Engram};
use clap::Subcommand;
use std::path::PathBuf;

// ── CLI definition ─────────────────────────────────────────────────────────

#[derive(Subcommand, Debug)]
pub enum Commands {
    /// Start the engramd daemon (same as running `engramd` directly)
    Daemon {
        /// Path to the vault directory
        #[arg(short, long, default_value = "./engram-data")]
        vault: PathBuf,
        /// Listen address
        #[arg(short, long, default_value = "127.0.0.1:8787")]
        bind: String,
        /// Passphrase for vault encryption
        #[arg(short, long)]
        passphrase: Option<String>,
        /// Path to static UI files to serve
        #[arg(long, env = "ENGRAM_UI_DIR")]
        ui_dir: Option<PathBuf>,
    },
    /// Interactive setup wizard — configure your vault
    Init,
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
    /// Install MCP server config for AI editors (Claude Desktop, Cursor, Continue)
    Mcp {
        /// "install" or "status"
        #[arg(default_value = "status")]
        command: String,
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

    #[cfg(not(any(target_os = "linux", target_os = "macos")))]
    {
        Err("Automatic service installation is only supported on Linux and macOS.".to_string())
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

pub async fn handle_mcp(command: String) -> Result<()> {
    match command.as_str() {
        "install" => {
            let home = home_dir();
            let mut configured = Vec::new();

            // Claude Desktop
            let claude_config = home.join("Library/Application Support/Claude/claude_desktop_config.json");
            if claude_config.exists() {
                configured.push("Claude Desktop".to_string());
            }
            // Check Linux path too
            let claude_linux = home.join(".config/Claude/claude_desktop_config.json");
            if claude_linux.exists() {
                configured.push("Claude Desktop (Linux)".to_string());
            }

            // Cursor
            let cursor_config = home.join(".cursor/mcp.json");
            if cursor_config.exists() {
                configured.push("Cursor".to_string());
            }

            // Continue
            let continue_config = home.join(".continue/config.json");
            if continue_config.exists() {
                configured.push("Continue".to_string());
            }

            if configured.is_empty() {
                println!("No supported AI editors detected.");
                println!();
                println!("Manual MCP config — add to your editor's MCP config file:");
                println!();
                println!(r#"{{"engram": {{"command": "engramd-mcp", "args": ["--engramd-url", "http://127.0.0.1:8787"]}}}}"#);
                println!();
            } else {
                println!("Configured MCP for: {}", configured.join(", "));
                println!("MCP server command: engramd-mcp");
            }
        }
        _ => {
            println!("MCP server status:");
            println!("  Command:  engramd-mcp --engramd-url http://127.0.0.1:8787");
            println!("  Tools:    6 (engram_search, engram_capture, engram_get, engram_context, engram_health, engram_decay)");
            println!("  Transport: stdio");
            println!();
            println!("To install: engram mcp install");
        }
    }
    Ok(())
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

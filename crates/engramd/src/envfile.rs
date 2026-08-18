//! KEY=VALUE env-file loading with systemd `EnvironmentFile=` semantics.
//!
//! Lets a service manager (systemd / launchd / Task Scheduler) hand the
//! daemon secrets like `ENGRAM_PASSPHRASE` via a file the user controls,
//! instead of putting them on a command line. The file only fills gaps:
//! variables already present in the real environment are never overridden.

use std::path::Path;
use tracing::{info, warn};

/// Parse KEY=VALUE lines. Returns only well-formed pairs; malformed
/// non-comment lines are logged **without their content** (they may hold
/// secrets) and skipped.
pub fn parse_env_file(text: &str) -> Vec<(String, String)> {
    let mut out = Vec::new();
    for (i, raw) in text.lines().enumerate() {
        let line = raw.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        match line.split_once('=') {
            Some((k, v)) if !k.trim().is_empty() => {
                let key = k.trim().to_string();
                let mut val = v.trim();
                // Strip one layer of matching quotes.
                if let Some(first) = val.chars().next() {
                    if (first == '"' || first == '\'') && val.ends_with(first) && val.chars().count() >= 2 {
                        val = &val[first.len_utf8()..val.len() - first.len_utf8()];
                    }
                }
                out.push((key, val.to_string()));
            }
            _ => warn!(line = i + 1, "malformed env-file line (skipped)"),
        }
    }
    out
}

/// Load KEY=VALUE pairs from `path`, setting each key only when it is not
/// already present in the real environment. Returns the number of vars
/// applied. Never logs values.
pub fn load_env_file(path: &Path) -> anyhow::Result<usize> {
    let text = std::fs::read_to_string(path)
        .map_err(|e| anyhow::anyhow!("cannot read env file {}: {e}", path.display()))?;
    let mut applied = 0usize;
    for (key, value) in parse_env_file(&text) {
        if std::env::var_os(&key).is_none() {
            std::env::set_var(key, value);
            applied += 1;
        }
    }
    info!(path = %path.display(), applied, "loaded env file");
    Ok(applied)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_key_value_lines() {
        let pairs = parse_env_file(
            "# comment\n\
             \n\
             ENGRAM_PASSPHRASE=correct horse battery staple\n\
             FOO = bar \n\
             KEEP=inline=equals\n\
             \"QUOTED\"=\"a b c\"\n\
             'SINGLE'='x y'\n\
             UNTERMINATED=\"oops\n",
        );
        let map: std::collections::HashMap<_, _> = pairs.into_iter().collect();
        assert_eq!(map.get("ENGRAM_PASSPHRASE").map(String::as_str), Some("correct horse battery staple"));
        assert_eq!(map.get("FOO").map(String::as_str), Some("bar"));
        assert_eq!(map.get("KEEP").map(String::as_str), Some("inline=equals"));
        assert_eq!(map.get("\"QUOTED\"").map(String::as_str), Some("a b c"));
        assert_eq!(map.get("'SINGLE'").map(String::as_str), Some("x y"));
        assert_eq!(map.get("UNTERMINATED").map(String::as_str), Some("\"oops"));
    }

    #[test]
    fn skips_malformed_lines_without_values() {
        let pairs = parse_env_file("NOKEY\n = novalue\nGOOD=1\n");
        assert_eq!(pairs.len(), 1);
        assert_eq!(pairs[0], ("GOOD".to_string(), "1".to_string()));
    }

    #[test]
    fn load_fills_gaps_only() {
        // Use unique keys so parallel tests can't collide.
        let gap_key = "ENGRAM_TEST_ENVFILE_GAP_ONLY";
        let set_key = "ENGRAM_TEST_ENVFILE_ALREADY_SET";
        std::env::remove_var(gap_key);
        std::env::remove_var(set_key);

        let dir = std::env::temp_dir().join(format!("engram-envfile-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("env");
        std::fs::write(&path, format!("{gap_key}=fromfile\n{set_key}=fromfile\n")).unwrap();

        // Pre-set one key: it must survive the load untouched.
        std::env::set_var(set_key, "fromreality");

        let applied = load_env_file(&path).unwrap();
        assert_eq!(applied, 1);
        assert_eq!(std::env::var(gap_key).unwrap(), "fromfile");
        assert_eq!(std::env::var(set_key).unwrap(), "fromreality");

        std::env::remove_var(gap_key);
        std::env::remove_var(set_key);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn load_missing_file_bails() {
        let path = std::path::Path::new("/nonexistent/engram-env-file-test");
        let err = load_env_file(path).unwrap_err();
        assert!(err.to_string().contains("cannot read env file"));
        assert!(!err.to_string().contains("SECRET"));
    }
}

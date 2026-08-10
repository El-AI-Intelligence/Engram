# Engram Memory Vault — npm wrapper

`npx engramd` — the Engram CLI via npm. Downloads the right platform binary
from GitHub Releases on install.

## Install

```bash
npm install -g engramd
```

This downloads the `engram` and `engramd` binaries for your platform.

## Usage

```bash
engram init           # Interactive setup
engram capture "..."  # Capture a memory
engram search "query" # Search your vault
engram daemon         # Start the vault server
engram today          # Today's memories
engram eco            # Environmental impact
engram demo           # Seed sample memories
```

## Supported platforms

| Platform | Arch | Status |
|----------|------|--------|
| macOS | x86_64 | ✅ |
| macOS | arm64 (Apple Silicon) | ✅ |
| Linux | x86_64 | ✅ |
| Linux | arm64 | ✅ |

## Alternative install methods

```bash
cargo install engramd    # Rust (crates.io)
brew install engramd      # macOS (Homebrew)
docker pull ghcr.io/pixelphantomai/engramd  # Docker
```

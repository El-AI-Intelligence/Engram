# Installing Engram

## One-command install

```bash
curl -fsSL https://engram.ellmstack.dev/install.sh | bash
```

This installs `engram` and `engramd` to `~/.local/bin`.

## Alternative methods

### Homebrew (macOS/Linux)
```bash
brew tap pixelphantomai/tap
brew install engramd
```

### cargo (Rust)
```bash
cargo install engramd
```

### Docker
```bash
docker run -v ./vault:/vault -p 8787:8787 ghcr.io/pixelphantomai/engramd:latest
```

## Post-install

1. **Initialize your vault** (interactive wizard):
   ```bash
   engram init
   ```
   This creates your encrypted vault and optionally installs engramd as a
   background service (systemd on Linux, launchd on macOS).

2. **Start the daemon**:
   ```bash
   engram daemon
   ```
   Opens the API at `http://localhost:8787`.

3. **Open the dashboard**:
   Visit `http://localhost:8787` in your browser.

## AI tool integration

### Claude Code
```bash
engram-inject --cursor   # writes .cursorrules
```

### Cursor
```bash
engram-inject --cursor
```

### Windsurf
```bash
engram-inject --windsurf
```

### aider
```bash
engram-inject --aider
```

## Updating

### Homebrew
```bash
brew upgrade engramd
```

### cargo
```bash
cargo install --force engramd
```

### Shell script
```bash
curl -fsSL https://engram.ellmstack.dev/install.sh | bash
```

## Uninstalling

```bash
# Stop the service
systemctl --user stop engramd    # Linux
launchctl unload ~/Library/LaunchAgents/com.ellmstack.engramd.plist  # macOS

# Remove binaries
rm ~/.local/bin/engram ~/.local/bin/engramd

# Remove data (careful!)
rm -rf ~/.engram
```

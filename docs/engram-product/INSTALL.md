# Installing Engram by El AI Intelligence

## One-command install

```bash
curl -fsSL https://engram.ellmstack.dev/install.sh | bash
```

This installs `engram` and `engramd` to `~/.local/bin`.

## Windows (PowerShell)

```powershell
iex (irm https://engram.ellmstack.dev/install.ps1)
```

Installs to `~\.local\bin` (no admin rights), verifies the SHA-256 checksum,
and adds the directory to your user PATH. Then:

```powershell
engram init              # create your vault — answer Y to install the service
engram link              # link this machine to your account (browser, one click)
```

The daemon runs as a **Task Scheduler task** (`Engramd`) that starts at logon
and restarts itself if it crashes:

```powershell
Get-ScheduledTask Engramd            # check status
Stop-ScheduledTask Engramd           # stop the daemon
Start-ScheduledTask Engramd          # start it again
Unregister-ScheduledTask Engramd     # remove the service
```

Logs: `~\.engram\daemon.log`. The vault passphrase lives in `~\.engram\env`
(not in the scheduled task or any command line).

> **SmartScreen note:** Windows binaries are unsigned. If SmartScreen blocks
> the download, choose "More info" → "Run anyway".

Building from source on Windows (`cargo install --git
https://github.com/El-AI-Intelligence/engram engramd`) requires MSVC C++
Build Tools, NASM, and Perl — the bundled SQLCipher links a vendored OpenSSL.
The one-liner installer above avoids all of that (everything is compiled in).

## Alternative methods

### Homebrew (macOS/Linux)
```bash
brew tap El-AI-Intelligence/engram
brew install engramd
```

macOS binaries from GitHub Releases are signed and notarized (Apple Developer
ID), so Gatekeeper accepts them. If a binary from an old download ever gets a
quarantine warning, clear it with `xattr -dr com.apple.quarantine <path>`.

### cargo (Rust)
```bash
cargo install engramd
```

### Docker
```bash
# One-time setup: initialize the vault (interactive wizard)
docker run --rm -it -v ./vault:/vault --entrypoint engram \
  ghcr.io/el-ai-intelligence/engramd:latest init

# Run the daemon (passphrase required on every start):
docker run -d -v ./vault:/vault -p 8787:8787 \
  -e ENGRAM_PASSPHRASE=... ghcr.io/el-ai-intelligence/engramd:latest
```

## Post-install

**Fastest path — one command does all three steps below:**

```bash
engram onboarding
```

It creates the vault, captures your first memory, and starts the daemon,
then prints the dashboard URL and next steps (MCP, sync).

**Sync to your Engram by El AI Intelligence account** (multi-device, WARP-style): create an
account on the vault's login screen ("New here? Create an account"), then
run one command — your browser opens, you click once, done:

```bash
engram link
```

The account key arrives sealed to an ephemeral keypair, so nothing secret
ever passes through the confirm URL. Headless/SSH machines can instead mint
a code in Settings → Account & Sync → **Pair a device (headless)** and run
`engram pair ENG-XXXX-XXXX-XXXX`. Full details in [SYNC.md](SYNC.md).

Or step through it manually:

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
   Opens the API at `http://localhost:8787`. With no `--vault`, the
   daemon opens the vault `engram init` created (`~/.engram/vault`), and
   loads the passphrase from `~/.engram/env` (0600) automatically — no
   re-typing. Point elsewhere with `--vault <path>` or `ENGRAM_VAULT`.

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

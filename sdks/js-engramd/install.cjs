#!/usr/bin/env node
// ── Engram binary installer ────────────────────────────────────────────
// Postinstall script that downloads the right platform binary from
// GitHub Releases. Runs automatically after `npm install engramd`.

const fs = require("fs");
const path = require("path");
const https = require("https");
const { execSync } = require("child_process");

const VERSION = require("./package.json").version;
const REPO = "El-AI-Intelligence/engram";
const BIN_DIR = path.join(__dirname, "bin");

// ── Platform detection ─────────────────────────────────────────────────
function getPlatform() {
  const os = process.platform; // "darwin" | "linux" | "win32"
  const arch = process.arch; // "x64" | "arm64"

  if (os === "darwin" && arch === "x64") return "darwin-x86_64";
  if (os === "darwin" && arch === "arm64") return "darwin-arm64";
  if (os === "linux" && arch === "x64") return "linux-x86_64";
  if (os === "linux" && arch === "arm64") return "linux-arm64";
  if (os === "win32" && arch === "x64") return "windows-x86_64";

  console.error(
    `engramd: unsupported platform ${os}-${arch}. ` +
    `Only darwin-x64, darwin-arm64, linux-x64, linux-arm64, windows-x64 are supported.`
  );
  process.exit(1);
}

// ── Download ───────────────────────────────────────────────────────────
function download(url, dest) {
  return new Promise((resolve, reject) => {
    const file = fs.createWriteStream(dest);
    https
      .get(url, (res) => {
        if (res.statusCode === 302 || res.statusCode === 301) {
          // Follow redirect
          https.get(res.headers.location, (redirectRes) => {
            redirectRes.pipe(file);
            file.on("finish", () => {
              file.close();
              resolve();
            });
          }).on("error", reject);
          return;
        }
        if (res.statusCode !== 200) {
          reject(new Error(`HTTP ${res.statusCode} fetching ${url}`));
          return;
        }
        res.pipe(file);
        file.on("finish", () => {
          file.close();
          resolve();
        });
      })
      .on("error", reject);
  });
}

async function main() {
  // Skip download if we're being installed from a local build
  if (process.env.ENGRAM_SKIP_DOWNLOAD) {
    console.log("engramd: skipping binary download (ENGRAM_SKIP_DOWNLOAD set)");
    return;
  }

  // Check if binaries already exist (Windows ships .exe names)
  const binExt = process.platform === "win32" ? ".exe" : "";
  const engramBin = path.join(BIN_DIR, `engram${binExt}`);
  const engramdBin = path.join(BIN_DIR, `engramd${binExt}`);
  if (fs.existsSync(engramBin) && fs.existsSync(engramdBin)) {
    console.log("engramd: binaries already installed");
    return;
  }

  const platform = getPlatform();
  const isWindows = platform.startsWith("windows");
  const archive = `engramd-${platform}.${isWindows ? "zip" : "tar.gz"}`;
  const url = `https://github.com/${REPO}/releases/download/v${VERSION}/${archive}`;

  console.log(`engramd: downloading ${archive}...`);

  // Create bin directory
  fs.mkdirSync(BIN_DIR, { recursive: true });

  const archivePath = path.join(BIN_DIR, archive);

  try {
    await download(url, archivePath);
  } catch (err) {
    console.error(`engramd: failed to download binary: ${err.message}`);
    console.error(`engramd: you can install manually via cargo: cargo install engramd`);
    process.exit(1);
  }

  // Extract (Windows: bsdtar ships with Win10 1803+; Expand-Archive fallback)
  try {
    if (isWindows) {
      try {
        execSync(`tar -xf "${archivePath}" -C "${BIN_DIR}"`, { stdio: "pipe" });
      } catch {
        execSync(`powershell -NoProfile -Command "Expand-Archive -Force '${archivePath}' '${BIN_DIR}'"`, { stdio: "pipe" });
      }
    } else {
      execSync(`tar xzf "${archivePath}" -C "${BIN_DIR}"`, { stdio: "pipe" });
    }
    fs.unlinkSync(archivePath);

    // Make binaries executable (no-op on Windows)
    if (!isWindows) {
      fs.chmodSync(engramBin, 0o755);
      fs.chmodSync(engramdBin, 0o755);
    }

    console.log(`engramd ${VERSION} installed successfully`);
  } catch (err) {
    console.error(`engramd: failed to extract binary: ${err.message}`);
    process.exit(1);
  }
}

main();

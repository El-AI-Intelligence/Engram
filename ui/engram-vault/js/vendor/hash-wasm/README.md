# Vendored: hash-wasm 4.12.0

Source: https://www.npmjs.com/package/hash-wasm · (c) Dani Biro · MIT license (see LICENSE)

Used by `js/unlock.js` for Argon2id key derivation (WebCrypto has no native Argon2).

- `index.esm.js` — the package's ESM entry, **self-contained**: all WASM binaries
  are base64-embedded inside it (no separate `.wasm` files, no imports). Loaded
  directly via `<script type="module">`/`import` — this repo has no bundler.
- `argon2.d.ts`, `index.d.ts` — typings, reference only.
- `LICENSE` — MIT.

Tarball: `hash-wasm-4.12.0.tgz` (npm registry)
sha256: `1db32a125fb46177932ec8ac438d3cd8214ebdfaccb5d6611b657d88eb586f92`

Notes:
- Only `index.esm.js` was copied (not the full `dist/` tree with UMD bundles) —
  verified the ESM entry has zero imports, so nothing else is needed.
- Requires CSP `'wasm-unsafe-eval'` in `script-src` (WebAssembly.instantiate on a
  base64-decoded buffer) — see `deploy/caddy/engram.Caddyfile`.
- API used: `argon2id({password, salt, parallelism, iterations, memorySize, hashLength, outputType})`
  — Argon2 v1.3 (0x13), matching Rust `argon2` crate params in engramd.
- To upgrade: `npm pack hash-wasm@<ver>`, copy the new `dist/index.esm.js` +
  LICENSE here, update version + sha256 above.

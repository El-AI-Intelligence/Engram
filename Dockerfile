# ── Engram Dockerfile ──────────────────────────────────────────────────────
# Multi-stage build for the engramd image; built by the release workflow
# (build-push-action, linux/amd64 + linux/arm64) and pushed to
# ghcr.io/el-ai-intelligence/engramd.
#
#   # One-time setup: initialize the vault (interactive wizard)
#   docker run --rm -it -v ./vault:/vault --entrypoint engram \
#     ghcr.io/el-ai-intelligence/engramd:latest init
#
#   # Run the daemon (passphrase required on every start):
#   docker run -d -v ./vault:/vault -p 8787:8787 \
#     -e ENGRAM_PASSPHRASE=... ghcr.io/el-ai-intelligence/engramd:latest

FROM rust:latest AS build
WORKDIR /app
COPY . .
# bundled-sqlcipher compiles C — the full rust image ships gcc + pkg-config.
RUN cargo build --release -p engramd -p engramd-mcp

FROM debian:bookworm-slim
RUN apt-get update && apt-get install -y --no-install-recommends ca-certificates \
    && rm -rf /var/lib/apt/lists/*
COPY --from=build /app/target/release/engram /usr/local/bin/engram
COPY --from=build /app/target/release/engramd /usr/local/bin/engramd
COPY --from=build /app/target/release/engramd-mcp /usr/local/bin/engramd-mcp
# Container default: listen on all interfaces (the CLI default of
# 127.0.0.1:8787 is unreachable through -p port mapping).
ENV ENGRAM_BIND=0.0.0.0:8787
VOLUME /vault
EXPOSE 8787
ENTRYPOINT ["engramd"]
CMD ["daemon", "--vault", "/vault"]

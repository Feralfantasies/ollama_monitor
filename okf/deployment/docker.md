---
type: Deployment Guide
title: Docker Deployment
description: Docker build, run, and network configuration for ollama_monitor.
tags: [docker, deployment, container]
timestamp: 2026-06-18T00:00:00Z
---

# Docker Deployment

Multi-stage Docker build producing a minimal runtime image.

## Dockerfile

```dockerfile
FROM rust:1-bookworm AS builder
WORKDIR /app
COPY . .
RUN cargo build --release

FROM debian:bookworm-slim
RUN apt-get update && apt-get install -y nvidia-utils && rm -rf /var/lib/apt/lists/*
COPY --from=builder /app/target/release/ollama_monitor /usr/local/bin/
EXPOSE 3000
ENTRYPOINT ["/usr/local/bin/ollama_monitor"]
```

## Run command

```bash
docker run -d \
  --name ollama_monitor \
  --gpus all \
  --network host \
  -e OLLAMA_HOST=http://192.168.1.50 \
  -e REFRESH_INTERVAL_SECS=15 \
  -e RUST_LOG=info \
  ollama_monitor
```

## Configuration notes

- **`--gpus all`** — Required for `nvidia-smi` access inside the container.
- **`--network host`** — Simplest mode. Container shares the host network stack. Ollama URL should use the host's IP if remote.
- Without `--network host`, port mapping is needed and `OLLAMA_HOST` must point to the host machine's IP address.
- SQLite database (`ollama_monitor.db`) is created in the container's working directory. Mount a volume for persistence: `-v /host/path:/data` and set working directory.

## See also

- [Config Module](/src/config.md) — All available environment variables
- [Systemd Deployment](systemd.md) — Alternative direct-host deployment

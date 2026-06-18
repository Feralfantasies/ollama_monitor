# Ollama Monitor — Knowledge Bundle

## Project Overview

* [Overview](overview.md) — What ollama_monitor is and what it does
* [Architecture](architecture.md) — System architecture and data flow

## Source Modules

* [Config Module](src/config.md) — Environment variable configuration with defaults
* [API Module](src/api.md) — Axum REST API server and embedded web dashboard
* [GPU Module](src/gpu.md) — nvidia-smi CLI wrapper and CSV parser
* [Ollama Client Module](src/ollama.md) — Ollama REST API client
* [System Metrics Module](src/system.md) — Linux /proc-based system metric collection
* [Models Module](src/models.md) — Shared data structures (Rust + serde)
* [Tests Module](src/tests.md) — Integration test harness with mock Ollama and mock nvidia-smi

## Persistence

* [Database Schema](db/schema.md) — SQLite schema, migrations, and queries

## API Reference

* [REST API Reference](api/api-reference.md) — HTTP endpoints, request/response schema

## Home Assistant Integration

* [Integration Overview](ha-integration/integration.md) — Custom integration architecture and setup

## Deployment

* [Docker Deployment](deployment/docker.md) — Docker build, run, and network configuration
* [Systemd Deployment](deployment/systemd.md) — Systemd service unit configuration

## Development

* [Testing Strategy](testing/testing.md) — Test matrix, mock infrastructure, and CI checks
* [CI/CD Pipeline](testing/ci-cd.md) — GitHub Actions workflows (CI and release)

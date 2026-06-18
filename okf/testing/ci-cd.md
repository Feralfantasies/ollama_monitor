---
type: CI/CD Document
title: CI/CD Pipeline
description: GitHub Actions workflows for continuous integration and automated releases.
resource: .github/workflows/
tags: [ci, cd, github-actions, release]
timestamp: 2026-06-18T00:00:00Z
---

# CI/CD Pipeline

Two GitHub Actions workflows enforce code quality and automate releases.

## CI workflow (`.github/workflows/ci.yml`)

Triggered on push to any branch and pull requests to `main`. Runs on `firethorn` self-hosted runner.

### Job sequence

| Job | Steps | Dependencies |
|---|---|---|
| **fmt** | `cargo fmt --all -- --check` | None |
| **clippy** | `cargo clippy --all-targets -- -D warnings` | None |
| **build** | `RUSTFLAGS="-D warnings" cargo build` | None |
| **test** | `RUSTFLAGS="-D warnings" cargo test --all-targets` | fmt, clippy |
| **build-musl** | `cargo build --release --target x86_64-unknown-linux-musl` | fmt, clippy |

- **Concurrency:** Cancel in-progress runs on same branch when new push arrives (`cancel-in-progress: true`)
- **Cache:** Shared `rust-cache` per job for dependency reuse
- **Toolchain:** Stable Rust with `rustfmt` and `clippy` components

## Release workflow (`.github/workflows/release.yml`)

Triggered on push to `main` or manual dispatch.

### Job sequence

| Job | Action |
|---|---|
| **create-tag** | Auto-bump semantic version tag (patch by default). Bumps on every push to `main`. |
| **build-binary** | Build musl static binary. Uploads as artifact. |
| **create-release** | Creates GitHub release with changelog + binary download. |

### Artifacts

Each release produces a statically-linked `ollama_monitor` binary (`x86_64-unknown-linux-musl`).

## Pre-commit hook (`hooks/pre-commit`)

```bash
cargo fmt --check
cargo clippy --all-targets -- -D warnings
cargo test --all-targets
```

Must pass for every commit. Installed by copying to `.git/hooks/pre-commit` and making executable.

## See also

- [Testing Strategy](testing.md) — Test matrix and mock infrastructure

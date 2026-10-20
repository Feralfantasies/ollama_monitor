---
type: Source Module
title: llama.cpp Client Module
description: Client for llama-server's OpenAI-compatible API, and why the Ollama client cannot be reused.
resource: src/llamacpp.rs
tags: [rust, llama-cpp, http, openai-compatible, readiness]
timestamp: 2026-10-19T00:00:00Z
---

# llama.cpp Client Module

`src/llamacpp.rs` talks to llama.cpp's `llama-server`. It exists because
**`llama-server` does not implement Ollama's API**.

Verified against a live b11583 server (`system_fingerprint: b11583-988015788`):

| Endpoint | Result |
|---|---|
| `GET /v1/models` | `200` — OpenAI-compatible list |
| `GET /props` | `200` — server/model state |
| `GET /api/tags` | **`404`** — Ollama only |
| `GET /api/ps` | **`404`** — Ollama only |

So pointing [`OllamaClient`](ollama.md) at a llama-server produces a warning every
refresh cycle and no data. The Ollama client is deliberately left untouched so
existing Ollama deployments keep working; selection between the two is automatic
— see [config](config.md) `NODE_<i>_*`.

## The readiness rule (load-bearing)

`llama-server` answers `GET /props` with **`503` while a model is still loading**,
and `200` only once it is serving. Measured load times on the V100 host:

| Model | Size | Load time |
|---|---|---|
| Qwen3.8-27B Q4_K_M | 16.4 GB | 36–66 s (warm/cold) |
| Precog-24B-v1b Q4_K_M | 14.3 GB | 76–148 s |

During that whole window `systemctl is-active` already reports `active`, because
the unit is `Type=simple`. Treating "process is up" or "TCP connects" as ready is
what causes a **CUDA OOM**: the [switcher](switcher.md) would start a second model
before the first released VRAM.

Therefore `LlamaProbe::ready` is `true` **only** on a `200` response. Observed live
during a switch: `props=503` for 110 consecutive seconds while VRAM climbed
311 → 13615 → 24445 MiB, and only then `200`.

## Response-shape quirks handled

Fixtures in `tests/fixtures/` are **real captures** from both live servers
(`llamacpp_props_qwen.json`, `llamacpp_v1models_qwen.json`,
`llamacpp_props_precog.json`, `llamacpp_v1models_precog.json`), so the parser is
tested against reality rather than an imagined schema.

* `/v1/models` returns **two** arrays: an Ollama-compat `models[]` and an OpenAI
  `data[]`. `LlamaModels::ids()` prefers `data[].id`, falls back to
  `models[].name`, and de-duplicates so a server emitting both does not report the
  same model twice.
* `model_alias` appears at the top level of `/props` *and* may appear inside
  `default_generation_settings`. `LlamaProps::alias()` prefers top level.
* `n_ctx` lives inside `default_generation_settings` and is **per slot**, not the
  total. With `--ctx-size 184320 --parallel 3`, `n_ctx` is `61440`. Reporting the
  total would badly misrepresent usable per-session context.
* Every field is `Option`/`#[serde(default)]`: llama.cpp's JSON shifts between
  builds, and an unknown or missing key must not break monitoring.

## Loaded size vs file size

`/v1/models` `meta.size` is the **loaded tensor size**, not the GGUF file size.
For Precog-24B-v1b Q4_K_M: server reports `14,326,026,240`, file on disk is
`14,333,913,536`. A test pins both numbers so nobody "fixes" one to the other.

## Types

| Type | Purpose |
|---|---|
| `LlamaCppClient` | HTTP client; `probe()`, `fetch_models()` |
| `LlamaProbe` | Non-throwing classification: `ready`, `reachable`, `http_status`, `alias`, `n_ctx`, `total_slots`, `available_models`, `error` |
| `LlamaProps` | Parsed `/props` |
| `LlamaModels` / `LlamaModelEntry` / `LlamaModelMeta` | Parsed `/v1/models` |

`probe()` never returns an error — it classifies. That matters because it runs on
the refresh loop, and one unreachable node must not break the whole cycle.

## See also

- [Switcher Module](switcher.md) — consumes readiness to arbitrate VRAM
- [Config Module](config.md) — `NODE_<i>_SERVER_URL` declares these endpoints
- [API Reference](../api/api-reference.md) — `GET /api/nodes` exposes probes

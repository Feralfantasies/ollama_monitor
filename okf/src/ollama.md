---
type: Source Module
title: Ollama Client Module
description: HTTP client for the Ollama REST API.
resource: src/ollama.rs
tags: [rust, http, ollama]
timestamp: 2026-06-18T00:00:00Z
---

# Ollama Client Module

HTTP client that queries the Ollama REST API to discover available models.

## Endpoints used

| Method | Path | Purpose |
|---|---|---|
| `GET` | `{base_url}/api/tags` | List all available models |

## Response parsing

Expects the standard Ollama `/api/tags` JSON response:

```json
{
  "models": [
    { "name": "llama3:8b", "size": 4700000000, "digest": "sha256:...", "modified_at": "..." }
  ]
}
```

## Public API

| Method | Returns | Description |
|---|---|---|
| `new(base_url)` | `OllamaClient` | Create client with given base URL |
| `fetch_models()` | `Result<OllamaTagsResponse>` | Fetch models. Returns error on failure. |
| `try_fetch_models()` | `Option<OllamaTagsResponse>` | Best-effort fetch. Returns `None` on failure. |

## Configuration

- Uses `reqwest::Client` with a 10-second timeout. Rustls TLS backend (no native OpenSSL dependency).

## Error handling

- Connection timeout or network unreachable → error from `fetch_models()`, `None` from `try_fetch_models()`.
- Non-200 response or JSON parse failure → error.

## See also

- [Models Module](models.md) — `OllamaTagsResponse` and `OllamaModelEntry` types

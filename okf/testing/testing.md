---
type: Testing Document
title: Testing Strategy
description: Test matrix, mock infrastructure, and CI verification steps.
tags: [testing, rust, mock, ci]
timestamp: 2026-06-18T00:00:00Z
---

# Testing Strategy

All tests run without a real GPU or running Ollama instance. Two test categories cover the codebase.

## Unit tests

Inline `#[cfg(test)]` modules inside source files.

### Database tests (`src/db.rs`)

| Test | Verifies |
|---|---|
| `test_insert_and_query_check_result` | Insert + query round-trip preserves all fields |
| `test_multiple_check_results_ordered_desc` | Multiple rows return newest-first |
| `test_insert_check_result_with_null_fields` | All-null GPU row round-trip |

Uses per-test SQLite temp pools (single connection each) for full isolation.

### System module tests (`src/system.rs`)

| Test | Verifies |
|---|---|
| `test_proc_stat_parses` | Live `/proc/stat` reads and parses |
| `test_meminfo_parses` | Live `/proc/meminfo` reads and parses |
| `test_query_system_returns_values` | `query_system()` populates at least memory_total and cpu_utilization |
| `test_system_metric_placeholder` | Placeholder returns all `None` |

These require a Linux environment with `/proc` mounted.

## Integration tests (`src/tests.rs`)

Full refresh-loop tests using mock infrastructure.

| Test | Verifies |
|---|---|
| `test_full_pipeline_history_accumulates` | 3 cycles → 3 DB rows, history returns correct points + in-memory state matches |
| `test_history_timestamps_ordered` | 4 cycles with 1s gaps → strictly ascending timestamps |
| `test_unreachable_ollama_records_correctly` | No Ollama → `ollama_reachable: false`, GPU still populated |
| `test_no_gpu_records_nulls` | Placeholder GPU → all GPU fields null, Ollama still recorded |
| `test_mixed_gpu_availability` | Alternating GPU good/placeholder → DB tolerates mixed |
| `test_dashboard_api_endpoints` | Full HTTP API on all endpoints (/api/status, /api/gpu, /api/models, /api/history, /) |

### llama.cpp client tests (`src/llamacpp.rs`)

These parse **real captures** from the two live servers, held in `tests/fixtures/`
(`llamacpp_props_qwen.json`, `llamacpp_v1models_qwen.json`,
`llamacpp_props_precog.json`, `llamacpp_v1models_precog.json`). Testing against
recorded reality rather than a hand-written schema is what caught the
`data[]`/`models[]` duplication and the loaded-size-vs-file-size difference.

| Test | Verifies |
|---|---|
| `parses_real_props_capture` | Qwen `/props`: alias, per-slot `n_ctx` 61440, `total_slots` 3, ftype, path, build |
| `parses_real_v1_models_capture` | Qwen `/v1/models`: both arrays populated, `meta` params/size/ftype |
| `parses_real_precog_props_capture` | Precog `/props`: `total_slots` 2 — a different shape from Qwen, so the parser is not fitted to one deployment |
| `parses_real_precog_v1_models_capture` | Loaded size `14,326,026,240` ≠ on-disk `14,333,913,536`; pins both so neither is "corrected" into the other |
| `ids_are_deduplicated_across_both_arrays` | A model present in both `data[]` and `models[]` is reported once |
| `ids_fall_back_to_compat_array_when_data_absent` | `data[]` missing ⇒ `models[].name` is used |
| `alias_prefers_top_level_then_nested` | `model_alias` resolution order |
| `tolerates_unknown_and_missing_fields` | Unknown keys ignored; `{}` does not panic |
| `probe_defaults_are_not_ready` | A default probe never claims readiness — the OOM-avoidance invariant |
| `probe_of_unreachable_endpoint_is_not_ready` | Dead endpoint classifies as unreachable, no panic, no hang |
| `two_servers_are_distinguishable` | Aliases/paths/slots differ, but per-session context is equal by design |
| `live_probe_against_real_server` | **`#[ignore]`d.** Runs against `LIVE_LLAMA_URL`; executed manually against both `.50` and `.53` |

### Switcher tests (`src/switcher.rs`)

The ordering logic is a **pure state machine** (`SwitchPlan::next(&WorldSnapshot)`)
with no I/O and no clocks, so the safety invariant is asserted over deterministic
snapshot sequences — no HTTP, no GPU, no sleeps.

| Test | Verifies |
|---|---|
| `never_starts_while_vram_above_floor` | **The core invariant.** 30 polls at 24753 MiB never yield `StartNode` |
| `unreadable_vram_fails_closed` | `vram_used_mib: None` ⇒ `Fail`, never a blind start |
| `fails_closed_on_release_timeout_instead_of_starting` | Wedged VRAM ⇒ `Fail` naming the observed MiB and "would OOM"; stays failed |
| `starts_only_after_vram_observed_below_floor` | Replays the real measured sequence 24753 → 9000 → 311 → 13615 → 27509 |
| `target_is_never_stopped` | Switching to a node that is mid-load does not stop the thing being switched to |
| `active_but_not_ready_never_counts_as_resident` | `Type=simple` `active` ≠ loaded (true for the whole 36–161 s load) |
| `stops_every_other_active_holder_including_comfyui` | ComfyUI participates as a VRAM holder |
| `already_resident_target_is_a_no_op` | Avoids a pointless reload |
| `ready_timeout_fails_with_a_clear_reason` | Bounded wait, not a hang |
| `missing_node_snapshot_does_not_panic_or_start` | Config drift mid-flight is safe |
| `parses_real_agent_state_llama` / `_comfyui_null_model` / `_stopped` | Verbatim captures from the deployed agents, including `"model":null` |
| `parses_agent_action_and_error_bodies` | Start/stop and allowlist-rejection payloads |
| `agent_state_tolerates_missing_fields` | `{}` does not panic |

### Config tests (`src/config.rs`)

Env access is injectable (`Config::load_from(&get)`), so tests never touch
process-global env — that would make them order-dependent under Rust's parallel
test runner.

| Test | Verifies |
|---|---|
| `parses_three_nodes_in_order` | `NODE_<i>_*` scan order, kinds, aliases, URLs |
| `scan_stops_at_first_gap` | A missing `NODE_1` means `NODE_2` is ignored |
| `empty_env_yields_no_nodes_not_a_panic` | Graceful degradation |
| `half_configured_node_is_kept_but_not_switchable` | Missing token ⇒ display-only, startup continues |
| `blank_token_is_treated_as_missing` | Whitespace-only is not a token |
| `llama_node_without_server_url_is_not_observable` | `observable` vs `switchable` are independent |
| `unknown_kind_falls_back_to_llama` | Bad `KIND` warns, does not fail |
| `debug_output_redacts_tokens` | `?config` (logged by `main.rs`) contains no token substring |
| `config_debug_redacts_monitor_api_token` | Same for `MONITOR_API_TOKEN` |
| `secret_matches_without_short_circuit` | Length-independent compare; unset secret authenticates nothing |
| `node_scan_max_bounds_the_scan` | 40 declared nodes, cap 8 ⇒ 8 parsed |
| `numeric_env_parsing_and_garbage_fallback` | Bad numbers warn and default |
| `ollama_base_url_behaviour_unchanged` | Regression guard for the pre-existing helper |

### Switch agent tests (`src/bin/switch_agent.rs`)

| Test | Verifies |
|---|---|
| `token_compare_matches_and_rejects` | Constant-ish-time compare, including length mismatch |
| `unit_charset_rejects_injection_attempts` | `; rm -rf /`, `$(reboot)`, backticks, `\|`, traversal, spaces, empty |
| `allowlist_returns_canonical_not_request_string` | The string reaching `systemctl` comes from the allowlist; non-listed ⇒ 403, bad charset ⇒ 400 |
| `authorize_requires_bearer` | Missing ⇒ 401, wrong scheme ⇒ 401, wrong token ⇒ 403, correct ⇒ ok |

## Mock infrastructure

- **Mock Ollama** — axum server on random port returning deterministic JSON on `/api/tags`
- **Mock nvidia-smi** — Executable shell script in temp dir printing deterministic CSV
- **Temp databases** — Unique filenames (PID + nanosecond) for full test isolation

## Live verification

Some guarantees cannot be tested hermetically and were verified against the real
deployment instead, with results recorded in the module docs:

* `live_probe_against_real_server` (`#[ignore]`d) was run against both live
  servers — see [llama.cpp Client](../src/llamacpp.md).
* Switch ordering was verified by driving `POST /api/model` while independently
  sampling host `nvidia-smi`, so the VRAM trace is observed rather than assumed —
  see [Switcher Module](../src/switcher.md).
* Agent auth and allowlist rejections were exercised over HTTP against all three
  deployed containers — see [Switch Agent Deployment](../deployment/switch-agent.md).

Live checks are **not** part of CI: they need the GPU host. The fixture tests above
are the hermetic gate, and the fixtures are real captures, so schema drift still
shows up.

## CI verification (local)

```bash
cargo fmt --all -- --check
cargo clippy --all-targets -- -D warnings
RUSTFLAGS="-D warnings" cargo build --release
RUSTFLAGS="-D warnings" cargo test --all-targets
```

## See also

- [Tests Module](../src/tests.md) — Test harness implementation details
- [CI/CD Pipeline](ci-cd.md) — GitHub Actions workflow definitions

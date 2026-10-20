---
type: Source Module
title: Switcher Module
description: VRAM arbitration between mutually exclusive GPU workloads — ordering invariant, state machine, and measured switch timings.
resource: src/switcher.rs
tags: [rust, switcher, vram, gpu, state-machine, safety]
timestamp: 2026-10-19T00:00:00Z
---

# Switcher Module

`src/switcher.rs` arbitrates a **single GPU between mutually exclusive workloads**.

## The invariant

Only one model can be VRAM-resident at a time. On the Tesla V100-SXM3-32GB:

| | weights | KV (measured/derived) | total |
|---|---|---|---|
| Qwen3.8-27B Q4_K_M | 15702 MiB | ~65 KiB/token (hybrid gated-delta-net) | 27509 MiB at 3×61440 |
| Precog-24B-v1b Q4_K_M | 13669 MiB | ~160 KiB/token fp16, ~85 KiB/token q8_0 | 24753 MiB at 2×61440 (FA + q8_0) |

Both at once is ~30.7 GB of weights before any KV cache. Starting a target before
the incumbent released VRAM is a **guaranteed CUDA OOM**.

> **Therefore: the target is never started until host VRAM has been observed below
> `SWITCH_VRAM_FLOOR_MIB`.** On timeout the switch *fails*; it never starts anyway
> "and hopes". Likewise, if VRAM cannot be read at all, the switch fails closed —
> we cannot prove the card is free, so we do not load onto it.

## Why VRAM comes from NVML and state from agents

`nvidia-smi --query-compute-apps` reports `process_name` as **`[Not Found]`** for
processes inside containers, because PID namespaces are not shared. Verified live:
two GPU PIDs (`2427`, `1257391`) both rendered as `[Not Found]`.

So responsibility is split:

* **NVML totals** (host `nvidia-smi`) are the authoritative VRAM guard —
  namespace-independent, and cannot lie about how much card is in use.
* **Switch agents** self-report which unit is active and whether their model is
  ready. This is the only reliable source of *attribution*.

If an agent is unreachable we do **not** guess that its node is idle: it is marked
`unreachable`, and the VRAM floor check will simply never pass, failing the switch
closed.

## Pure state machine

Ordering is a pure function, [`SwitchPlan::next(&WorldSnapshot) -> SwitchAction`],
with no I/O and no clocks. The async `Switcher` driver gathers real snapshots and
executes the returned action. This is what makes the safety property **testable**:
the invariant is asserted over deterministic snapshot sequences, with no HTTP, no
GPU and no sleeps.

Phases: `init → stopping-others → waiting-for-vram → starting-target →
waiting-for-ready → finished`.

Actions: `StopNode(i)`, `StartNode(i)`, `Wait`, `Done`, `Fail(reason)`.

Tests that pin the invariant:

| Test | Guarantees |
|---|---|
| `never_starts_while_vram_above_floor` | `StartNode` never appears across 30 polls at 24753 MiB |
| `unreadable_vram_fails_closed` | `vram_used_mib: None` ⇒ `Fail`, never `StartNode` |
| `fails_closed_on_release_timeout_instead_of_starting` | wedged VRAM ⇒ `Fail` containing "would OOM" |
| `starts_only_after_vram_observed_below_floor` | replays the real measured sequence |
| `target_is_never_stopped` | switching to a node that is mid-load does not stop it |
| `active_but_not_ready_never_counts_as_resident` | `Type=simple` active ≠ loaded |
| `stops_every_other_active_holder_including_comfyui` | ComfyUI participates |

## Readiness by node kind

* `NodeKind::Llama` — ready only when `/props` returns `200` (see
  [llamacpp.md](llamacpp.md)). `unit_active` is true for the entire load, so it is
  *not* readiness.
* `NodeKind::ComfyUi` — no `/props`; readiness = systemd unit active.

## Measured switch performance

Driven through `POST /api/model` against the live containers:

| Direction | VRAM trace | release | ready | total |
|---|---|---|---|---|
| llama01 → llama02 | 27509 → 657 → 0 → 311 → 13615 → 24445 | 161 s* | 152 s | 161 s |
| llama02 → llama01 | 24445 → 349 → 311 → 0 → 27161 → 27201 | 16 s | 40 s | 57 s |

\* the first switch's `release_secs` was reported as 161 s due to a bug fixed
during development — see below.

`stopped_nodes` included `comfyui` on the first switch, and `vram_min_mib` hit
**0**, confirming ComfyUI's 308 MiB context was released too.

A switch to an already-resident target is a **no-op**: `success:true, no_op:true,
total_secs:0`. This avoids a pointless 36–148 s reload.

## Known bug found and fixed by live testing

`release_secs` was stamped when the first `StopNode` was *issued*, then
unconditionally overwritten in the `Done` branch, so it always equalled
`total_secs`. Observed live as `release_secs: 161, total_secs: 161` while the VRAM
trace showed the card empty at ~12 s. It is now stamped when `StartNode` is issued
— the first moment VRAM is *known* to be below the floor — and the `Done` branch
only fills it in for the no-op path. After the fix: `release_secs:16,
ready_secs:40, total_secs:57`.

This is why the switch is verified by observation, not by reading the code.

## Types

| Type | Purpose |
|---|---|
| `SwitchPlan` | Pure ordering state machine |
| `SwitchAction` | What the driver should do next |
| `WorldSnapshot` / `NodeSnapshot` | Everything the machine may know |
| `Switcher` | Async driver: agents + NVML |
| `AgentClient` | Bearer-auth client for one [switch agent](../deployment/switch-agent.md) |
| `AgentState` / `AgentUnit` / `AgentModel` | Agent wire types |
| `SwitchOutcome` | Result with the measurements that prove it |
| `SwitchProgress` | Live progress for `GET /api/switch` |
| `NodeStatus` | Per-node view for `GET /api/nodes` |

## See also

- [Switch Agent Deployment](../deployment/switch-agent.md) — the per-container agent
- [llama.cpp Client](llamacpp.md) — readiness semantics
- [API Reference](../api/api-reference.md) — `POST /api/model`, `GET /api/switch`
- [Config Module](config.md) — thresholds and node declaration

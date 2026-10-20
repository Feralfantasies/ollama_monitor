---
type: Deployment
title: Switch Agent Deployment
description: Deploying the token-authenticated switch agent into each GPU workload container, and the static-musl build gotcha.
resource: src/bin/switch_agent.rs
tags: [deployment, systemd, lxc, proxmox, security, musl, static]
timestamp: 2026-10-19T00:00:00Z
---

# Switch Agent Deployment

The switch agent is a **second binary in this crate** (`src/bin/switch_agent.rs`),
built as a static musl executable and installed into each GPU workload container.
It is what lets the central monitor stop one workload and start another without
handing out SSH or shell access.

Deployed on the Proxmox host that owns the GPU. Addresses below are **RFC 5737
documentation placeholders** — substitute your own:

| Container | IP (example) | Agent | Allowlisted unit |
|---|---|---|---|
| `llama01` (Qwen3.8-27B) | 192.0.2.10 | `:8100` | `llama-server.service` |
| `llama02` (Precog-24B) | 192.0.2.11 | `:8100` | `llama-server.service` |
| `comfyui` | 192.0.2.12 | `:8100` | `comfyui.service` |

The monitor itself runs in its own container (`llama-mon`, 192.0.2.13) — see
[systemd deployment](systemd.md).

> **This repository is public**, so real LAN addresses, container IDs and
> hostnames are deliberately kept out of it. The measured VRAM figures, load
> times and throughput numbers quoted throughout this bundle are real; the
> addresses are not.

## Security model

* **Allowlist only.** The sole operations are `systemctl start|stop` plus read-only
  `is-active`/`is-enabled`, and only for unit names appearing verbatim in
  `SWITCH_AGENT_ALLOWED_UNITS`. There is no arbitrary command execution path.
* **Canonical string substitution.** `check_unit()` returns the string *from the
  allowlist*, never from the request body, so a crafted name cannot reach
  `systemctl` even if it somehow passed the charset check.
* **Charset validation.** `^[A-Za-z0-9@:_.\-]{1,200}$` rejects `;`, `$()`, backticks,
  `|` and path traversal before the allowlist is even consulted.
* **Bearer token required** on every route except `/health`, compared
  length-independently without short-circuiting.
* **Fail closed at startup.** No token, or an empty allowlist, and the agent exits
  rather than serving unauthenticated.
* **Per-container tokens.** Each container has its own token, so compromising one
  does not reach the others.

Verified live (all three containers):

| Request | Result |
|---|---|
| `GET /health`, no auth | `200 {"ok":true}` |
| `GET /state`, no token | `401 missing bearer token` |
| `GET /state`, wrong token | `403 invalid token` |
| `GET /state`, `Basic` scheme | `401` |
| stop `comfyui.service` on llama01 | `403 not in the allowlist` |
| stop `llama-server.service` on comfyui | `403 not in the allowlist` |
| stop `ssh.service` | `403 not in the allowlist` |
| stop `"llama-server.service; reboot"` | `400 invalid unit name` |
| stop `"$(reboot)"` | `400 invalid unit name` |
| stop `"../../etc/passwd"` | `400 invalid unit name` |
| stop `""` | `400 unit name required` |

## Build — the static-musl gotcha

This cost a debugging cycle, so it is written down.

```bash
cargo build --release --target x86_64-unknown-linux-musl --bin switch_agent
```

`ring` (pulled in by `reqwest`'s rustls) needs a C compiler for the musl target:

```bash
sudo apt-get install -y musl-tools   # what CI does too
```

> **Do not set `CARGO_TARGET_X86_64_UNKNOWN_LINUX_MUSL_LINKER=musl-gcc`.**
> `musl-gcc` links *dynamically* by default, which defeats Rust's self-contained
> static musl linking. The result looks fine — `ldd` even prints
> `statically linked` — but the binary carries `PT_INTERP` pointing at
> `/lib/ld-musl-x86_64.so.1`, which does not exist in an Ubuntu container. Every
> `execve` then fails with `ENOENT`, surfacing as systemd `status=203/EXEC`
> "No such file or directory" for a file that verifiably exists.
>
> Set only `CC_x86_64_unknown_linux_musl=musl-gcc` (for `ring`'s C code) and let
> Rust choose the linker.

Verify before deploying — `ldd` is **not** sufficient:

```bash
readelf -l target/x86_64-unknown-linux-musl/release/switch_agent | grep -c INTERP
# must print 0
readelf -d ... | grep -c NEEDED     # must print 0
ldd ...                             # "statically linked"
```

## Install

```bash
# from the Proxmox host; <CTID> is the container's numeric ID
pct exec <CTID> -- mkdir -p /etc/switch-agent
pct push <CTID> switch_agent /usr/local/bin/switch-agent --perms 0755
pct push <CTID> env           /etc/switch-agent/env    --perms 0600
pct push <CTID> unit          /etc/systemd/system/switch-agent.service --perms 0644
pct exec <CTID> -- systemctl daemon-reload
pct exec <CTID> -- systemctl enable --now switch-agent
```

`/etc/switch-agent/env` (0600 root:root) holds the secret; the unit file does not.

```ini
SWITCH_AGENT_TOKEN=<openssl rand -hex 32>
SWITCH_AGENT_ALLOWED_UNITS=llama-server.service
SWITCH_AGENT_BIND=192.0.2.11
SWITCH_AGENT_PORT=8100
SWITCH_AGENT_LABEL=llama02
SWITCH_AGENT_PROPS_URL=http://192.0.2.11:8000/props
```

Unit file:

```ini
[Unit]
Description=Switch agent - token-authenticated systemd unit controller for GPU workload arbitration
After=network-online.target
Wants=network-online.target

[Service]
Type=simple
ExecStart=/usr/local/bin/switch-agent
EnvironmentFile=/etc/switch-agent/env
Restart=on-failure
RestartSec=5
NoNewPrivileges=yes
PrivateTmp=yes
ProtectSystem=full
ProtectHome=yes

[Install]
WantedBy=multi-user.target
```

`SWITCH_AGENT_PROPS_URL` is what lets the agent report genuine *readiness* rather
than just "the unit is active" — see the [readiness rule](../src/llamacpp.md).

## Endpoints

| Method | Path | Auth | Purpose |
|---|---|---|---|
| `GET` | `/health` | no | liveness |
| `GET` | `/state` | bearer | label, per-unit `active`/`enabled`, model readiness |
| `POST` | `/start` | bearer | `{"unit":"..."}` — allowlisted only |
| `POST` | `/stop` | bearer | `{"unit":"..."}` — allowlisted only |

`GET /state` response (real capture from `llama01`):

```json
{"label":"llama01",
 "units":[{"unit":"llama-server.service","active":"active","enabled":"enabled"}],
 "model":{"ready":true,"model_alias":"Qwen3.8-27B","n_ctx":61440,
          "total_slots":3,"http_status":200,"error":null}}
```

On a non-llama node `model` is `null` (real capture from `comfyui`).

## Operational notes

* **Tokens must never be committed.** They live only in
  `/root/switch-agent-tokens.env` on the host (0600) and each container's
  `/etc/switch-agent/env` (0600). Generate with `openssl rand -hex 32`.
* When writing tokens into a container env file, pass them **via stdin**, not as a
  command argument — otherwise they appear in `ps` output.
* A full clone of a container duplicates its **SSH host keys**. Regenerate them
  (`rm /etc/ssh/ssh_host_*; ssh-keygen -A; systemctl restart ssh`) or two hosts
  share a key and host verification becomes meaningless.
* Proxmox rewrites `/etc/hostname` and the `# --- BEGIN PVE ---` block in
  `/etc/hosts` on start, so a clone's identity self-corrects from
  `pct set --hostname`. `machine-id` was observed to differ automatically.
* **Always readdress a clone's `net0` before starting it.** A full clone inherits
  the source IP; two containers on one address silently breaks egress for the newer
  one (the router's ARP entry points at the incumbent). This exact failure was
  diagnosed earlier on this host.
* `systemctl is-active` exits **non-zero** for `activating`/`inactive`. Under
  `set -e`, an unguarded probe aborts a deploy script part-way through.
* `pct push` onto a **running binary** fails with `Text file busy` — stop the
  service first. Note `pct push` can still exit 0 after printing that error, so
  verify with `sha256sum` inside the container rather than trusting the exit code.
* `pct restart` **does not exist**; use `pct stop` + `pct start` (or `pct reboot`).

## See also

- [Switcher Module](../src/switcher.md) — the orchestration that drives these agents
- [Systemd Deployment](systemd.md) — the monitor service
- [Config Module](../src/config.md) — `NODE_<i>_AGENT_URL` / `_AGENT_TOKEN`

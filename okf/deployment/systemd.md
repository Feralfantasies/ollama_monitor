---
type: Deployment Guide
title: Systemd Deployment
description: Systemd service unit configuration for persistent background operation.
tags: [systemd, linux, deployment, service]
timestamp: 2026-06-18T00:00:00Z
---

# Systemd Deployment

Run ollama_monitor as a persistent background service that auto-restarts on failure.

## Service unit file

Create `/etc/systemd/system/ollama_monitor.service`:

```ini
[Unit]
Description=Ollama Monitor
After=network-online.target
Wants=network-online.target

[Service]
Type=simple
ExecStart=/usr/local/bin/ollama_monitor
Restart=always
RestartSec=5
Environment=RUST_LOG=info
Environment=OLLAMA_HOST=http://127.0.0.1
Environment=OLLAMA_PORT=11434
Environment=SERVER_BIND=0.0.0.0
Environment=SERVER_PORT=3000
Environment=REFRESH_INTERVAL_SECS=15
Environment=GPU_DEVICE_INDEX=0
StandardOutput=journal
StandardError=journal
User=root
Group=root

[Install]
WantedBy=multi-user.target
```

## Installation steps

```bash
# 1. Install the binary
sudo cp ./target/release/ollama_monitor /usr/local/bin/
sudo chmod 755 /usr/local/bin/ollama_monitor

# 2. Enable and start the service
sudo systemctl daemon-reload
sudo systemctl enable ollama_monitor
sudo systemctl start ollama_monitor

# 3. Verify it is running
sudo systemctl status ollama_monitor
```

## Management commands

| Command | Purpose |
|---|---|
| `sudo systemctl status ollama_monitor` | Check service status |
| `sudo journalctl -u ollama_monitor -f` | View live logs |
| `sudo systemctl restart ollama_monitor` | Restart after config changes |
| `sudo systemctl stop ollama_monitor` | Stop the service |
| `sudo systemctl disable ollama_monitor` | Prevent auto-start on boot |

## Notes

- Use `User=root` and `Group=root` if ollama_monitor needs access to `nvidia-smi` and `/proc`. Adjust to a dedicated service user for tighter security.
- All environment variables can be moved to a separate env file: `EnvironmentFile=/etc/ollama_monitor.env`.

## See also

- [Docker Deployment](docker.md) — Container deployment alternative
- [Config Module](/src/config.md) — All available environment variables

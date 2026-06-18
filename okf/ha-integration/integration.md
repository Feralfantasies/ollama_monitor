---
type: Integration
title: Home Assistant Integration
description: Native HA custom integration exposing ollama_monitor metrics as sensors.
resource: ha_integration/custom_components/ollama_monitor/
tags: [home-assistant, python, custom-integration, sensors]
timestamp: 2026-06-18T00:00:00Z
---

# Home Assistant Integration

A native Home Assistant custom integration — no YAML config, no MQTT, no terminal access needed. Metrics from ollama_monitor appear as real HA sensors usable in dashboards, automations, and graphs.

## Architecture

| File | Purpose |
|---|---|
| `__init__.py` | Integration setup and unload logic |
| `config_flow.py` | GUI config flow — user enters monitor URL |
| `const.py` | Constants, sensor keys, platform list |
| `coordinator.py` | `DataUpdateCoordinator` polls `/api/status` every 15 seconds |
| `manifest.json` | HA integration manifest (version 1.0.0) |
| `sensor.py` | Sensor platform — registers 5 sensors under one device |
| `strings.json` | Localized strings for config flow UI |

## Data flow

```
Ollama Monitor :3000/api/status ──→ DataUpdateCoordinator (every 15s) ──→ Sensor states
```

The coordinator parses the JSON response into `OllamaMonitorData` and distributes values to individual sensor entities.

## Sensors

All sensors appear under a single **Ollama Monitor** device entry:

| Sensor | Unit | State Values | Key attributes |
|---|---|---|---|
| **Ollama Status** | — | `online` / `offline` | `loaded_model`, `model_count`, `ollama_url` |
| **GPU Temperature** | °C | numeric | `gpu_name` |
| **GPU Memory Used** | MiB | numeric | `gpu_name`, `memory_total_mib`, `memory_remaining_mib`, `memory_usage_pct` |
| **GPU Utilization** | % | numeric | `gpu_name` |
| **GPU Power** | W | numeric | `gpu_name` |

All numeric sensors use `state_class: measurement` for HA statistics/history graphs.

## Installation

### HACS (recommended)

1. Add repo `https://github.com/Feralfantasies/ollama_monitor` as custom repository in HACS → Integrations
2. Category: **Integration**
3. Download, restart Home Assistant

### Manual

Copy files from `ha_integration/custom_components/ollama_monitor/` to:

| HA install type | Target path |
|---|---|
| Supervised / Docker | `/config/custom_components/ollama_monitor/` |
| Core | `~/.homeassistant/custom_components/ollama_monitor/` |
| OS | `/usr/share/hassio/homeassistant/custom_components/ollama_monitor/` |

Then restart Home Assistant.

## Setup (GUI)

1. **Settings** → **Devices & Services** → **Add integration**
2. Search for **Ollama Monitor**
3. Enter monitor URL (e.g. `http://192.168.1.10:3000`)
4. Integration tests `/api/status` immediately
5. Click **Submit** on success

## See also

- [REST API Reference](/api/api-reference.md) — The `/api/status` endpoint the integration polls

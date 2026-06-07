"""Constants for the Ollama Monitor integration."""

from __future__ import annotations

from homeassistant.const import Platform

DOMAIN = "ollama_monitor"

# Config flow keys
CONF_MONITOR_URL = "monitor_url"
DEFAULT_MONITOR_URL = "http://192.168.1.1:3000"

# Sensor platform
PLATFORMS: list[Platform] = [Platform.SENSOR]

# Sensor names for unique IDs
SENSOR_OLLAMA_STATUS = "ollama_status"
SENSOR_GPU_TEMPERATURE = "gpu_temperature"
SENSOR_GPU_MEMORY_USED = "gpu_memory_used"
SENSOR_GPU_UTILIZATION = "gpu_utilization"
SENSOR_GPU_POWER = "gpu_power"

# Sensor display names
NAME = "Ollama Monitor"

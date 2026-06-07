"""Data update coordinator for the Ollama Monitor integration."""

from __future__ import annotations

import logging
from dataclasses import dataclass
from datetime import timedelta
from typing import Any

import aiohttp
from homeassistant.core import HomeAssistant
from homeassistant.helpers.update_coordinator import DataUpdateCoordinator, UpdateFailed

from .const import DOMAIN

_LOGGER = logging.getLogger(__name__)

# How often to poll /api/status — matches the ollama_monitor default refresh.
DEFAULT_SCAN_INTERVAL = timedelta(seconds=15)


@dataclass
class OllamaMonitorData:
    """Full data payload from /api/status."""

    ollama_url: str
    ollama_reachable: bool
    loaded_model: str | None
    available_models_count: int
    gpu_name: str | None
    gpu_temperature_c: float | None
    gpu_memory_used_mib: int | None
    gpu_memory_total_mib: int | None
    gpu_memory_remaining_mib: int | None
    gpu_utilization_pct: float | None
    gpu_power_watts: float | None
    timestamp: str

    @classmethod
    def from_api_response(cls, data: dict[str, Any]) -> OllamaMonitorData:
        """Parse /api/status JSON response into OllamaMonitorData."""
        gpu = data.get("gpu") or {}
        models = data.get("available_models") or []
        return cls(
            ollama_url=data.get("ollama_url", ""),
            ollama_reachable=data.get("ollama_reachable", False),
            loaded_model=data.get("loaded_model"),
            available_models_count=len(models),
            gpu_name=gpu.get("name"),
            gpu_temperature_c=gpu.get("temperature_c"),
            gpu_memory_used_mib=gpu.get("memory_used_mib"),
            gpu_memory_total_mib=gpu.get("memory_total_mib"),
            gpu_memory_remaining_mib=gpu.get("memory_remaining_mib"),
            gpu_utilization_pct=gpu.get("utilization_pct"),
            gpu_power_watts=gpu.get("power_watts"),
            timestamp=data.get("timestamp", ""),
        )


class OllamaMonitorCoordinator(DataUpdateCoordinator[OllamaMonitorData]):
    """Fetches data from the Ollama Monitor REST API."""

    def __init__(
        self,
        hass: HomeAssistant,
        client: aiohttp.ClientSession,
        monitor_url: str,
    ) -> None:
        """Initialize the coordinator."""
        super().__init__(
            hass=hass,
            logger=_LOGGER,
            name=DOMAIN,
            update_interval=DEFAULT_SCAN_INTERVAL,
            always_update=False,
        )
        self._client = client
        self._monitor_url = monitor_url.rstrip("/")

    async def _async_update_data(self) -> OllamaMonitorData:
        """Fetch latest status from /api/status."""
        try:
            response = await self._client.get(
                f"{self._monitor_url}/api/status",
                timeout=aiohttp.ClientTimeout(total=10),
            )
            response.raise_for_status()
            payload = await response.json()
        except TimeoutError as err:
            raise UpdateFailed(
                f"Timeout connecting to monitor at {self._monitor_url}"
            ) from err
        except aiohttp.ClientError as err:
            raise UpdateFailed(
                f"Error fetching data from {self._monitor_url}: {err}"
            ) from err

        return OllamaMonitorData.from_api_response(payload)

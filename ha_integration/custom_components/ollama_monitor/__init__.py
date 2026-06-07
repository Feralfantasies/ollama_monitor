"""The Ollama Monitor integration."""

from __future__ import annotations

import logging

from homeassistant.config_entries import ConfigEntry
from homeassistant.core import HomeAssistant
from homeassistant.helpers.aiohttp_client import async_get_clientsession

from .const import CONF_MONITOR_URL, DOMAIN, NAME, PLATFORMS
from .coordinator import OllamaMonitorCoordinator

_LOGGER = logging.getLogger(__name__)


async def async_setup_entry(hass: HomeAssistant, entry: ConfigEntry) -> bool:
    """Set up Ollama Monitor from a config entry."""
    _LOGGER.info("Setting up %s integration (title=%s)", NAME, entry.title)

    # Create the data update coordinator.
    coordinator = OllamaMonitorCoordinator(
        hass=hass,
        client=async_get_clientsession(hass),
        monitor_url=entry.data[CONF_MONITOR_URL],
    )

    # Fetch initial data before registering platforms.
    await coordinator.async_config_entry_first_refresh()

    # Store coordinator for sensor platform to access.
    hass.data.setdefault(DOMAIN, {})
    hass.data[DOMAIN][entry.entry_id] = coordinator

    # Forward setup to sensor platform.
    await hass.config_entries.async_forward_entry_setups(entry, PLATFORMS)

    return True


async def async_unload_entry(hass: HomeAssistant, entry: ConfigEntry) -> bool:
    """Unload a config entry."""
    if unload_ok := await hass.config_entries.async_unload_platforms(entry, PLATFORMS):
        hass.data[DOMAIN].pop(entry.entry_id)
        _LOGGER.info("Unloaded %s integration", NAME)
    return unload_ok

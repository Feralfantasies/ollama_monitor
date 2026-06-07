"""Config flow for the Ollama Monitor integration."""

from __future__ import annotations

import logging
from typing import Any

import aiohttp
import voluptuous as vol
from homeassistant import config_entries
from homeassistant.core import HomeAssistant, callback
from homeassistant.data_entry_flow import FlowResult
from homeassistant.helpers.aiohttp_client import async_get_clientsession
import homeassistant.helpers.config_validation as cv  # noqa: F401

from .const import CONF_MONITOR_URL, DEFAULT_MONITOR_URL, DOMAIN

_LOGGER = logging.getLogger(__name__)


class OllamaMonitorConfigFlow(config_entries.ConfigFlow, domain=DOMAIN):  # type: ignore[call-arg]
    """Handle a config flow for Ollama Monitor."""

    VERSION = 1

    async def async_step_user(
        self, user_input: dict[str, Any] | None = None
    ) -> FlowResult:
        """Handle the initial user step — show the monitor URL form."""
        errors: dict[str, str] = {}

        if user_input is not None:
            # Validate the connection to the monitor before creating the entry.
            monitor_url = user_input[CONF_MONITOR_URL].rstrip("/")
            try:
                await self._validate_connection(monitor_url)
            except CannotConnect:
                errors["base"] = "cannot_connect"
            except InvalidUrl:
                errors["base"] = "invalid_url"
            else:
                # Connection OK — create the config entry.
                await self.async_set_unique_id(monitor_url)
                self._abort_if_unique_id_configured()
                return self.async_create_entry(
                    title=monitor_url,
                    data={CONF_MONITOR_URL: monitor_url},
                )

        return self.async_show_form(
            step_id="user",
            data_schema=vol.Schema(
                {
                    vol.Required(
                        CONF_MONITOR_URL,
                        default=DEFAULT_MONITOR_URL,
                    ): cv.url
                }
            ),
            errors=errors,
        )

    @staticmethod
    @callback
    def async_get_options_flow(
        config_entry: config_entries.ConfigEntry,
    ) -> OllamaMonitorOptionsFlowHandler:
        """Options flow not needed — all config is in the initial form."""
        return OllamaMonitorOptionsFlowHandler(config_entry)

    async def _validate_connection(self, monitor_url: str) -> None:
        """Fetch /api/status to verify the monitor is reachable and responding."""
        try:
            client = async_get_clientsession(self.hass)
            response = await client.get(
                f"{monitor_url}/api/status",
                timeout=aiohttp.ClientTimeout(total=10),
            )
            if response.status == 200:
                payload = await response.json()
                if not isinstance(payload, dict):
                    raise CannotConnect
                return
        except TimeoutError as err:
            raise CannotConnect from err
        except aiohttp.ClientError as err:
            raise CannotConnect from err
        except Exception as exc:
            _LOGGER.warning("Connection validation failed: %s", exc)
            raise CannotConnect from exc

        raise CannotConnect


class OllamaMonitorOptionsFlowHandler(config_entries.OptionsFlow):
    """Handle options flow — placeholder for future settings."""

    def __init__(self, config_entry: config_entries.ConfigEntry) -> None:
        """Initialize options flow."""
        self.config_entry = config_entry

    async def async_step_init(
        self, user_input: dict[str, Any] | None = None
    ) -> FlowResult:
        """No options to configure yet."""
        return self.async_create_entry(title="", data={})


class CannotConnect(Exception):
    """Error to indicate connection failure."""


class InvalidUrl(Exception):
    """Error to indicate invalid URL format."""

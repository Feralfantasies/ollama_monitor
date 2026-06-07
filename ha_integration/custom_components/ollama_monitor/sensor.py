"""Sensor platform for the Ollama Monitor integration."""

from __future__ import annotations

from homeassistant.components.sensor import (
    SensorDeviceClass,
    SensorEntity,
    SensorEntityDescription,
    SensorStateClass,
)
from homeassistant.const import (
    PERCENTAGE,
    UnitOfTemperature,
)
from homeassistant.core import HomeAssistant
from homeassistant.helpers.device_registry import DeviceEntryType
from homeassistant.helpers.entity import EntityCategory
from homeassistant.helpers.entity_platform import AddEntitiesCallback
from homeassistant.helpers.update_coordinator import CoordinatorEntity
from homeassistant.config_entries import ConfigEntry

from .const import (
    DOMAIN,
    NAME,
    SENSOR_GPU_MEMORY_USED,
    SENSOR_GPU_POWER,
    SENSOR_GPU_TEMPERATURE,
    SENSOR_GPU_UTILIZATION,
    SENSOR_OLLAMA_STATUS,
)
from .coordinator import OllamaMonitorData, OllamaMonitorCoordinator

# ── Unit constants ──────────────────────────────────────────────────────

DATA_MEBIBYTE = "MiB"
DATA_UNKNOWN = "unknown"

# ── Sensor descriptions ─────────────────────────────────────────────────────

SENSOR_DESCRIPTIONS: tuple[SensorEntityDescription, ...] = (
    SensorEntityDescription(
        key=SENSOR_OLLAMA_STATUS,
        name="Ollama Status",
        icon="mdi:robot",
        entity_category=EntityCategory.DIAGNOSTIC,
    ),
    SensorEntityDescription(
        key=SENSOR_GPU_TEMPERATURE,
        name="GPU Temperature",
        native_unit_of_measurement=UnitOfTemperature.CELSIUS,
        device_class=SensorDeviceClass.TEMPERATURE,
        state_class=SensorStateClass.MEASUREMENT,
        suggested_display_precision=1,
        icon="mdi:thermometer",
    ),
    SensorEntityDescription(
        key=SENSOR_GPU_MEMORY_USED,
        name="GPU Memory Used",
        native_unit_of_measurement=DATA_MEBIBYTE,
        state_class=SensorStateClass.MEASUREMENT,
        icon="mdi:memory",
    ),
    SensorEntityDescription(
        key=SENSOR_GPU_UTILIZATION,
        name="GPU Utilization",
        native_unit_of_measurement=PERCENTAGE,
        state_class=SensorStateClass.MEASUREMENT,
        suggested_display_precision=0,
        icon="mdi:speedometer",
    ),
    SensorEntityDescription(
        key=SENSOR_GPU_POWER,
        name="GPU Power",
        native_unit_of_measurement="W",
        device_class=SensorDeviceClass.POWER,
        state_class=SensorStateClass.MEASUREMENT,
        icon="mdi:flash",
    ),
)

# ── Sensor factory ──────────────────────────────────────────────────────


def _create_sensor(
    coordinator: OllamaMonitorCoordinator,
    description: SensorEntityDescription,
    entry_id: str,
) -> OllamaMonitorSensor:
    """Create the correct sensor subclass based on description key."""
    key = description.key
    if key == SENSOR_OLLAMA_STATUS:
        return OllamaStatusSensor(coordinator, description, entry_id)
    if key == SENSOR_GPU_TEMPERATURE:
        return GpuTemperatureSensor(coordinator, description, entry_id)
    if key == SENSOR_GPU_MEMORY_USED:
        return GpuMemorySensor(coordinator, description, entry_id)
    if key == SENSOR_GPU_UTILIZATION:
        return GpuUtilizationSensor(coordinator, description, entry_id)
    if key == SENSOR_GPU_POWER:
        return GpuPowerSensor(coordinator, description, entry_id)
    # Fallback — should not happen with current SENSOR_DESCRIPTIONS.
    return OllamaMonitorSensor(coordinator, description, entry_id)


async def async_setup_entry(
    hass: HomeAssistant,
    config_entry: ConfigEntry,
    async_add_entities: AddEntitiesCallback,
) -> None:
    """Set up sensor entities."""
    coordinator: OllamaMonitorCoordinator = hass.data[DOMAIN][config_entry.entry_id]

    async_add_entities(
        _create_sensor(coordinator, desc, config_entry.entry_id)
        for desc in SENSOR_DESCRIPTIONS
    )


# ── Sensor base ──────────────────────────────────────────────────────────────

class OllamaMonitorSensor(CoordinatorEntity[OllamaMonitorCoordinator], SensorEntity):
    """Base sensor entity backed by the Ollama Monitor coordinator."""

    _attr_has_entity_name = True
    _attr_should_poll = False

    def __init__(
        self,
        coordinator: OllamaMonitorCoordinator,
        description: SensorEntityDescription,
        entry_id: str,
    ) -> None:
        """Initialize the sensor."""
        super().__init__(coordinator)
        self.entity_description = description

        self._attr_unique_id = f"{entry_id}_{description.key}"
        self._attr_device_info = {
            "identifiers": {(DOMAIN, entry_id)},
            "name": NAME,
            "manufacturer": "acleveland",
            "model": "Ollama Monitor",
            "sw_version": "1.0.0",
            "entry_type": DeviceEntryType.SERVICE,
        }

    @property
    def extra_state_attributes(self) -> dict:
        """Extra attributes — override in subclasses."""
        return {}


# ── Ollama status sensor ────────────────────────────────────────────────

class OllamaStatusSensor(OllamaMonitorSensor):
    """Sensor for Ollama availability + loaded model info."""

    @property
    def native_value(self) -> str:  # type: ignore[override]
        """Return 'online' if Ollama is reachable, 'offline' otherwise."""
        data = self.coordinator.data
        if data is None:
            return DATA_UNKNOWN
        return "online" if data.ollama_reachable else "offline"

    @property
    def extra_state_attributes(self) -> dict:  # type: ignore[override]
        """Extra attributes: loaded model, total model count, URL."""
        data = self.coordinator.data
        if data is None:
            return {}
        attrs: dict = {
            "ollama_url": data.ollama_url,
            "model_count": data.available_models_count,
        }
        if data.loaded_model:
            attrs["loaded_model"] = data.loaded_model
        return attrs


# ── GPU numeric sensor base ─────────────────────────────────────────────

class GpuNumericSensor(OllamaMonitorSensor):
    """Sensor subclass for numeric GPU metrics (temperature, memory, utilization, power)."""

    @property
    def extra_state_attributes(self) -> dict:  # type: ignore[override]
        """Include GPU name in attributes."""
        data = self.coordinator.data
        if data is None:
            return {}
        return {"gpu_name": data.gpu_name}


class GpuTemperatureSensor(GpuNumericSensor):
    """GPU temperature sensor."""

    @property
    def native_value(self) -> float | None:  # type: ignore[override]
        """Return GPU temperature in °C."""
        data = self.coordinator.data
        return data.gpu_temperature_c if data is not None else None


class GpuMemorySensor(GpuNumericSensor):
    """GPU memory used sensor with total/remaining attributes."""

    @property
    def native_value(self) -> int | None:  # type: ignore[override]
        """Return GPU memory used in MiB."""
        data = self.coordinator.data
        return data.gpu_memory_used_mib if data is not None else None

    @property
    def extra_state_attributes(self) -> dict:  # type: ignore[override]
        """Include total, remaining, and percentage attributes."""
        data = self.coordinator.data
        if data is None:
            return {}

        total = data.gpu_memory_total_mib
        used = data.gpu_memory_used_mib
        remaining = data.gpu_memory_remaining_mib
        percentage = None
        if total and used and total > 0:
            percentage = round(used / total * 100)

        return {
            "gpu_name": data.gpu_name,
            "memory_total_mib": total,
            "memory_remaining_mib": remaining,
            "memory_usage_pct": percentage,
        }


class GpuUtilizationSensor(GpuNumericSensor):
    """GPU utilization sensor."""

    @property
    def native_value(self) -> float | None:  # type: ignore[override]
        """Return GPU utilization percentage."""
        data = self.coordinator.data
        return data.gpu_utilization_pct if data is not None else None


class GpuPowerSensor(GpuNumericSensor):
    """GPU power draw sensor."""

    @property
    def native_value(self) -> float | None:  # type: ignore[override]
        """Return GPU power draw in Watts (W)."""
        data = self.coordinator.data
        return data.gpu_power_watts if data is not None else None

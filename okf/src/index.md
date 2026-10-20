# Source Modules

* [Config Module](config.md) — Environment variable configuration with defaults
* [API Module](api.md) — Axum REST API server and embedded web dashboard
* [GPU Module](gpu.md) — nvidia-smi CLI wrapper and CSV parser
* [Ollama Client Module](ollama.md) — Ollama REST API client
* [llama.cpp Client Module](llamacpp.md) — llama-server `/v1/models` + `/props` client and the readiness rule
* [Switcher Module](switcher.md) — VRAM arbitration between mutually exclusive GPU workloads
* [Switch Agent](../deployment/switch-agent.md) — second binary (`src/bin/switch_agent.rs`) deployed into each workload container
* [System Metrics Module](system.md) — Linux /proc-based system metric collection
* [Models Module](models.md) — Shared data structures (Rust + serde)
* [Tests Module](tests.md) — Integration test harness with mock infrastructure

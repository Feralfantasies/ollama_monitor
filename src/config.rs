use serde::Deserialize;

/// Application-wide configuration loaded from TOML file or defaults.

#[derive(Debug, Clone)]
pub struct Config {
    pub ollama_host: String,
    pub ollama_port: u16,
    pub server_bind: String,
    pub server_port: u16,
    pub refresh_interval_secs: u64,
    pub gpu_device_index: usize,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            ollama_host: "http://192.168.1.50".into(),
            ollama_port: 11434,
            server_bind: "0.0.0.0".into(),
            server_port: 3000,
            refresh_interval_secs: 15,
            gpu_device_index: 0,
        }
    }
}

impl Config {
    /// Load config from `config.toml` in the current directory, falling back to defaults for missing values.
    pub fn load() -> Self {
        let mut config = Self::default();

        if let Ok(contents) = std::fs::read_to_string("config.toml") {
            if let Ok(parsed) = toml::from_str::<ConfigRaw>(&contents) {
                if let Some(ollama) = parsed.ollama {
                    config.ollama_host = ollama.host.unwrap_or(config.ollama_host);
                    config.ollama_port = ollama.port.unwrap_or(config.ollama_port);
                }
                if let Some(server) = parsed.server {
                    config.server_bind = server.bind.unwrap_or(config.server_bind);
                    config.server_port = server.port.unwrap_or(config.server_port);
                    config.refresh_interval_secs = server.refresh.unwrap_or(config.refresh_interval_secs);
                }
                if let Some(gpu) = parsed.gpu {
                    config.gpu_device_index = gpu.device.unwrap_or(config.gpu_device_index);
                }
            } else {
                tracing::warn!("Failed to parse config.toml, using defaults");
            }
        }

        config
    }

    pub fn ollama_base_url(&self) -> String {
        format!("{}:{}", self.ollama_host, self.ollama_port)
    }
}

// --- Raw TOML parsing structures ---

#[derive(Deserialize)]
struct ConfigRaw {
    ollama: Option<OllamaSection>,
    server: Option<ServerSection>,
    gpu: Option<GpuSection>,
}

#[derive(Deserialize)]
struct OllamaSection {
    host: Option<String>,
    port: Option<u16>,
}

#[derive(Deserialize)]
struct ServerSection {
    bind: Option<String>,
    port: Option<u16>,
    refresh: Option<u64>,
}

#[derive(Deserialize)]
struct GpuSection {
    device: Option<usize>,
}

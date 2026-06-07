/// Application-wide configuration loaded from environment variables with sensible defaults.
///
/// Environment variables:
///
/// | Variable                  | Default               | Description                        |
/// |---------------------------|-----------------------|------------------------------------|
/// | `OLLAMA_HOST`             | `http://127.0.0.1`      | Ollama server base URL             |
/// | `OLLAMA_PORT`             | `11434`               | Ollama server port                 |
/// | `SERVER_BIND`             | `0.0.0.0`             | Address to bind the API server to  |
/// | `SERVER_PORT`             | `3000`                | Port for the API server            |
/// | `REFRESH_INTERVAL_SECS`   | `15`                  | Seconds between status refreshes   |
/// | `GPU_DEVICE_INDEX`        | `0`                   | NVIDIA GPU device index to query   |

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
            ollama_host: "http://127.0.0.1".into(),
            ollama_port: 11434,
            server_bind: "0.0.0.0".into(),
            server_port: 3000,
            refresh_interval_secs: 15,
            gpu_device_index: 0,
        }
    }
}

impl Config {
    /// Load config from environment variables, falling back to defaults for unset values.
    pub fn load() -> Self {
        let mut config = Self::default();

        if let Ok(val) = std::env::var("OLLAMA_HOST") {
            config.ollama_host = val;
        }
        if let Ok(val) = std::env::var("OLLAMA_PORT") {
            if let Ok(port) = val.parse::<u16>() {
                config.ollama_port = port;
            } else {
                tracing::warn!("Invalid OLLAMA_PORT '{}', using default", val);
            }
        }

        if let Ok(val) = std::env::var("SERVER_BIND") {
            config.server_bind = val;
        }
        if let Ok(val) = std::env::var("SERVER_PORT") {
            if let Ok(port) = val.parse::<u16>() {
                config.server_port = port;
            } else {
                tracing::warn!("Invalid SERVER_PORT '{}', using default", val);
            }
        }
        if let Ok(val) = std::env::var("REFRESH_INTERVAL_SECS") {
            if let Ok(interval) = val.parse::<u64>() {
                config.refresh_interval_secs = interval;
            } else {
                tracing::warn!("Invalid REFRESH_INTERVAL_SECS '{}', using default", val);
            }
        }

        if let Ok(val) = std::env::var("GPU_DEVICE_INDEX") {
            if let Ok(index) = val.parse::<usize>() {
                config.gpu_device_index = index;
            } else {
                tracing::warn!("Invalid GPU_DEVICE_INDEX '{}', using default", val);
            }
        }

        config
    }

    pub fn ollama_base_url(&self) -> String {
        // If ollama_host already contains a scheme (e.g. "http://127.0.0.1:12345"),
        // treat it as a complete URL — useful for test mocks.
        if self.ollama_host.starts_with("http://") || self.ollama_host.starts_with("https://") {
            self.ollama_host.clone()
        } else {
            format!("{}:{}", self.ollama_host, self.ollama_port)
        }
    }
}

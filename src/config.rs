use serde::Deserialize;
use std::path::Path;

/// Top-level configuration, deserialized from agent2web.toml.
#[derive(Debug, Default, Deserialize)]
pub struct Config {
    #[serde(default)]
    pub server: ServerConfig,
    #[serde(default)]
    pub forge: ForgeConfig,
    #[serde(default)]
    pub stt: SttConfig,
    #[serde(default)]
    pub diff: DiffConfig,
}

#[derive(Debug, Deserialize)]
pub struct ServerConfig {
    /// Socket address to bind, e.g. "0.0.0.0:8080".
    #[serde(default = "defaults::bind")]
    pub bind: String,

    /// Absolute path to the git repository ForgeCode operates on.
    #[serde(default = "defaults::project_dir")]
    pub project_dir: String,

    /// Maximum seconds a `forge` run may run before being killed.
    #[serde(default = "defaults::run_timeout")]
    pub run_timeout: u64,

    /// Shared password for all mutating endpoints.
    /// Empty string disables authentication entirely.
    /// Overridden by the `AGENT2WEB_PASSWORD` environment variable.
    #[serde(default)]
    pub password: String,

    /// Optional TLS configuration.
    pub tls: Option<TlsConfig>,
}

impl Default for ServerConfig {
    fn default() -> Self {
        Self {
            bind: defaults::bind(),
            project_dir: defaults::project_dir(),
            run_timeout: defaults::run_timeout(),
            password: String::new(),
            tls: Some(TlsConfig {
                enabled: true,
                cert: defaults::tls_cert(),
                key: defaults::tls_key(),
            }),
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
pub struct TlsConfig {
    /// Enable TLS listener.
    #[serde(default)]
    pub enabled: bool,
    /// Path to PEM certificate (absolute, or relative to the config file).
    #[serde(default)]
    pub cert: String,
    /// Path to PEM private key (absolute, or relative to the config file).
    #[serde(default)]
    pub key: String,
}

#[derive(Debug, Deserialize)]
pub struct ForgeConfig {
    /// Name or absolute path of the `forge` binary.
    #[serde(default = "defaults::forge_binary")]
    pub binary: String,
}

impl Default for ForgeConfig {
    fn default() -> Self {
        Self {
            binary: defaults::forge_binary(),
        }
    }
}

#[derive(Debug, Deserialize)]
pub struct SttConfig {
    /// Which STT provider to use: "whisper_server", "whisper_cpp", "openai", or "deepgram".
    #[serde(default = "defaults::stt_provider")]
    pub provider: String,

    /// API key for cloud STT providers.
    /// Overridden by `AGENT2WEB_STT_API_KEY`.
    #[serde(default)]
    pub api_key: String,

    /// Settings specific to the local whisper-server provider.
    pub whisper_server: Option<WhisperServerConfig>,

    /// Settings for the whisper-cli subprocess provider.
    pub whisper_cpp: Option<WhisperCppConfig>,

    /// Settings for the OpenAI Whisper API provider.
    pub openai: Option<OpenAiSttConfig>,

    /// Settings for the Deepgram provider.
    pub deepgram: Option<DeepgramConfig>,
}

impl Default for SttConfig {
    fn default() -> Self {
        Self {
            provider: defaults::stt_provider(),
            api_key: String::new(),
            whisper_server: None,
            whisper_cpp: None,
            openai: None,
            deepgram: None,
        }
    }
}

#[derive(Debug, Deserialize)]
pub struct WhisperServerConfig {
    /// HTTP URL of the running whisper-server endpoint.
    #[serde(default = "defaults::whisper_server_url")]
    pub url: String,
}

/// Configuration for the `whisper_cpp` provider (subprocess-based).
#[derive(Debug, Deserialize)]
pub struct WhisperCppConfig {
    /// Path or name of the `whisper-cli` binary on `$PATH`.
    #[serde(default = "defaults::whisper_cpp_binary")]
    pub binary: String,

    /// Path to the GGML model file (e.g. `models/ggml-large-v3-turbo.bin`).
    pub model: String,

    /// BCP-47 language code or `"auto"` for automatic detection.
    #[serde(default = "defaults::whisper_cpp_language")]
    pub language: String,

    /// Number of CPU threads to pass to `whisper-cli` via `-t`.
    #[serde(default = "defaults::whisper_cpp_threads")]
    pub threads: u32,
}

#[derive(Debug, Deserialize)]
pub struct OpenAiSttConfig {
    /// OpenAI Whisper model name.
    #[serde(default = "defaults::openai_model")]
    pub model: String,
}

#[derive(Debug, Deserialize)]
pub struct DeepgramConfig {
    // Deepgram-specific settings can be added here.
}

#[derive(Debug, Deserialize)]
pub struct DiffConfig {
    /// Number of recent commit SHAs to track for the diff history UI.
    #[serde(default = "defaults::max_history")]
    pub max_history: usize,

    /// Lines of context in diff output.
    #[serde(default = "defaults::context_lines")]
    pub context_lines: usize,
}

impl Default for DiffConfig {
    fn default() -> Self {
        Self {
            max_history: defaults::max_history(),
            context_lines: defaults::context_lines(),
        }
    }
}

mod defaults {
    pub fn bind() -> String {
        "0.0.0.0:8080".to_string()
    }
    pub fn project_dir() -> String {
        ".".to_string()
    }
    pub fn run_timeout() -> u64 {
        600
    }
    pub fn forge_binary() -> String {
        "forge".to_string()
    }
    pub fn stt_provider() -> String {
        "whisper_server".to_string()
    }
    pub fn whisper_server_url() -> String {
        "http://127.0.0.1:8090/v1/audio/transcriptions".to_string()
    }
    pub fn whisper_cpp_binary() -> String {
        "whisper-cli".to_string()
    }
    pub fn whisper_cpp_language() -> String {
        "auto".to_string()
    }
    pub fn whisper_cpp_threads() -> u32 {
        4
    }
    pub fn openai_model() -> String {
        "whisper-1".to_string()
    }
    pub fn max_history() -> usize {
        20
    }
    pub fn context_lines() -> usize {
        5
    }
    pub fn tls_cert() -> String {
        let home = std::env::var("HOME").unwrap_or_else(|_| "/root".to_string());
        format!("{home}/.tls/cert.pem")
    }
    pub fn tls_key() -> String {
        let home = std::env::var("HOME").unwrap_or_else(|_| "/root".to_string());
        format!("{home}/.tls/key.pem")
    }
}

impl Config {
    /// Load configuration from a TOML file, then apply environment variable
    /// overrides for secrets.
    pub fn load(path: &str) -> anyhow::Result<Self> {
        let text = std::fs::read_to_string(path)
            .map_err(|e| anyhow::anyhow!("Cannot read config file '{}': {}", path, e))?;

        let mut config: Config = toml::from_str(&text)
            .map_err(|e| anyhow::anyhow!("Config parse error in '{}': {}", path, e))?;

        // Resolve paths in TLS config relative to the config file's directory.
        if let Some(ref mut tls) = config.server.tls
            && tls.enabled
        {
            let base = Path::new(path).parent().unwrap_or(Path::new("."));
            if !tls.cert.is_empty() && !Path::new(&tls.cert).is_absolute() {
                tls.cert = base.join(&tls.cert).to_string_lossy().into_owned();
            }
            if !tls.key.is_empty() && !Path::new(&tls.key).is_absolute() {
                tls.key = base.join(&tls.key).to_string_lossy().into_owned();
            }
        }

        config.apply_env_overrides();
        Ok(config)
    }

    /// Build a default configuration (no config file) and apply environment
    /// variable overrides for secrets.
    pub fn load_defaults() -> Self {
        let mut config = Self::default();
        config.apply_env_overrides();
        config
    }

    /// Apply environment variable overrides for secrets.
    fn apply_env_overrides(&mut self) {
        if let Ok(pwd) = std::env::var("AGENT2WEB_PASSWORD") {
            self.server.password = pwd;
        }
        if let Ok(key) = std::env::var("AGENT2WEB_STT_API_KEY") {
            self.stt.api_key = key;
        }
    }
}

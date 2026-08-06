//! 读 `providers.toml`。**任何路径上都不打印 `api_key` 本身**——`Debug` 手写，
//! 只吐长度；日志/CLI 要看 key 状态就调 [`ProviderConfig::key_len`]，不是拿字符串。
//!
//! 查找顺序、字段形状与 `providers.example.toml` 顶部注释一致：
//! `$AGENT_PROVIDERS_CONFIG` → `./providers.toml` → `~/.config/agent/providers.toml`。

use std::collections::BTreeMap;
use std::fmt;
use std::path::{Path, PathBuf};

use serde::Deserialize;

/// 整份配置文件：每家一段 `[providers.<name>]`，外加 `[default]` 指定用谁。
#[derive(Debug, Deserialize)]
pub struct RootConfig {
    pub providers: BTreeMap<String, ProviderConfig>,
    pub default: DefaultConfig,
    /// 持久化状态里可出现的 profile id 到进程受信 provider 配置的映射。
    /// 空表保持旧配置兼容；真实连接凭据只在启动时解析为运行时 binding，不会
    /// 被写进会话或提示词。
    #[serde(default)]
    pub execution_profiles: BTreeMap<String, String>,
}

impl RootConfig {
    /// 解析一个 durable profile id。未声明 profile 不是配置错误（调用方可按
    /// 默认 binding 执行）；声明后却指向不存在 provider 必须 fail closed。
    pub fn execution_profile(
        &self,
        profile: &str,
    ) -> Result<Option<(&str, &ProviderConfig)>, ConfigError> {
        let Some(provider_name) = self.execution_profiles.get(profile) else {
            return Ok(None);
        };
        let provider = self
            .providers
            .get(provider_name)
            .ok_or_else(|| ConfigError::UnknownExecutionProfileProvider(provider_name.clone()))?;
        Ok(Some((provider_name, provider)))
    }
}

#[derive(Debug, Deserialize)]
pub struct DefaultConfig {
    pub provider: String,
}

/// 一家 provider 的连接配置。字段以 `providers.example.toml` 实际写的为准；
/// 未在这里声明的字段（`cache_hit_discount`、`clear_thinking` 等）serde 默认
/// 忽略，不会导致解析失败。
#[derive(Deserialize)]
pub struct ProviderConfig {
    #[serde(default)]
    api_key: String,
    #[serde(default)]
    api_key_env: Option<String>,
    pub base_url: String,
    /// `prefix`/`partial` 续写要换的 base_url（DeepSeek 是 `/beta`）。
    /// 022 的最小 CLI 不做续写，这个字段先留着给以后接。
    #[serde(default)]
    pub beta_base_url: Option<String>,
    pub model: String,
}

impl ProviderConfig {
    /// 解出真正要用的 key。`api_key_env` 优先——让部署环境能覆盖开发机上
    /// 写在文件里的默认值。
    pub fn resolve_key(&self) -> Option<String> {
        if let Some(var) = &self.api_key_env
            && let Ok(v) = std::env::var(var)
            && !v.is_empty()
        {
            return Some(v);
        }
        if self.api_key.is_empty() {
            None
        } else {
            Some(self.api_key.clone())
        }
    }

    /// 给人看的 key 长度——**不是 key 本身**。日志/CLI 只许打印这个。
    pub fn key_len(&self) -> usize {
        self.resolve_key().map(|k| k.len()).unwrap_or(0)
    }

    /// OpenAI 兼容的 chat completions 路径。三家探针复用同一形状
    /// （probes/api/src/caps.rs `endpoint`），这里原样照抄。
    pub fn endpoint(&self) -> String {
        format!("{}/chat/completions", self.base_url.trim_end_matches('/'))
    }
}

/// **手写 `Debug`：绝不出现 `api_key` 的内容**，只报长度。派生的 `Debug` 会把
/// 字段值原样打出来，这个类型必须手写来堵住这条路（红线之外的仓内硬规矩，
/// 见 issue 022 「key 任何时候不打印」）。
impl fmt::Debug for ProviderConfig {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ProviderConfig")
            .field("base_url", &self.base_url)
            .field("beta_base_url", &self.beta_base_url)
            .field("model", &self.model)
            .field("api_key_env", &self.api_key_env)
            .field("api_key_len", &self.api_key.len())
            .finish()
    }
}

#[derive(Debug)]
pub enum ConfigError {
    /// 三个候选路径都没有文件。
    NotFound {
        tried: Vec<PathBuf>,
    },
    Io {
        path: PathBuf,
        message: String,
    },
    /// 不保存 parser 的原始文本：它可能连同 inline `api_key` 的源行一起回显。
    Parse {
        path: PathBuf,
    },
    /// `[default] provider` 指了一个 `[providers.*]` 里没有的名字。
    UnknownDefault(String),
    /// `[execution_profiles]` 指向的 provider 段不存在。
    UnknownExecutionProfileProvider(String),
}

impl fmt::Display for ConfigError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ConfigError::NotFound { tried } => {
                writeln!(f, "找不到 providers.toml，试过：")?;
                for p in tried {
                    writeln!(f, "  {}", p.display())?;
                }
                Ok(())
            }
            ConfigError::Io { path, message } => {
                write!(f, "{}: {message}", path.display())
            }
            ConfigError::Parse { path } => {
                write!(f, "{} 解析失败（配置格式或字段类型不正确）", path.display())
            }
            ConfigError::UnknownDefault(name) => {
                write!(
                    f,
                    "[default] provider = \"{name}\"，但 [providers.{name}] 不存在"
                )
            }
            ConfigError::UnknownExecutionProfileProvider(name) => {
                write!(
                    f,
                    "[execution_profiles] 指向 provider \"{name}\"，但 [providers.{name}] 不存在"
                )
            }
        }
    }
}

impl std::error::Error for ConfigError {}

/// 按查找顺序找第一个存在的 `providers.toml` 并解析。
pub fn load() -> Result<RootConfig, ConfigError> {
    load_from(&candidates())
}

/// `[default]` 指的那家。名字不存在时报 [`ConfigError::UnknownDefault`]，
/// 不是 `panic`/`unwrap`——配置写错是用户错误，不是程序 bug。
pub fn default_provider(root: &RootConfig) -> Result<&ProviderConfig, ConfigError> {
    root.providers
        .get(&root.default.provider)
        .ok_or_else(|| ConfigError::UnknownDefault(root.default.provider.clone()))
}

fn candidates() -> Vec<PathBuf> {
    [
        std::env::var("AGENT_PROVIDERS_CONFIG")
            .ok()
            .map(PathBuf::from),
        Some(PathBuf::from("providers.toml")),
        std::env::var("HOME")
            .ok()
            .map(|h| PathBuf::from(h).join(".config/agent/providers.toml")),
    ]
    .into_iter()
    .flatten()
    .collect()
}

fn load_from(candidates: &[PathBuf]) -> Result<RootConfig, ConfigError> {
    for path in candidates {
        if path.is_file() {
            return parse_file(path);
        }
    }
    Err(ConfigError::NotFound {
        tried: candidates.to_vec(),
    })
}

fn parse_file(path: &Path) -> Result<RootConfig, ConfigError> {
    let text = std::fs::read_to_string(path).map_err(|e| ConfigError::Io {
        path: path.to_path_buf(),
        message: e.to_string(),
    })?;
    toml::from_str(&text).map_err(|_| ConfigError::Parse {
        path: path.to_path_buf(),
    })
}

#[cfg(test)]
#[path = "config_tests.rs"]
mod tests;

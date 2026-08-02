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
        if self.api_key.is_empty() { None } else { Some(self.api_key.clone()) }
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
    NotFound { tried: Vec<PathBuf> },
    Io { path: PathBuf, message: String },
    Parse { path: PathBuf, message: String },
    /// `[default] provider` 指了一个 `[providers.*]` 里没有的名字。
    UnknownDefault(String),
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
            ConfigError::Parse { path, message } => {
                write!(f, "{} 解析失败: {message}", path.display())
            }
            ConfigError::UnknownDefault(name) => {
                write!(f, "[default] provider = \"{name}\"，但 [providers.{name}] 不存在")
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
        std::env::var("AGENT_PROVIDERS_CONFIG").ok().map(PathBuf::from),
        Some(PathBuf::from("providers.toml")),
        std::env::var("HOME").ok().map(|h| PathBuf::from(h).join(".config/agent/providers.toml")),
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
    Err(ConfigError::NotFound { tried: candidates.to_vec() })
}

fn parse_file(path: &Path) -> Result<RootConfig, ConfigError> {
    let text = std::fs::read_to_string(path)
        .map_err(|e| ConfigError::Io { path: path.to_path_buf(), message: e.to_string() })?;
    toml::from_str(&text)
        .map_err(|e| ConfigError::Parse { path: path.to_path_buf(), message: e.to_string() })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use std::sync::atomic::{AtomicU32, Ordering};

    /// 临时文件，`Drop` 时自己删——不引入 `tempfile` 依赖，标准库够用。
    struct TempFile(PathBuf);
    impl Drop for TempFile {
        fn drop(&mut self) {
            let _ = std::fs::remove_file(&self.0);
        }
    }

    fn write_temp(contents: &str) -> (TempFile, PathBuf) {
        static COUNTER: AtomicU32 = AtomicU32::new(0);
        let n = COUNTER.fetch_add(1, Ordering::Relaxed);
        let path = std::env::temp_dir()
            .join(format!("agent-transport-test-{}-{n}.toml", std::process::id()));
        let mut f = std::fs::File::create(&path).unwrap();
        f.write_all(contents.as_bytes()).unwrap();
        (TempFile(path.clone()), path)
    }

    const FAKE_KEY: &str = "sk-super-secret-do-not-print-12345";

    fn sample_toml() -> String {
        format!(
            r#"
[providers.deepseek]
api_key = "{FAKE_KEY}"
base_url = "https://api.deepseek.com"
beta_base_url = "https://api.deepseek.com/beta"
model = "deepseek-v4-pro"

[providers.kimi]
api_key_env = "MOONSHOT_API_KEY"
base_url = "https://api.moonshot.cn/v1"
model = "kimi-k3"

[default]
provider = "deepseek"
"#
        )
    }

    /// 红线之外的仓内硬规矩：`Debug` 输出绝不含 key 本身，只报长度。
    #[test]
    fn debug_output_never_contains_the_key() {
        let root: RootConfig = toml::from_str(&sample_toml()).unwrap();
        let ds = &root.providers["deepseek"];
        let dump = format!("{ds:?}");
        assert!(!dump.contains(FAKE_KEY), "Debug 输出泄漏了 key: {dump}");
        assert!(dump.contains(&FAKE_KEY.len().to_string()), "该报长度: {dump}");
        // RootConfig 整体的 Debug（走派生）也一样干净——因为它内部用的还是
        // ProviderConfig 手写的 Debug。
        let root_dump = format!("{root:?}");
        assert!(!root_dump.contains(FAKE_KEY));
    }

    #[test]
    fn resolve_key_prefers_env_over_inline() {
        let root: RootConfig = toml::from_str(&sample_toml()).unwrap();
        assert_eq!(root.providers["deepseek"].resolve_key().as_deref(), Some(FAKE_KEY));

        // api_key_env 指向的变量没设时，回落到「未配置」，不会误读别的变量。
        let kimi = &root.providers["kimi"];
        // SAFETY: 测试进程内单线程设置，不与其它测试的同名变量冲突
        // （变量名 MOONSHOT_API_KEY 只在这条测试里出现）。
        unsafe {
            std::env::remove_var("MOONSHOT_API_KEY");
        }
        assert_eq!(kimi.resolve_key(), None);
        assert_eq!(kimi.key_len(), 0);

        unsafe {
            std::env::set_var("MOONSHOT_API_KEY", "env-value-xyz");
        }
        assert_eq!(kimi.resolve_key().as_deref(), Some("env-value-xyz"));
        unsafe {
            std::env::remove_var("MOONSHOT_API_KEY");
        }
    }

    #[test]
    fn default_provider_resolves_the_configured_name() {
        let root: RootConfig = toml::from_str(&sample_toml()).unwrap();
        let p = default_provider(&root).unwrap();
        assert_eq!(p.model, "deepseek-v4-pro");
    }

    #[test]
    fn default_provider_errors_on_unknown_name() {
        let mut root: RootConfig = toml::from_str(&sample_toml()).unwrap();
        root.default.provider = "not-there".to_string();
        let err = default_provider(&root).unwrap_err();
        assert!(matches!(err, ConfigError::UnknownDefault(n) if n == "not-there"));
    }

    #[test]
    fn endpoint_appends_chat_completions_and_trims_trailing_slash() {
        let root: RootConfig = toml::from_str(&sample_toml()).unwrap();
        assert_eq!(
            root.providers["deepseek"].endpoint(),
            "https://api.deepseek.com/chat/completions"
        );
    }

    /// `load_from` 按顺序取第一个存在的文件；不存在的候选路径被跳过，不报错。
    #[test]
    fn load_from_picks_first_existing_candidate() {
        let (_guard, path) = write_temp(&sample_toml());
        let missing = PathBuf::from("/definitely/does/not/exist/providers.toml");
        let root = load_from(&[missing, path]).unwrap();
        assert_eq!(root.default.provider, "deepseek");
    }

    #[test]
    fn load_from_reports_all_tried_paths_when_none_exist() {
        let missing = PathBuf::from("/definitely/does/not/exist/providers.toml");
        let err = load_from(std::slice::from_ref(&missing)).unwrap_err();
        match err {
            ConfigError::NotFound { tried } => assert_eq!(tried, vec![missing]),
            other => panic!("期望 NotFound，拿到 {other:?}"),
        }
    }
}

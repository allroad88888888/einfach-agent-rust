//! 「已经解析好的配置」这个中间形态（issue 114d）：`RootConfig` / `ProviderConfig`
//! 连同它们的 `ConfigError`。**两种来源都汇到这里，产出同一个类型**：
//!
//! - native：`config.rs` 从 `providers.toml` 读文本，`toml::from_str::<RootConfig>`
//!   解析（那部分逻辑一行没动，只是这个文件把类型定义搬出来给两边共用）。
//! - 没有文件系统的宿主（wasm 页面）：[`ProviderConfig::from_host`] /
//!   [`RootConfig::from_host`] 直接构造，或者对同一个 `#[derive(Deserialize)]`
//!   类型喂一份宿主传来的 JSON（`serde_json::from_str`，两边都已经依赖
//!   `serde_json`，不需要新增）。
//!
//! 不让 wasm 侧另起一套配置类型——两份配置结构分叉之后，「native 能跑 wasm
//! 不能」的排查会变成噩梦（111 决策原话）。所以这个文件本身**平台无关**：
//! 不碰 `std::fs`、不依赖 `toml` crate，native/wasm32 两个目标都编译。
//!
//! `api_key` 只在内存里活着，**任何路径上都不打印 key 本身**——[`ProviderConfig`]
//! 的 `Debug` 是手写的、只吐长度，见下面 `impl fmt::Debug`；新增的两个 `from_host`
//! 构造器复用同一个结构体，不会绕开这条规矩。

use std::collections::BTreeMap;
use std::fmt;
use std::path::PathBuf;

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
    /// 宿主直接给一份配置（114d）：没有 `providers.toml` 可读的环境（wasm 页面）
    /// 用这个入口。产出的仍是 `RootConfig`——跟 `toml::from_str::<RootConfig>`
    /// 解析出的值同一个类型，[`RootConfig::execution_profile`]、自由函数
    /// [`default_provider`] 原样可用，不需要为宿主注入另开一条装配路径。
    /// `execution_profiles` 留空：那是 durable profile id 的映射，宿主注入这
    /// 条路径目前不涉及。
    pub fn from_host(
        default_provider: String,
        providers: BTreeMap<String, ProviderConfig>,
    ) -> Self {
        RootConfig {
            providers,
            default: DefaultConfig {
                provider: default_provider,
            },
            execution_profiles: BTreeMap::new(),
        }
    }

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
    /// 上下文窗口大小，单位 token。三家各自的实测值与查证日期见
    /// `providers.example.toml` 对应段落的「窗口」条目。这个值原样喂给
    /// `agent_core::SessionConfig::context_window`——压缩触发在 core 端只拿它
    /// 做纯算术比较（红线 12：这里是参数不是分支）。
    ///
    /// `#[serde(default)]` → 缺省 `None`：没写这个键的旧 `providers.toml`
    /// 照常加载，`None` 一路走到触发逻辑就是「不触发」（`value/session.rs`
    /// 该字段的文档），不是加载失败，也不是某个隐藏默认窗口。
    #[serde(default)]
    pub context_window: Option<u32>,
}

impl ProviderConfig {
    /// 宿主直接构造一份 provider 配置（114d）：绕开 `api_key_env`——浏览器
    /// 没有进程环境变量可读，`api_key` 就是最终答案（`resolve_key` 原样生效，
    /// 只是 `api_key_env` 分支永远不命中）。跟 toml 解析出的 `ProviderConfig`
    /// 是同一个类型，`resolve_key`/`key_len`/`endpoint` 全部直接可用。
    pub fn from_host(base_url: String, model: String, api_key: String) -> Self {
        ProviderConfig {
            api_key,
            api_key_env: None,
            base_url,
            beta_base_url: None,
            model,
            context_window: None,
        }
    }

    /// 补一个续写用的 `beta_base_url`。toml 路径靠字段直接反序列化拿到；
    /// [`ProviderConfig::from_host`] 没有对应的构造参数，用这个 builder 补上。
    pub fn with_beta_base_url(mut self, beta_base_url: String) -> Self {
        self.beta_base_url = Some(beta_base_url);
        self
    }

    /// 补一个上下文窗口。跟 [`ProviderConfig::with_beta_base_url`] 同款理由：
    /// toml 路径靠字段反序列化拿到，宿主注入这条（114d）没有对应的构造参数。
    /// **不补就等于 `None` = 自动压缩阶梯永不触发**（M12 的 108），所以浏览器
    /// 宿主想要压缩就得调它——这是 M13 合并 M12 时才出现的接缝，两边单独看都
    /// 没有这个组合。
    pub fn with_context_window(mut self, context_window: u32) -> Self {
        self.context_window = Some(context_window);
        self
    }

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
/// 见 issue 022 「key 任何时候不打印」）。`from_host` 构造出来的实例走的是
/// 同一个 `impl`，不存在绕过这条规矩的第二条路。
impl fmt::Debug for ProviderConfig {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ProviderConfig")
            .field("base_url", &self.base_url)
            .field("beta_base_url", &self.beta_base_url)
            .field("model", &self.model)
            .field("context_window", &self.context_window)
            .field("api_key_env", &self.api_key_env)
            .field("api_key_len", &self.api_key.len())
            .finish()
    }
}

#[derive(Debug)]
pub enum ConfigError {
    /// 三个候选路径都没有文件。只有 native 的文件查找会构造这个变体，但类型
    /// 本身两个目标都编——枚举定义齐全比按平台裁剪枚举变体更不容易出岔子。
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

/// `[default]` 指的那家。名字不存在时报 [`ConfigError::UnknownDefault`]，
/// 不是 `panic`/`unwrap`——配置写错是用户错误，不是程序 bug。native/宿主注入
/// 两条来源共用这一个函数：装配起点相同，不会走出两条分叉的路。
pub fn default_provider(root: &RootConfig) -> Result<&ProviderConfig, ConfigError> {
    root.providers
        .get(&root.default.provider)
        .ok_or_else(|| ConfigError::UnknownDefault(root.default.provider.clone()))
}

#[cfg(test)]
#[path = "provider_config_tests.rs"]
mod tests;

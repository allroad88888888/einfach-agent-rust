//! [`bootstrap`]：读 `providers.toml`（`agent_transport::config` 的三级查找
//! 顺序：`$AGENT_PROVIDERS_CONFIG` → `./providers.toml` →
//! `~/.config/agent/providers.toml`）、按 `[default] provider` 解出具体
//! adapter（[`crate::provider_dispatch::resolve_provider`]）、拼出一份可以
//! 直接喂给 `AgentServer::new`/`ServerConfig::new` 的 [`SessionTemplate`]。
//!
//! 这条装配线原本各个宿主（`examples/serve.rs`、035 的 `agent-server-bin`，
//! 以及 036 打算做的桌面内嵌）各抄一遍——读配置、查 key、拼 `SessionTemplate`
//! 的八九个字段、错误文案还得保持一致。提成库函数是 035 issue 原文「注意」
//! 条目原话：「bin 里重复的装配逻辑若超过三十行，提库函数（`agent_server::
//! bootstrap` 之类），example 一起换用」。
//!
//! # 这个函数管什么，不管什么
//!
//! **管**：读配置文件、选 provider、查 key、拼 [`SessionTemplate`] 里「必须
//! 读配置文件才知道」的五个字段（`provider`/`endpoint`/`upload_base_url`/
//! `api_key`/`model`）。
//!
//! **不管**：`--sessions-dir`/`--config`/`--port` 这类命令行参数怎么解析——
//! 各宿主的参数形状不同（CLI flag、环境变量、桌面壳的配置文件/平台标准目录），
//! 不该被这一层收窄成一种形状。工具表开哪一档（[`ToolTableSpec`]）、system
//! prompt 写什么、快照节奏——这些是产品判断，由调用方经 [`BootstrapOptions`]
//! 显式传入，这个函数只管「填进去」，不替调用方决定值。
//!
//! `--config <path>` 的落地方式：`agent_transport::config::load` 已经认
//! `AGENT_PROVIDERS_CONFIG` 环境变量、且优先于 `./providers.toml`（该模块文档
//! 的三级查找顺序）——宿主收到 `--config <path>` 时把它写进这个环境变量再调
//! [`bootstrap`]，不需要这一层再单开一个参数通道。`agent-server-bin`/
//! `examples/serve.rs` 都这么用。

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use agent_core::{ExecutionProfileId, SessionConfig, SystemChunk};
use agent_runtime::ExecutionBinding;
use agent_transport::{Client, config};

use crate::SessionTemplate;
use crate::provider_dispatch::resolve_provider;
use crate::registry::ToolTableSpec;

/// [`bootstrap`] 的输入：跟 provider 无关、调用方必须自己决定的那部分
/// `SessionTemplate` 字段——字段形状直接照抄 `SessionTemplate` 减去
/// `provider`/`endpoint`/`upload_base_url`/`api_key`/`model` 那五个（那五个只能从配置文件解出，
/// 不该由调用方伪造）。
pub struct BootstrapOptions {
    /// 内置工具路径监狱的根目录——`SessionTemplate::tools_root` 原样转发。
    pub tools_root: PathBuf,
    /// `POST /sessions` 不给 `session_path` 时自动落盘的目录，`None` 保持
    /// 「退回 `Memory`」的旧行为——`SessionTemplate::default_sessions_dir`
    /// 原样转发，字段文档有完整语义。
    pub default_sessions_dir: Option<PathBuf>,
    pub tools: ToolTableSpec,
    pub system: Vec<SystemChunk>,
    pub client: Arc<Client>,
    pub history_cap: Option<usize>,
    pub snapshot_every: Option<u64>,
    pub provider_timeout: Option<Duration>,
    /// 前端/桌面端远程工具被领取后的结果等待上限；`None` 使用运行时默认值。
    pub remote_tool_timeout: Option<Duration>,
}

/// 装配失败的三类原因，判据顺序跟 `agent-cli`/`examples/serve.rs` 原来各自
/// 手写的失败分支一致——改用这个函数不该打散宿主的错误提示文案。
#[derive(Debug)]
pub enum BootstrapError {
    /// 找不到/读不了/解不了 `providers.toml`，或者 `[default] provider` 指了
    /// 一个不存在的段名。
    Config(config::ConfigError),
    /// `[default] provider` 的名字没有对应的 adapter（`resolve_provider` 的
    /// 错误文案已经列出可选值）。
    UnknownProvider(String),
    /// provider 段既没写 `api_key` 也没有 `api_key_env` 指向的环境变量。
    MissingApiKey,
    /// execution profile 指向了当前进程没有编译进来的 adapter。
    UnknownExecutionProfileProvider { profile: String, message: String },
    /// execution profile 的 provider 没有可用 key；启动时拒绝，不留到子 agent
    /// 真正请求 provider 时才变成含糊的失败。
    MissingExecutionProfileApiKey(String),
    /// 固定的视觉 profile 必须绑定到明确支持图片的 adapter。
    VisionExecutionProfileRequiresImages { provider: String },
}

impl std::fmt::Display for BootstrapError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            BootstrapError::Config(e) => write!(f, "配置加载失败: {e}"),
            BootstrapError::UnknownProvider(msg) => write!(f, "{msg}"),
            BootstrapError::MissingApiKey => write!(
                f,
                "provider 没配 key：检查 providers.toml 里的 api_key，或对应的 api_key_env 指向的环境变量"
            ),
            BootstrapError::UnknownExecutionProfileProvider { profile, message } => {
                write!(
                    f,
                    "execution profile \"{profile}\" 的 provider 无法使用: {message}"
                )
            }
            BootstrapError::MissingExecutionProfileApiKey(profile) => write!(
                f,
                "execution profile \"{profile}\" 的 provider 没配 key：检查 providers.toml 里的 api_key，或对应的 api_key_env 指向的环境变量"
            ),
            BootstrapError::VisionExecutionProfileRequiresImages { provider } => write!(
                f,
                "execution profile \"vision\" 的 provider \"{provider}\" 不支持图片输入"
            ),
        }
    }
}

impl std::error::Error for BootstrapError {}

/// 装配结果：`template` 直接喂 `ServerConfig::new`/`AgentServer::new`；
/// `provider_name` 是 `[default] provider` 的原始名字（`template.model` 已经
/// 是解出来的模型名，两个凑在一起够宿主拼一条启动横幅，不用另外再读一遍
/// `providers.toml`）。
pub struct Bootstrapped {
    pub template: SessionTemplate,
    pub provider_name: String,
    /// 已解析的 live bindings；只随 server 进程存活，绝不进入 `SessionTemplate`
    /// 或 durable session state。
    pub execution_bindings: BTreeMap<ExecutionProfileId, ExecutionBinding>,
}

/// 读 `providers.toml` → 选 `[default]` provider → 拼 [`SessionTemplate`]。
/// 三类失败见 [`BootstrapError`]。
pub fn bootstrap(options: BootstrapOptions) -> Result<Bootstrapped, BootstrapError> {
    let root = config::load().map_err(BootstrapError::Config)?;
    let provider_cfg = config::default_provider(&root).map_err(BootstrapError::Config)?;
    let provider_name = root.default.provider.clone();
    let provider = resolve_provider(&provider_name).map_err(BootstrapError::UnknownProvider)?;
    let api_key = provider_cfg
        .resolve_key()
        .ok_or(BootstrapError::MissingApiKey)?;
    let execution_bindings =
        resolve_execution_bindings(&root, &options.client, options.provider_timeout)?;

    Ok(Bootstrapped {
        template: SessionTemplate {
            provider,
            upload_base_url: provider_cfg.base_url.clone(),
            endpoint: provider_cfg.endpoint(),
            api_key,
            model: Arc::from(provider_cfg.model.as_str()),
            tools: options.tools,
            tools_root: options.tools_root,
            default_sessions_dir: options.default_sessions_dir,
            system: options.system,
            client: options.client,
            history_cap: options.history_cap,
            snapshot_every: options.snapshot_every,
            provider_timeout: options.provider_timeout,
            remote_tool_timeout: options.remote_tool_timeout,
        },
        provider_name,
        execution_bindings,
    })
}

/// 仅在启动时把配置中的 durable id 解析成活 provider 资源。映射一旦写了错
/// provider、缺 adapter 或缺 key，整个 server 都拒绝启动；不能让会话随后静默
/// 回落到 default provider。
fn resolve_execution_bindings(
    root: &config::RootConfig,
    client: &Arc<Client>,
    provider_timeout: Option<Duration>,
) -> Result<BTreeMap<ExecutionProfileId, ExecutionBinding>, BootstrapError> {
    let mut bindings = BTreeMap::new();
    for profile_name in root.execution_profiles.keys() {
        let (provider_name, provider_config) = root
            .execution_profile(profile_name)
            .map_err(BootstrapError::Config)?
            .expect("enumerated execution profile must resolve to itself");
        let provider = resolve_provider(provider_name).map_err(|message| {
            BootstrapError::UnknownExecutionProfileProvider {
                profile: profile_name.clone(),
                message,
            }
        })?;
        if profile_name == "vision" && !provider.supports_images() {
            return Err(BootstrapError::VisionExecutionProfileRequiresImages {
                provider: provider_name.to_owned(),
            });
        }
        let api_key = provider_config
            .resolve_key()
            .ok_or_else(|| BootstrapError::MissingExecutionProfileApiKey(profile_name.clone()))?;
        let binding = ExecutionBinding::new(
            provider,
            Arc::clone(client),
            provider_config.endpoint(),
            api_key,
            SessionConfig {
                model: Arc::from(provider_config.model.as_str()),
                temperature: None,
                max_tokens: None,
                context_window: None,
            },
        )
        .with_image_upload_base_url(provider_config.base_url.clone());
        let binding = match provider_timeout {
            Some(timeout) => binding.with_timeout(timeout),
            None => binding,
        };
        bindings.insert(ExecutionProfileId::new(profile_name.as_str()), binding);
    }
    Ok(bindings)
}

#[cfg(test)]
#[path = "bootstrap_execution_bindings_tests.rs"]
mod tests;

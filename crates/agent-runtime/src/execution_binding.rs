//! 093 的活执行绑定：把 durable 的不透明 profile id 解析为本进程的 provider 资源。
//!
//! `Session` 只持久化 [`ExecutionProfileId`]；provider、key、client 和超时都留在
//! 这个 runtime registry。每次调用由 [`RunnerCtx::execution_binding_for`] 克隆一份
//! binding，此后默认 provider 的切换不会追溯影响已在飞的调用。

use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::Duration;

use agent_core::cache::TurnHit;
use agent_core::{ExecutionProfileId, SessionConfig};
use agent_providers::Provider;
use agent_transport::Client;

use crate::ctx::{DEFAULT_PROVIDER_TIMEOUT, RunnerCtx};

/// 一条已由服务端授权的 provider 执行绑定。
///
/// 它不实现 `Debug`，以免 API key 经日志或错误输出泄露。`RunnerCtx::new` 建默认
/// binding；服务端为具名 profile 构造本值并以 [`RunnerCtx::with_execution_bindings`]
/// 注入。
#[derive(Clone)]
pub struct ExecutionBinding {
    pub(crate) provider: Arc<dyn Provider>,
    pub(crate) client: Arc<Client>,
    pub(crate) endpoint: String,
    pub(crate) api_key: String,
    pub(crate) session_config: SessionConfig,
    pub(crate) timeout: Duration,
}

/// 绑定实例的不可复用 guard 窗口标识。
///
/// 它不是 durable profile id：profile 只说明该 agent 该选哪条已授权 binding；这个
/// scope 说明一趟已起飞请求完成时该写回哪个 guard 窗口。默认 provider 每次切换都
/// 分配新 scope，因而旧 in-flight 请求绝不会污染新 provider 的缓存观测。
#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub(crate) struct GuardScope(u64);

impl GuardScope {
    pub(crate) const INITIAL: Self = Self(0);
    pub(crate) const FIRST_DYNAMIC: u64 = 1;
}

/// 一次 provider 调用在起飞时固定下来的 binding 与 guard scope。
pub(crate) struct ExecutionSelection {
    pub(crate) binding: ExecutionBinding,
    pub(crate) guard_scope: GuardScope,
}

impl ExecutionBinding {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        provider: Arc<dyn Provider>,
        client: Arc<Client>,
        endpoint: String,
        api_key: String,
        session_config: SessionConfig,
    ) -> Self {
        Self {
            provider,
            client,
            endpoint,
            api_key,
            session_config,
            timeout: DEFAULT_PROVIDER_TIMEOUT,
        }
    }

    /// 为这条 binding 设置单次 provider 调用的总超时。
    pub fn with_timeout(mut self, timeout: Duration) -> Self {
        self.timeout = timeout;
        self
    }
}

#[derive(Debug)]
pub(crate) struct MissingExecutionBinding;

impl RunnerCtx {
    /// 注入由服务端解析并授权的具名 execution profile 绑定。
    ///
    /// 重复 id 由后写值替换；调用者应在配置装配处保证映射的可信来源。默认 binding
    /// 不在此表中，profile 缺省的 root/旧会话始终走默认 binding。
    pub fn with_execution_bindings(
        mut self,
        bindings: BTreeMap<ExecutionProfileId, ExecutionBinding>,
    ) -> Self {
        let mut next_guard_scope = GuardScope::FIRST_DYNAMIC;
        let execution_guard_scopes = bindings
            .keys()
            .cloned()
            .map(|profile| {
                let scope = GuardScope(next_guard_scope);
                next_guard_scope += 1;
                (profile, scope)
            })
            .collect();
        self.execution_bindings = bindings;
        self.execution_guard_scopes = execution_guard_scopes;
        self.next_guard_scope = next_guard_scope;
        self
    }

    pub(crate) fn execution_binding_for(
        &self,
        profile: Option<&ExecutionProfileId>,
    ) -> Result<ExecutionSelection, MissingExecutionBinding> {
        match profile {
            Some(profile) => self
                .execution_bindings
                .get(profile)
                .cloned()
                .zip(self.execution_guard_scopes.get(profile).copied())
                .map(|(binding, guard_scope)| ExecutionSelection {
                    binding,
                    guard_scope,
                })
                .ok_or(MissingExecutionBinding),
            None => Ok(ExecutionSelection {
                binding: self.default_binding.clone(),
                guard_scope: self.default_guard_scope,
            }),
        }
    }

    pub(crate) fn guard_history_for(&mut self, scope: GuardScope) -> &mut Vec<TurnHit> {
        self.guard_histories.entry(scope).or_default()
    }

    /// 覆盖默认 binding 的超时。具名 profile 的 timeout 由各自的
    /// [`ExecutionBinding::with_timeout`] 固化，不会被这一默认设置追溯改写。
    pub fn with_provider_timeout(mut self, timeout: Duration) -> Self {
        self.default_binding.timeout = timeout;
        self
    }

    /// 运行时切 provider（014 `/model <name>`）。只替换默认 binding，并为新
    /// binding 分配新 guard scope；旧请求仍可安全地写回自己起飞时的 scope。具名
    /// profile 的已授权绑定和它们独立的 guard 窗口均保留。
    pub fn switch_provider(
        &mut self,
        provider: Arc<dyn Provider>,
        endpoint: String,
        api_key: String,
        model: Arc<str>,
    ) {
        self.default_binding.provider = provider;
        self.default_binding.endpoint = endpoint;
        self.default_binding.api_key = api_key;
        self.default_binding.session_config.model = model;
        self.default_guard_scope = GuardScope(self.next_guard_scope);
        self.next_guard_scope += 1;
    }
}

#[cfg(test)]
#[path = "execution_binding_tests.rs"]
mod tests;

//! `Effect::CallProvider` 的执行点，拆成**起飞**（[`start`]）与**落地**
//! （[`finish`]）两半。
//!
//! # 为什么拆开：029 的并行就是这一刀
//!
//! 012 时它是一个函数：取料 → `encode` → 起 IO 线程 → 就地 `recv_timeout` 等到
//! 底 → 返回一个 `Event`。「等到底」在单 agent 下没有代价，多 agent 下就是把并行
//! 掐死的那一句——root 和两个子 agent 的调用会一个接一个地跑完。
//!
//! 拆开之后：`start` 只做「在 actor 线程上能做完的部分」（取料、`encode`、发前
//! 第 1 层判读、起线程）并返回一张**在飞凭据** [`ProviderCall`]；等谁先回来、
//! 谁超时了归泵（`crate::runner`）统一管；某一个回来了再调 `finish` 把它翻译成
//! 一个 loop 事件。凭据里装的全是「起飞时就定了、落地时才用得上」的东西
//! （第 1 层判读结论、预测命中、adjustments、这次请求的前缀镜像）——它们必须是
//! 起飞那一刻的值，不能等落地时回头再算一遍。
//!
//! # 超时/取消之后：锁存本次调用的取消，不 join
//!
//! 每张 [`ProviderCall`] 都有一枚独立、只会从 `false` 变成 `true` 的取消标志。
//! session 取消和截止线都会先锁存它，再划掉在飞凭据；后续轮次即使重置 session
//! 标志，也不能让这次已放弃的请求复活。请求准备会在每次上传前、两张图片之间和
//! 构造 chat body 前观察这枚 call-local 标志。已经进入稳定同步上传 API 的请求允许
//! 物理完成；完成后若已取消，不会继续下一张上传或 chat。
//!
//! 这里仍然不保留、也不 join IO 线程。runner 在超时后放弃 `(agent, attempt)` 凭据，
//! 所以已开始上传的迟到结果无法回写、重试或重新进入本次调用。

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::SyncSender;
use std::time::Instant;

use agent_core::cache::{self, PrefixIntent};
use agent_core::{
    Adjustment, AgentId, DriftVerdict, Epoch, ErrorClass, Event, PrefixImage, RequestIntent,
    Segment, Session,
};
use agent_providers::{Encoded, Ingredients};

use crate::ctx::RunnerCtx;
use crate::event::RunnerEvent;
use crate::execution_binding::{ExecutionBinding, GuardScope};
use crate::io_thread::{self, IoMsg};
use crate::provider_attempt::ProviderAttemptId;
use crate::subagent;

pub(crate) use crate::provider_call_finish::{
    finish, preparation_failed, preparation_start_failed, thread_gone,
};

/// 一次仍由 runner 认领的 provider 调用。**一个 agent 同时最多一张凭据**；超时
/// 划掉的旧 IO 线程可以与重试短暂重叠，但它持有不同的 [`ProviderAttemptId`]。
pub(crate) struct ProviderCall {
    pub(crate) agent: AgentId,
    /// One transport launch. Retries may share an epoch, so epoch cannot correlate IO replies.
    pub(crate) attempt: ProviderAttemptId,
    /// 起飞时的世代。落地时原样带回事件里过闸（红线 6）。
    pub(crate) epoch: Epoch,
    /// 到点就判超时。每个调用各自一份——两个子 agent 不共享同一条截止线。
    pub(crate) deadline: Instant,
    pub(crate) binding: ExecutionBinding,
    /// 起飞时固定的 binding guard scope；落地只能写回这里，不能按当前默认
    /// provider 或 profile 重新查找。
    pub(crate) guard_scope: GuardScope,
    pub(crate) drift: DriftVerdict,
    pub(crate) predicted_cache: u32,
    pub(crate) adjustments: Vec<Adjustment>,
    pub(crate) prefix: PrefixImage,
    /// This request consumed transient source material and must never be retried.
    pub(crate) one_shot: bool,
    /// Calls whose raw stream may echo request-local image references suppress live deltas.
    /// Direct visual output is replayed after terminal scrubbing; vision output returns through
    /// its stable parent Tool envelope.
    pub(crate) hold_deltas: bool,
    /// Direct visual calls replay their terminal deltas only after exact request-local references
    /// have been scrubbed. Vision children stay behind their stable parent Tool envelope.
    pub(crate) replay_sanitized_deltas: bool,
    /// Vision runs behind a stable Tool envelope, so raw upstream failures must stay private.
    pub(crate) redact_provider_errors: bool,
    /// Per-attempt cancellation latch. Unlike the session flag, this is never reset by a later
    /// turn, so an abandoned upload cannot resume after the runner starts another turn.
    cancel_token: Arc<AtomicBool>,
}

impl ProviderCall {
    /// Permanently cancel this attempt. There is intentionally no inverse operation.
    pub(crate) fn cancel(&self) {
        self.cancel_token.store(true, Ordering::Relaxed);
    }
}

/// 起飞：取料 → `encode` → 发前第 1 层 → 起 IO 线程。**不等结果。**
pub(crate) fn start(
    session: &Session,
    ctx: &mut RunnerCtx,
    tx: SyncSender<IoMsg>,
    agent: AgentId,
    epoch: Epoch,
) -> Result<ProviderCall, Event> {
    ctx.clear_image_preparation_failure(&agent);
    let profile = session.execution_profile_of(&agent);
    let redact_provider_errors = profile.as_ref().is_some_and(crate::vision_tool::is_profile);
    let selection = match ctx.execution_binding_for(profile.as_ref()) {
        Ok(selection) => selection,
        Err(_) => {
            return Err(Event::ProviderFailed {
                agent,
                epoch,
                class: ErrorClass::Unknown,
                message: Arc::from("execution profile is not configured"),
            });
        }
    };
    let binding = selection.binding;
    // 宿主从状态取料（012，027 换成 `Session` 的读口，029 换成 per-agent 的那
    // 一版）：`imbl::Vector` 不是连续切片，Ingredients 要的是 `&[Message]`，
    // 这里物化一份——克隆的是 `Message`（里面全是 `Arc`），代价是指针拷贝乘
    // 消息数，不是深拷内容（红线 5）。
    let durable_messages: Vec<agent_core::Message> =
        session.messages_of(&agent).iter().cloned().collect();
    let prepared = match crate::transient_source_prompt::prepare(
        &durable_messages,
        &mut ctx.transient_sources,
        &agent,
        epoch,
    ) {
        Ok(prepared) => prepared,
        Err(()) => {
            ctx.transient_sources.purge_agent_epoch(&agent, epoch);
            return Err(crate::transient_source_completion::failure(agent, epoch));
        }
    };
    let prev_prefix = session.prev_prefix_of(&agent);
    let system = subagent::system_for(session, ctx, &agent);
    let tools = subagent::tools_for(session, ctx, &agent);
    // 039：这个 agent 当前激活的 skill 展开成本轮注入——正文进 `late_system`、
    // 携带的工具进 `late_tools`。空激活集 → 两个都空，`Ingredients` 逐字节回到
    // 039 之前（向后兼容）。索引不在这里，它常驻在 `ctx.system`（宿主放进去的）。
    let active = session.active_skills_of(&agent);
    let (late_system, late_tools) = ctx.tools.skill_injection(&active);
    let ing = Ingredients {
        system: &system,
        messages: &prepared.messages,
        tools: &tools,
        late_tools: &late_tools,
        late_system: &late_system,
        config: &binding.session_config,
        intent: RequestIntent::Free,
        prev_prefix: prev_prefix.as_ref(),
    };
    let Encoded {
        body,
        mut prefix,
        mut drift,
        mut predicted_cache,
        mut adjustments,
    } = binding.provider.encode(&ing);
    if prepared.one_shot {
        // Prefix mirrors, drift reports and cache predictions are durable/public metadata.
        // Re-encode the placeholder history locally so none of them fingerprints the raw
        // one-shot overlay; only the first body's bytes are sent to the provider.
        let safe = binding.provider.encode(&Ingredients {
            system: &system,
            messages: &durable_messages,
            tools: &tools,
            late_tools: &late_tools,
            late_system: &late_system,
            config: &binding.session_config,
            intent: RequestIntent::Free,
            prev_prefix: prev_prefix.as_ref(),
        });
        prefix = safe.prefix;
        drift = safe.drift;
        predicted_cache = safe.predicted_cache;
        adjustments = safe.adjustments;
    }

    // 兜底第 1 层：发前比对，花钱之前。M1 恒 `Reuse`——`agent_core::cache`
    // 模块文档：还没有任何一处会有意改前缀。
    let one_shot = prepared.one_shot;
    let request = match crate::image_materialization::ProviderRequest::new(
        &binding,
        body,
        crate::image_materialization::OwnedIngredients {
            system,
            messages: prepared.messages,
            tools,
            late_tools,
            late_system,
            prev_prefix,
        },
        ctx.image_resolver.as_ref().map(Arc::clone),
    ) {
        Ok(request) => request,
        Err(failure) => {
            return Err(preparation_start_failed(
                ctx, agent, epoch, one_shot, failure,
            ));
        }
    };
    let materializes_images = request.materializes_images();
    let drift_verdict = if materializes_images {
        predicted_cache = 0;
        DriftVerdict::Expected {
            segment: Segment::History,
        }
    } else {
        cache::check_drift(drift, PrefixIntent::Reuse)
    };
    if matches!(drift_verdict, DriftVerdict::Unexpected { .. }) {
        // 照发不拦（M1 只告警不熔断），但必须立刻可见——这一轮接下来可能
        // 失败/超时/被取消，等不到成功收尾时的 `TurnGuard` 才补一句。
        ctx.emit(&agent, RunnerEvent::PreflightDriftAlert(drift_verdict));
    }

    // Snapshot an already-observed session cancellation, then let this call own a monotonic
    // latch. The runner propagates later session cancellation and deadlines into the same token.
    let cancel_token = Arc::new(AtomicBool::new(ctx.cancel_flag().load(Ordering::Relaxed)));
    let attempt = ProviderAttemptId::allocate();
    io_thread::spawn(
        tx,
        agent.clone(),
        attempt,
        binding.clone(),
        request,
        Arc::clone(&cancel_token),
    );

    Ok(ProviderCall {
        agent,
        attempt,
        epoch,
        deadline: Instant::now() + binding.timeout,
        binding,
        guard_scope: selection.guard_scope,
        drift: drift_verdict,
        predicted_cache,
        adjustments,
        prefix,
        one_shot,
        hold_deltas: one_shot || redact_provider_errors || materializes_images,
        replay_sanitized_deltas: materializes_images && !redact_provider_errors && !one_shot,
        redact_provider_errors,
        cancel_token,
    })
}

/// Suppress live deltas when the complete response must cross a later release gate.
pub(crate) fn gate_delta(call: &mut ProviderCall, event: RunnerEvent) -> Option<RunnerEvent> {
    if call.hold_deltas { None } else { Some(event) }
}

#[cfg(test)]
#[path = "provider_call_tests.rs"]
mod tests;

#[cfg(test)]
#[path = "provider_attempt_correlation_tests.rs"]
mod attempt_correlation_tests;

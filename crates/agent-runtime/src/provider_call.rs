//! `Effect::CallProvider` 的执行点，拆成**起飞**（[`start`]）与**落地**
//! （[`finish`]）两半。
//!
//! # 为什么拆开：029 的并行就是这一刀
//!
//! 012 时它是一个函数：取料 → `encode` → 起 IO 线程 → 就地 `recv_timeout` 等到
//! 底 → 返回一个 `Event`。「等到底」在单 agent 下没有代价，多 agent 下就是把并行
//! 掐死的那一句——root 和两个子 agent 的调用会一个接一个地跑完。
//!
//! 拆开之后：`start` 只做「在泵所在线程上能做完的部分」（取料、`encode`、发前
//! 第 1 层判读、把 IO future 交给泵）并返回一张**在飞凭据** [`ProviderCall`]；等谁先回来、
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
//! 这里仍然不保留、也不等待 IO 载体（117 之前是线程，现在是泵手上的一个
//! future）。runner 在超时后放弃 `(agent, attempt)` 凭据，所以已开始上传的迟到
//! 结果无法回写、重试或重新进入本次调用。

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use agent_core::cache::{self, PrefixIntent};
use agent_core::value::send_plan::project;
use agent_core::{
    Adjustment, AgentId, DriftVerdict, Epoch, ErrorClass, Event, PrefixImage, RequestIntent,
    Session,
};
use agent_providers::{Encoded, Ingredients};
// 114b：`Instant::now()` panic 在 wasm32-unknown-unknown 上，垫 `web-time`
// （native 目标下就是 `std::time::Instant` 本尊，行为不变）。
// `ProviderCall.deadline` 存的正是绝对时刻，每一次 provider 调用都会算它。
use web_time::Instant;

use crate::ctx::RunnerCtx;
use crate::event::RunnerEvent;
use crate::execution_binding::{ExecutionBinding, GuardScope};
use crate::io_bus::IoBus;
use crate::io_task;
use crate::provider_attempt::ProviderAttemptId;
use crate::subagent;
use crate::transient_source_failure::TransientSourceFailure;

pub(crate) use crate::provider_call_finish::{finish, thread_gone};

/// A provider launch either yields a regular session event or exposes a terminal transient-source
/// failure to the embedding host.
pub(crate) enum StartFailure {
    Event(Event),
    TransientSource(TransientSourceFailure),
}

/// 一次仍由 runner 认领的 provider 调用。**一个 agent 同时最多一张凭据**；超时
/// 划掉的旧 IO 载体可以与重试短暂重叠，但它持有不同的 [`ProviderAttemptId`]。
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
    /// Transient-source calls suppress live deltas; their content crosses public boundaries
    /// only through the host-facing terminal/failure paths.
    pub(crate) hold_deltas: bool,
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

/// 起飞：取料 → `encode` → 发前第 1 层 → 把 IO future 交给泵。**不等结果。**
//
// `StartFailure` 大是因为它内嵌 `Event`，而 `Event` 是 loop 的事件枚举、天然按最大
// 变体算尺寸。**不 Box**：起飞失败要原样变成一个进 loop 的事件（调用方直接把它
// 喂回泵），套一层 Box 只是把同一份数据挪到堆上、多一次解引用，换不到任何东西——
// 这条路径每轮至多走一次，不在热路上。
#[allow(clippy::result_large_err)]
pub(crate) fn start(
    session: &Session,
    ctx: &mut RunnerCtx,
    bus: &IoBus,
    agent: AgentId,
    epoch: Epoch,
) -> Result<ProviderCall, StartFailure> {
    let profile = session.execution_profile_of(&agent);
    let selection = match ctx.execution_binding_for(profile.as_ref()) {
        Ok(selection) => selection,
        Err(_) => {
            return Err(StartFailure::Event(Event::ProviderFailed {
                agent,
                epoch,
                class: ErrorClass::Unknown,
                message: Arc::from("execution profile is not configured"),
            }));
        }
    };
    let binding = selection.binding;
    // 宿主从状态取料（012，027 换成 `Session` 的读口，029 换成 per-agent 的那
    // 一版）：`imbl::Vector` 不是连续切片，Ingredients 要的是 `&[Message]`，
    // 这里物化一份——克隆的是 `Message`（里面全是 `Arc`），代价是指针拷贝乘
    // 消息数，不是深拷内容（红线 5）。
    //
    // 100：完整历史先过一遍 099 的投影纯函数，才进后面的 transient-source
    // overlay 和 `Ingredients`——「取料处只有一个」的另一半兑现在这里。
    // 摘要正文要跟着计划一起取出来喂给投影。**漏了这一步的症状极其隐蔽**：
    // 099 的 `project` 规定「有摘要引用但拿不到正文 → 边界作废、整份历史照发」
    // （宁可多发，不可发一段引用不到正文的空洞）。于是第 3 档会变成完全哑火——
    // `apply_summary` 照常写、`SendPlan` 状态全对、undo/恢复都正常，**只有实际
    // 发出去的请求体里一个字都没压**。不报错、测不到状态异常，只在账单上浮出来。
    //
    // 这一行在 100 落地时确实是 `None`（那会儿摘要还不存在），107 把 `summary_text`
    // 做出来之后就该跟上，但 100/107/108 三条各自的范围都没盖住这根线，
    // 由 108 的独测在真实请求体上抓到。见 108 实做记录。
    let history = session.messages_of(&agent);
    let plan = session.send_plan_of(&agent);
    let summary_text = plan
        .summary()
        .and_then(|id| session.summary_text(&agent, id));
    let durable_messages: Vec<agent_core::Message> =
        project(&history, &plan, summary_text.as_ref());
    let prepared = match crate::transient_source_prompt::prepare(
        &durable_messages,
        &mut ctx.transient_sources,
        &agent,
        epoch,
    ) {
        Ok(prepared) => prepared,
        Err(()) => {
            ctx.transient_sources.purge_agent_epoch(&agent, epoch);
            return Err(StartFailure::TransientSource(
                TransientSourceFailure::PromptPreparation { agent, epoch },
            ));
        }
    };
    let prev_prefix = session.prev_prefix_of(&agent);
    let system = subagent::system_for(session, ctx, &agent);
    let tools = subagent::tools_for(session, ctx, &agent);
    // 141：这里曾经把「这个 agent 当前激活的 skill」展开成两份注入料，塞进料单的
    // 正文段与中途工具段（039，决策 21）。决策 27 把 skill 正文改成按需
    // `srv:skill/read`（进 tool_result，不进 system），常驻索引也换成开局工具
    // （138/139，落进 `ctx.system`/`session.prefix_chunks()`）——那条展开方法与它
    // 背后的激活机制随 141 一起删掉。`late_tools` 字段留着给别的、非 skill 的
    // 中途加工具场景，这里恒传空（`Ingredients` 逐字节回到 039 之前）。
    let ing = Ingredients {
        system: &system,
        messages: &prepared.messages,
        tools: &tools,
        late_tools: &[],
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
            late_tools: &[],
            config: &binding.session_config,
            intent: RequestIntent::Free,
            prev_prefix: prev_prefix.as_ref(),
        });
        prefix = safe.prefix;
        drift = safe.drift;
        predicted_cache = safe.predicted_cache;
        adjustments = safe.adjustments;
    }

    // 兜底第 1 层：发前比对，花钱之前。103：意图不再恒 `Reuse`——拿这一轮实际
    // 要用的 `SendPlan`（`plan`，已经在上面读过）跟上一次请求用的那份比：一样就
    // 是沿用上一轮的前缀，任何漂移都是事故；不一样说明中间压缩改过发送计划，
    // 漂移是预期内的（全价重编码，但不是 bug）。这条比较本身就是「反向锁」的
    // 落点——没有压缩开火的轮次，两份计划天然相等，意图自动落回 `Reuse`。
    let one_shot = prepared.one_shot;
    let prev_plan = session.prev_send_plan_of(&agent);
    let intent = if plan == prev_plan {
        PrefixIntent::Reuse
    } else {
        PrefixIntent::Intentional
    };
    let drift_verdict = cache::check_drift(drift, intent);
    if matches!(drift_verdict, DriftVerdict::Unexpected { .. }) {
        // 照发不拦（M1 只告警不熔断），但必须立刻可见——这一轮接下来可能
        // 失败/超时/被取消，等不到成功收尾时的 `TurnGuard` 才补一句。
        ctx.emit(&agent, RunnerEvent::PreflightDriftAlert(drift_verdict));
    }

    // Snapshot an already-observed session cancellation, then let this call own a monotonic
    // latch. The runner propagates later session cancellation and deadlines into the same token.
    let cancel_token = Arc::new(AtomicBool::new(ctx.cancel_flag().load(Ordering::Relaxed)));
    let attempt = ProviderAttemptId::allocate();
    // 两份独立的发送端：增量走一份（会被会合背压停住），终态债走另一份（它一辈
    // 子只发一条，靠 `futures` 的「每个 sender 一个保底槽位」保证发得出去）。
    // 共用一份 = 债有可能因为「槽位被增量占着」而发不出去 = 泵为一个已经没了的
    // 调用永远等下去，见 `io_task::DoneDebt`。
    bus.start(io_task::task(
        bus.sender(),
        bus.sender(),
        agent.clone(),
        attempt,
        binding.clone(),
        body,
        Arc::clone(&cancel_token),
    ));

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
        hold_deltas: one_shot,
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

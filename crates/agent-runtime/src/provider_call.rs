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
//! # 超时之后：放弃 IO 线程，不 join，不主动断它的连接
//!
//! 超时只做两件事：把 `Event::Timeout` 塞回 loop、把这张凭据从在飞表里划掉。
//! **不触碰取消标志**——`agent-transport::client` 顶部记录过同一类权衡（读线程
//! 可能正卡在最长 60s 的阻塞 read 里，join 会把已经解耦掉的问题接回来）：被放弃
//! 的 IO 线程接着发的东西，泵按 agent 认不出在飞凭据就丢掉，最坏情况下它占着
//! 那条连接到 60s 死流兜底或者服务端最终答复为止——这段等待不会产生「重复计费」
//! （没有主动断线重发），真正的浪费只是「算完了但没人要这个答案了」，跟用户手动
//! Ctrl-C 打断是同一类代价。

use std::sync::Arc;
use std::sync::mpsc::SyncSender;
use std::time::Instant;

use agent_core::cache::{self, PrefixIntent};
use agent_core::{
    Adjustment, AgentId, ContentBlock, DriftVerdict, Epoch, ErrorClass, Event, PrefixImage,
    RequestIntent, Session, StopReason, TokenUsage,
};
use agent_providers::{Encoded, Ingredients};
use agent_transport::{StreamOutcome, TransportError};

use crate::ctx::RunnerCtx;
use crate::event::RunnerEvent;
use crate::guard;
use crate::io_thread::{self, IoMsg};
use crate::subagent;

/// 一次在飞的 provider 调用。**一个 agent 同时最多一张**（`Thinking` 是唯一
/// 会有 provider 调用在飞的状态，一个 agent 不可能同时处在两次 `Thinking` 里）。
pub(crate) struct ProviderCall {
    pub(crate) agent: AgentId,
    /// 起飞时的世代。落地时原样带回事件里过闸（红线 6）。
    pub(crate) epoch: Epoch,
    /// 到点就判超时。每个调用各自一份——两个子 agent 不共享同一条截止线。
    pub(crate) deadline: Instant,
    drift: DriftVerdict,
    predicted_cache: u32,
    adjustments: Vec<Adjustment>,
    prefix: PrefixImage,
}

/// 起飞：取料 → `encode` → 发前第 1 层 → 起 IO 线程。**不等结果。**
pub(crate) fn start(
    session: &Session,
    ctx: &mut RunnerCtx,
    tx: SyncSender<IoMsg>,
    agent: AgentId,
    epoch: Epoch,
) -> ProviderCall {
    // 宿主从状态取料（012，027 换成 `Session` 的读口，029 换成 per-agent 的那
    // 一版）：`imbl::Vector` 不是连续切片，Ingredients 要的是 `&[Message]`，
    // 这里物化一份——克隆的是 `Message`（里面全是 `Arc`），代价是指针拷贝乘
    // 消息数，不是深拷内容（红线 5）。
    let messages: Vec<agent_core::Message> = session.messages_of(&agent).iter().cloned().collect();
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
        messages: &messages,
        tools: &tools,
        late_tools: &late_tools,
        late_system: &late_system,
        config: &ctx.session_config,
        intent: RequestIntent::Free,
        prev_prefix: prev_prefix.as_ref(),
    };
    let Encoded { body, prefix, drift, predicted_cache, adjustments } = ctx.provider.encode(&ing);

    // 兜底第 1 层：发前比对，花钱之前。M1 恒 `Reuse`——`agent_core::cache`
    // 模块文档：还没有任何一处会有意改前缀。
    let drift_verdict = cache::check_drift(drift, PrefixIntent::Reuse);
    if matches!(drift_verdict, DriftVerdict::Unexpected { .. }) {
        // 照发不拦（M1 只告警不熔断），但必须立刻可见——这一轮接下来可能
        // 失败/超时/被取消，等不到成功收尾时的 `TurnGuard` 才补一句。
        ctx.emit(&agent, RunnerEvent::PreflightDriftAlert(drift_verdict));
    }

    io_thread::spawn(
        tx,
        agent.clone(),
        Arc::clone(&ctx.client),
        Arc::clone(&ctx.provider),
        ctx.endpoint.clone(),
        ctx.api_key.clone(),
        body,
        ctx.cancel_flag(),
    );

    ProviderCall {
        agent,
        epoch,
        deadline: Instant::now() + ctx.provider_timeout,
        drift: drift_verdict,
        predicted_cache,
        adjustments,
        prefix,
    }
}

/// 落地：把 IO 线程的终态翻译成一个 loop 事件；成功路径顺带装配 `GuardReport`。
pub(crate) fn finish(
    ctx: &mut RunnerCtx,
    call: ProviderCall,
    result: Result<StreamOutcome, TransportError>,
    blocks: Vec<ContentBlock>,
    stop: StopReason,
    usage: TokenUsage,
) -> Event {
    let ProviderCall { agent, epoch, drift, predicted_cache, adjustments, prefix, .. } = call;
    match result {
        Ok(StreamOutcome::Finished) => {
            guard::report_success(ctx, &agent, &usage, drift, predicted_cache, adjustments.clone());
            Event::ProviderDone { agent, epoch, blocks, stop, usage, prefix, adjustments }
        }
        // 半截的文本/工具调用不喂回 loop——回填回下一轮请求就不诚实了
        // （跟旧版 `agent-cli::turn::run_turn` 的判断一致）。取消是用户意图，
        // 不带 epoch。
        Ok(StreamOutcome::Cancelled) => Event::Cancel { agent },
        Ok(StreamOutcome::Broken(message)) => transport_trouble(ctx, agent, epoch, ErrorClass::Retryable, message),
        Err(TransportError::Connect { message, .. }) => {
            transport_trouble(ctx, agent, epoch, ErrorClass::Retryable, message)
        }
        Err(TransportError::Http { status, body }) => {
            let class = ctx.provider.classify(status, &body);
            ctx.emit(&agent, RunnerEvent::TransportTrouble(Arc::from(format!("HTTP {status}: {body}"))));
            Event::ProviderFailed { agent, epoch, class, message: Arc::from(body) }
        }
    }
}

/// IO 线程 panic 了（`IoMsg::Gone`）——没留下任何终态消息。按可重试的传输故障
/// 处理：`agent-transport::read_loop` 对读线程异常退出是同一个判断（它那边归
/// `StreamOutcome::Broken`）。
pub(crate) fn thread_gone(agent: AgentId, epoch: Epoch) -> Event {
    Event::ProviderFailed {
        agent,
        epoch,
        class: ErrorClass::Retryable,
        message: Arc::from("IO 线程异常退出（未留下终态消息）"),
    }
}

fn transport_trouble(
    ctx: &mut RunnerCtx,
    agent: AgentId,
    epoch: Epoch,
    class: ErrorClass,
    message: String,
) -> Event {
    ctx.emit(&agent, RunnerEvent::TransportTrouble(Arc::from(message.as_str())));
    Event::ProviderFailed { agent, epoch, class, message: Arc::from(message) }
}

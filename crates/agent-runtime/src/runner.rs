//! `run_turn`：**事件泵**。从用户输入到整棵 agent 树收工的驱动循环。
//!
//! 012 的形状是一个同步的四步循环（拿 effect → 逐个执行到底 → 结果转成事件 →
//! 喂回去）。029 把「执行到底」这一句拆开：`CallProvider` 只起飞不落地，谁先回来
//! 由泵统一等——于是 root 和它的子 agent 的 provider 调用**真的同时在飞**，而
//! 状态回写仍然一条一条串行地过 `Session::step`。STATE-MODEL §「并发」写的
//! 「子 agent 的并发是 IO 并发，不是状态并发」，这个文件就是那句话的形状。
//!
//! ```text
//!   ┌──────────────────────────────────────────────────────────┐
//!   │ A 排空待办：session.step(事件) → 持久化 → 分派 effect      │
//!   │              ↑                              ↓             │
//!   │   子 agent 落终态 → 父的 spawn 槽收敛 ← 立刻有结果的 effect │
//!   ├──────────────────────────────────────────────────────────┤
//!   │ B 在飞表空了 → 收工（root 终态就落快照）                   │
//!   │ C 到点的在飞调用 → 注入 Timeout 事件，回 A                 │
//!   │ D 统一 mpsc 上等一条消息（增量 / 终态 / 线程没了），回 A    │
//!   └──────────────────────────────────────────────────────────┘
//! ```
//!
//! # 收工条件：在飞表空
//!
//! 029 原文写的是「root 终态且无在飞」。实现成「**无在飞**」一个条件，是因为
//! 另一半是它的推论而不是补充：root 只有在自己那批工具槽全部收敛之后才可能落
//! 终态，而 spawn 的槽位要等子 agent 落终态才收敛，子 agent 同理递归——所以
//! 「root 终态」的时候在飞表必然已经空了。反过来写两个条件，就等于承认存在
//! 「root 已经终态、子树还在跑」的世界，那个世界里泵该怎么办没有答案。
//!
//! 真正需要单独说的是另一支：在飞表空了但 root **不是**终态。那是 016 裁决过的
//! 「转移表判了 `ProtocolViolation` 但状态没落终态」（例子见 `provider_done` 模块
//! 文档：响应自称 `ToolUse` 却一个 `ToolUse` 块都没有）。泵没有更多能喂的事件，
//! 也不该瞎猜一个终态或者死循环硬等——把控制权交还宿主，`Notice::ProtocolViolation`
//! 已经经回调发出去了。
//!
//! # 每条命令之后同步持久化（027）
//!
//! `session.step` 产出的每一条 `Entry` 都经 [`crate::persist::sync`] 转发进
//! [`RunnerCtx`] 挂的 `SessionStore`，游标与裁剪事件同一次调用里一起转发（011 的
//! 调用顺序契约）。`spawn_child` 也是一条命令，它那一条在 `crate::dispatch` 里
//! 转发。

use std::collections::VecDeque;
use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::sync::mpsc::{self, RecvTimeoutError};
use std::time::{Duration, Instant};

use agent_core::{AgentId, Event, Session, TurnStatus};

use crate::ctx::RunnerCtx;
use crate::dispatch::{self, Dispatched};
use crate::io_thread::IoMsg;
use crate::persist;
use crate::provider_call::{self, ProviderCall};
use crate::subtree::Subtree;

/// actor 线程轮询统一 channel 的间隔。不需要跟超时预算同量级，只要比测试用的
/// 超时预算（毫秒级）小得多，超时就能在可接受的粒度内被发现。
const POLL_INTERVAL: Duration = Duration::from_millis(20);

/// 跑一整轮：喂 `user_input`，驱动整棵 agent 树直到没有任何东西在飞。
///
/// `session` 原地推进，返回值只是 **root** 终态的一份拷贝，方便调用方立即判断
/// 结果——真正的历史/状态变化都已经写进 `session`（并已经同步进持久化后端）。
///
/// **公开签名从 012 起一个字没变**，029 也不许变：`agent-server` 的 session actor
/// （030）接在它上面，031 的 HTTP/SSE 层又接在 actor 上面。泵是这个函数的内部
/// 形状，不是它的契约。
///
/// 取消标志只在这里清零一次（每轮开始各清一次，理由跟 022 的 `agent-cli::
/// repl::run` 一致：上一轮遗留的标志不该提前打断这一轮还没开始的请求）；
/// 轮内的重试**不**清——那会把重试等待期间真实按下的 Ctrl-C 抹掉。
///
/// **调用方必须先 `session.begin_turn()`**（除了会话的第一轮，`Session::new`
/// 已经是 `Idle`）——`Session::begin_turn` 是显式命令（026 判断 13：turn 边界
/// 是会话层面的概念，不藏进转移表），这个函数不替调用方决定「新一轮从哪开始」。
pub fn run_turn(session: &mut Session, ctx: &mut RunnerCtx, user_input: &str) -> TurnStatus {
    ctx.cancel.store(false, Ordering::Relaxed);

    // 容量 0（rendezvous）：一个 IO 线程发一条增量就等泵收走，天然背压。
    // 泵自己握着一份发送端，所以 `recv` 永远不会因为「所有发送端都没了」而
    // 断开——在飞与否由下面这张表回答，不是由 channel 的连接状态回答。
    let (tx, rx) = mpsc::sync_channel::<IoMsg>(0);
    let mut pending: VecDeque<Event> = VecDeque::new();
    let mut calls: Vec<ProviderCall> = Vec::new();
    let mut subtree = Subtree::default();
    let root = session.agent().clone();
    let mut cancel_seen = false;

    pending.push_back(Event::UserInput { agent: root.clone(), text: Arc::from(user_input) });

    loop {
        // A. 排空待办。FIFO：一批 effect 产出的事件排在当前这批后面，
        //    顺序与 012 的「一代一代喂」完全一致。
        while let Some(event) = pending.pop_front() {
            let source = event.agent().clone();
            let effects = session.step(event);
            persist::sync(ctx, session);
            for effect in effects {
                match dispatch::run_effect(session, ctx, &mut subtree, &tx, &source, effect) {
                    Dispatched::Nothing => {}
                    Dispatched::Event(next) => pending.push_back(next),
                    Dispatched::Call(call) => calls.push(call),
                    // 会话级取消：在飞的流由取消标志斩断（它们各自会以
                    // `StreamOutcome::Cancelled` 回来），队列里还没喂进去的
                    // 待办在这里斩断——见 `Dispatched::CancelAll` 的文档。
                    Dispatched::CancelAll => pending.clear(),
                }
            }
            // 子 agent 可能就在刚才那一步里落了终态（它自己的 `ProviderDone`）。
            // 收割紧跟在 `step` 之后而不是攒到批末：父那个槽早一步收敛，父就早
            // 一步能接着干活。
            pending.extend(subtree.harvest(session, ctx));
        }

        // B. 没有在飞的东西了 —— 收工。
        if calls.is_empty() {
            let status = session.status();
            if status.is_terminal() {
                persist::maybe_snapshot(ctx, session);
            }
            return status;
        }

        // C. 到点的在飞调用：注入 `Timeout` 事件，回 A 让转移表决定重试还是失败。
        sweep_deadlines(&mut calls, &mut pending);
        speak_for_root_on_cancel(session, ctx, &root, &calls, &mut pending, &mut cancel_seen);
        if !pending.is_empty() {
            continue;
        }

        // D. 等一条 IO 消息。
        receive(ctx, &rx, &mut calls, &mut pending);
    }
}

/// 每个在飞调用各有各的截止线（它们不是同时起飞的）。到点的从表里划掉——
/// **不 join、不断连接**，理由见 `provider_call` 模块文档。
fn sweep_deadlines(calls: &mut Vec<ProviderCall>, pending: &mut VecDeque<Event>) {
    let now = Instant::now();
    let mut i = 0;
    while i < calls.len() {
        if calls[i].deadline > now {
            i += 1;
            continue;
        }
        let call = calls.remove(i);
        pending.push_back(Event::Timeout { agent: call.agent, epoch: call.epoch, call_id: None });
    }
}

/// 宿主按了 Ctrl-C（翻了 [`RunnerCtx::cancel_flag`]），而 **root 手上没有在飞的
/// provider 调用**——替它说一声。
///
/// M2 单 agent 时不需要这一步：取消发生时 root 一定在 `Thinking`，它自己那条流
/// 会以 `StreamOutcome::Cancelled` 回来，翻译成 `Event::Cancel { root }`。029
/// 多了一种形态：root 处在 `ToolsPending`，等的是几个子 agent——它自己没有任何
/// IO 在飞，那条「取消从流上回来」的路对它不存在。不补这一下的话，取消斩掉的是
/// 全部子 agent，而 root 停在 `ToolsPending` 永远等不到结果（子 agent 的
/// `Cancel` 已经把世代推走了，它们的 tool_result 会被 epoch 闸正当地丢掉），
/// `run_turn` 返回一个非终态——用户按了 Ctrl-C 却看不到「取消了」。
///
/// **root 自己在飞时不补**：那条路已经通了，补一下只会让它收到两次 `Cancel`，
/// 第二次落在终态上就是一条没有意义的协议违规通报。
///
/// 只补一次（`seen` 闩）。取消标志在这一轮剩下的时间里一直是 `true`
/// （`run_turn` 开头清零，轮内不清——那会把重试等待期间真按下的 Ctrl-C 抹掉）。
fn speak_for_root_on_cancel(
    session: &Session,
    ctx: &RunnerCtx,
    root: &AgentId,
    calls: &[ProviderCall],
    pending: &mut VecDeque<Event>,
    seen: &mut bool,
) {
    if *seen || !ctx.cancel_flag().load(Ordering::Relaxed) {
        return;
    }
    *seen = true;
    let root_in_flight = calls.iter().any(|call| &call.agent == root);
    if !root_in_flight && !session.status().is_terminal() {
        pending.push_back(Event::Cancel { agent: root.clone() });
    }
}

fn receive(
    ctx: &mut RunnerCtx,
    rx: &mpsc::Receiver<IoMsg>,
    calls: &mut Vec<ProviderCall>,
    pending: &mut VecDeque<Event>,
) {
    match rx.recv_timeout(POLL_INTERVAL) {
        Ok(IoMsg::Delta(delta)) => {
            // 已经被放弃的调用（超时划掉）接着发来的增量：丢。跟 `Session::step`
            // 对过期 epoch 的处理同一条判据——过期回执是正常现象，不是错误，
            // 每条喊一声只会刷屏。
            if calls.iter().any(|call| call.agent == delta.agent) {
                ctx.emit(&delta.agent, delta.event);
            }
        }
        Ok(IoMsg::Done { agent, result, blocks, stop, usage }) => {
            if let Some(call) = take_call(calls, &agent) {
                pending.push_back(provider_call::finish(ctx, call, result, blocks, stop, usage));
            }
        }
        Ok(IoMsg::Gone { agent }) => {
            if let Some(call) = take_call(calls, &agent) {
                pending.push_back(provider_call::thread_gone(call.agent, call.epoch));
            }
        }
        Err(RecvTimeoutError::Timeout) => {}
        // 结构上不可达：泵自己握着一份发送端，`rx` 不会断。当成一次空转，
        // 下一圈 C 的截止线扫描会兜住任何真的没人再说话的情况。
        Err(RecvTimeoutError::Disconnected) => {}
    }
}

fn take_call(calls: &mut Vec<ProviderCall>, agent: &AgentId) -> Option<ProviderCall> {
    let at = calls.iter().position(|call| &call.agent == agent)?;
    Some(calls.remove(at))
}

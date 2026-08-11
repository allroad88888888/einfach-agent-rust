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
//!   │ D 统一 mpsc 上等一条消息（增量 / 终态 / 载体没了），回 A    │
//!   └──────────────────────────────────────────────────────────┘
//! ```
//!
//! # 收工条件：在飞表空
//!
//! 029 原文写的是「root 终态且无在飞」。实现成「**无在飞**」一个条件，是因为
//! 另一半是它的推论而不是补充：root 只有在自己那批工具槽全部收敛之后才可能落
//! 终态，而 spawn 的槽位要等子 agent 落终态才收敛，子 agent 同理递归——所以
//! 「root 终态」的时候在飞表必然已经空了。
//!
//! **052 的修正**：后台 spawn（`background=true`）让父那个槽在 spawn 那一刻就
//! 收敛，于是「root 已经终态、子树还在跑」这个世界**真的存在**了。它不需要新的
//! 静止条件——后台子自己的 provider 调用就住在同一张 `calls` 表里，所以泵照旧把
//! 它驱动到静止再返回，语义天然是「一轮结束 = root 终态 **且** 后台子静止」。
//! 需要补的只是别把没人要的子跑到底（浪费）：B 之前加一道定点 `despawn_child`，
//! 见 [`crate::orphan`]。原先这段文档说那个世界「没有答案」，是过虑。
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
//!
//! # 116/117：泵是 `async fn`，D 点是真的 await 点
//!
//! 115 拍板「一套路径、两边都 async」之后，`run_turn`/`resume_after_first_
//! commit` 从 116 起是 `async fn`。116 只改了「怎么等」不改「等什么」，在同步
//! channel 与 async 泵之间搭了一座临时桥（`receive` 里还是那句真阻塞的
//! `rx.recv_timeout`）；**117 把桥拆了**：IO 载体从 `std::thread` 换成同一个
//! 事件循环上的并发 future，`sync_channel(0)` 换成 `futures` 的
//! `mpsc::channel(0)`，D 点第一次成为一个会让出线程的真实 await 点。三样东西
//! 收在 [`crate::io_bus`] 里，这个文件只剩「什么时候等、等到了怎么落地」。

use std::collections::VecDeque;
use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::time::Duration;

use agent_core::{AgentId, AgentTree, Event, Session, TurnStatus, UserImage};

use crate::ctx::RunnerCtx;
use crate::deadline;
use crate::dispatch::{self, Dispatched};
use crate::io_bus::IoBus;
use crate::io_task::IoMsg;
use crate::mcp_call::{self, McpCall};
use crate::orphan;
use crate::persist;
use crate::provider_call::ProviderCall;
use crate::provider_message;
use crate::subtree::Subtree;

/// 泵「什么都没发生也要醒一次」的节奏（117 之前是 `recv_timeout` 的参数，现在
/// 是 `crate::heartbeat` 的心跳间隔，值和含义都没变）。不需要跟超时预算同量级，
/// 只要比测试用的超时预算（毫秒级）小得多，超时就能在可接受的粒度内被发现。
const POLL_INTERVAL: Duration = Duration::from_millis(20);

/// 跑一整轮：喂 `user_input`，驱动整棵 agent 树直到没有任何东西在飞。
///
/// `session` 原地推进，返回值只是 **root** 终态的一份拷贝，方便调用方立即判断
/// 结果——真正的历史/状态变化都已经写进 `session`（并已经同步进持久化后端）。
///
/// **公开签名从 012 到 115 一直没变，116 破了这条戒**：115 拍板「一套路径，
/// 两边都 async」，native 也不例外——这个函数从这一版起是 `async fn`。
/// `agent-server` 的 session actor（030）与 `agent-cli` 的 `repl::run` 都要
/// 跟着接一个 [`crate::block_on`]（116 实做记录：115 原文建议的
/// `futures_util::executor::block_on` 这条路径实测不存在——`executor` 是
/// `futures-executor` 那个另外的 crate 才有的东西，`futures-util` 自己从来没有
/// `executor` 模块，加它就在「futures 最小子集」之外多一个依赖，所以改成手写
/// 的 `block_on`，115 原文也预先批准了这条口子）。`agent-cli` 与 `agent-server`
/// 的 actor 线程（裸 `std::thread`，不在 tokio 运行时里）都调同一个
/// `agent_runtime::block_on`——两边都不是「在已有的 async 运行时里
/// `.await`」，而是「把这一整条 await 链在调用线程上跑到底」，跟改动前的同步
/// 阻塞行为逐字节一致。泵的**内部形状**仍然不是契约，改的只是「怎么等」
/// （116 范围），不是这个函数驱动整棵树、原地推进 `session` 的这套语义。
///
/// 取消标志只在这里清零一次（每轮开始各清一次，理由跟 022 的 `agent-cli::
/// repl::run` 一致：上一轮遗留的标志不该提前打断这一轮还没开始的请求）；
/// 轮内的重试**不**清——那会把重试等待期间真实按下的 Ctrl-C 抹掉。
///
/// **调用方必须先 `session.begin_turn()`**（除了会话的第一轮，`Session::new`
/// 已经是 `Idle`）——`Session::begin_turn` 是显式命令（026 判断 13：turn 边界
/// 是会话层面的概念，不藏进转移表），这个函数不替调用方决定「新一轮从哪开始」。
pub async fn run_turn(session: &mut Session, ctx: &mut RunnerCtx, user_input: &str) -> TurnStatus {
    run_turn_with_images(session, ctx, user_input, Vec::new()).await
}

/// 跑一整轮，连同已经由宿主准备好的用户图片一起喂入。
///
/// 图片引用在进入这里之前已经是不可变的纯数据；上传之类的 IO 属于宿主边界，
/// 不属于事件泵。保留 [`run_turn`] 的原签名，使没有图片的所有既有调用逐字不变。
pub async fn run_turn_with_images(
    session: &mut Session,
    ctx: &mut RunnerCtx,
    user_input: &str,
    images: Vec<UserImage>,
) -> TurnStatus {
    ctx.cancel.store(false, Ordering::Relaxed);
    let root = session.agent().clone();
    resume(
        session,
        ctx,
        Event::UserInput {
            agent: root,
            text: Arc::from(user_input),
            images,
        },
    )
    .await
}

/// 从一项已发生的事件继续驱动会话。
///
/// 远端工具的回传不是新的用户轮次，不能清除取消标志或调用 `begin_turn`；因此由
/// 受控的远端回传入口走这里，和普通工具执行完成后进入泵的路径完全一致。
pub(crate) async fn resume(
    session: &mut Session,
    ctx: &mut RunnerCtx,
    initial: Event,
) -> TurnStatus {
    resume_after_first_commit(session, ctx, initial, |_| {}).await
}

/// Resume the pump, invoking `after_commit` once after the initial event is committed and
/// persisted, but before any effect from that event is dispatched.
pub(crate) async fn resume_after_first_commit(
    session: &mut Session,
    ctx: &mut RunnerCtx,
    initial: Event,
    after_commit: impl FnOnce(&mut RunnerCtx),
) -> TurnStatus {
    // IO 载体：一条 channel + 一批并发在跑的 IO future + 心跳，见 `crate::io_bus`。
    // 背压仍然是「发一条增量就等泵收走」，只是 `futures` 的容量 0 会给每个发送端
    // 保底一个槽位（115 决策 3 拍板接受，代价与对抗测试见那边的模块文档）。
    let mut bus = IoBus::new(POLL_INTERVAL);
    let mut pending: VecDeque<Event> = VecDeque::new();
    let mut calls: Vec<ProviderCall> = Vec::new();
    // MCP 第四路（043）的在飞表，跟 `calls` 并列——两类在飞凭据（工具结果 vs 模型
    // 响应）各自按键落地，收工要两张表都空（见 B）。`Dispatched::CancelAll` **不清**
    // 这张表（跟 `calls` 一样）：留着让迟到的结果回来撞 `Session::step` 的 epoch 闸
    // 被正当丢弃（红线 6），而不是在泵这层无声抹掉。
    let mut mcp_calls: Vec<McpCall> = Vec::new();
    let mut subtree = Subtree::default();
    let root = session.agent().clone();
    let mut cancel_seen = false;
    // 048：树快照变化检测的起点——`ctx.tree_events_enabled()` 是 `false`（CLI）
    // 时留 `None`，一次 `agent_tree()` 都不多算；是 `true`（server）时用**这一轮
    // 开始之前**的树种它，而不是留 `None`：这样只有这一轮里真正改变了树的 step
    // 才会跟它比出差异、触发一次回调，`run_turn` 被反复调用（一轮接一轮）也不会
    // 在每轮开头都白白重发一次跟上一轮收尾时完全相同的树（见 `maybe_emit_tree`）。
    let mut last_tree: Option<AgentTree> = ctx.tree_events_enabled().then(|| session.agent_tree());
    let mut after_commit = Some(after_commit);

    pending.push_back(initial);

    loop {
        // A. 排空待办。FIFO：一批 effect 产出的事件排在当前这批后面，
        //    顺序与 012 的「一代一代喂」完全一致。
        while let Some(event) = pending.pop_front() {
            let event = crate::transient_source_ingress::prepare(session, ctx, event);
            let source = event.agent().clone();
            let effects = session.step(event);
            persist::sync(ctx, session);
            if let Some(after_commit) = after_commit.take() {
                after_commit(ctx);
            }
            maybe_emit_tree(ctx, session, &mut last_tree);
            for effect in effects {
                match dispatch::run_effect(session, ctx, &mut subtree, &bus, &source, effect) {
                    Dispatched::Nothing => {}
                    Dispatched::Event(next) => pending.push_back(next),
                    // 后台 spawn（052）：父的槽收敛 + 子开工，两件事按 dispatch
                    // 给的顺序排进同一批待办。
                    Dispatched::Events(list) => pending.extend(list),
                    Dispatched::Call(call) => calls.push(call),
                    Dispatched::McpCall(call) => mcp_calls.push(call),
                    // 会话级取消：在飞的流由取消标志斩断（它们各自会以
                    // `StreamOutcome::Cancelled` 回来），队列里还没喂进去的
                    // 待办在这里斩断——见 `Dispatched::CancelAll` 的文档。
                    Dispatched::CancelAll => {
                        for call in &calls {
                            call.cancel();
                        }
                        pending.clear();
                    }
                }
            }
            // 子 agent 可能就在刚才那一步里落了终态（它自己的 `ProviderDone`）。
            // 收割紧跟在 `step` 之后而不是攒到批末：父那个槽早一步收敛，父就早
            // 一步能接着干活。
            pending.extend(subtree.harvest(session, ctx));
        }

        // B0. 轮末清算（052）：root 已经答完，而后台子还没人领 —— 活的定点拆掉
        //     （不走会话级取消，理由见 `crate::orphan`），跑完没人领的告警丢掉。
        //     放在 B **之前**：这一圈可能就是收工的那一圈（后台子已经静止但还活
        //     着），拆干净了再返回，别把一棵没人要的子树留给下一轮。
        if orphan::reap(session, ctx, &mut subtree) {
            // 拆掉一棵子树改变了 `agent_tree()`，而 A 那段的变化检测只跟着
            // `session.step` 走 —— 这条路不经过 step，得自己补一次。
            maybe_emit_tree(ctx, session, &mut last_tree);
        }

        // B. 没有在飞的东西了 —— 收工。两张在飞表都空才算空（MCP 第四路，043）。
        if calls.is_empty() && mcp_calls.is_empty() {
            let status = session.status();
            if status.is_terminal() {
                ctx.transient_sources.purge_all();
                persist::maybe_snapshot(ctx, session);
            }
            // Preparation failures are request-local routing metadata. Vision child slots have
            // already consumed theirs; root and generic failures must not cross a run boundary.
            ctx.clear_image_preparation_failures();
            return status;
        }

        // C. 到点的在飞调用：注入 `Timeout` 事件，回 A 让转移表决定重试还是失败。
        // 060 起同一次扫描也管远端等待槽（到点注入 `ToolFailed`）——它们没有在飞
        // 凭据，但泵这一圈本来就活着（root 等远端、后台子还在飞）时顺手扫掉，
        // 比等泵收工再由宿主扫更早。判定与注入都在 `crate::deadline`。
        // MCP 调用没有泵级截止线——`tools/call` 自带客户端侧超时（`ctx.mcp_timeout`
        // 传给背景线程），线程必在超时内报回一条 `McpDone`（成功/错误/超时都算），
        // 所以 MCP 凭据一定会被 D 排空，不需要在这里扫。
        deadline::sweep(ctx, &mut calls, &mut subtree, &mut pending);
        speak_for_root_on_cancel(session, ctx, &root, &calls, &mut pending, &mut cancel_seen);
        if !pending.is_empty() {
            continue;
        }

        // D. 等一条 IO 消息。
        receive(ctx, &mut bus, &mut calls, &mut mcp_calls, &mut pending).await;
    }
}

/// 048：一次 `session.step` + persist 之后重算 `agent_tree()`，跟 `last_tree`
/// 比，**变了才经 `ctx.emit_tree` 发出去**并更新 `last_tree`——`SessionEvent
/// 没变的 step 不该重复推同一棵树`（048 验收原文）落在这一个函数上，调用点
/// （`run_turn` 主循环）不需要自己判断。
///
/// `!ctx.tree_events_enabled()`（`with_tree_events` 没设，CLI 的默认状态）直接
/// 返回——`agent_tree()` 遍历 `live_agents()` 逐个组 `AgentNode`，没人要看的话
/// 这次计算就是纯粹的浪费，`RunnerCtx::with_tree_events` 文档「CLI 不设 → 无
/// 开销」的承诺就靠这一行判断兑现，不是靠 `on_tree_change` 内部的 `None` 分支
/// （那时已经算完了）。
fn maybe_emit_tree(ctx: &mut RunnerCtx, session: &Session, last_tree: &mut Option<AgentTree>) {
    if !ctx.tree_events_enabled() {
        return;
    }
    let tree = session.agent_tree();
    if last_tree.as_ref() == Some(&tree) {
        return;
    }
    *last_tree = Some(tree.clone());
    ctx.emit_tree(tree);
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
    if !ctx.cancel_flag().load(Ordering::Relaxed) {
        return;
    }
    // The session flag is reset when a later turn starts. Every in-flight attempt therefore
    // receives its own irreversible snapshot before the runner considers returning control.
    for call in calls {
        call.cancel();
    }
    if *seen {
        return;
    }
    *seen = true;
    let root_in_flight = calls.iter().any(|call| &call.agent == root);
    if !root_in_flight && !session.status().is_terminal() {
        pending.push_back(Event::Cancel {
            agent: root.clone(),
        });
    }
}

/// D 点：等一条 IO 消息，最多等 [`POLL_INTERVAL`]，等到了按键认领。
///
/// 117 起这里是一个**真的 await 点**：`IoBus::receive` 会先把所有在飞的 IO
/// future 推一遍（029 的并行就发生在那一步），再看 channel 上有没有消息；什么
/// 都没有就把线程交还给执行器，由心跳在 20ms 后叫醒。等待期间泵不占着线程，
/// 这正是 wasm 上「单线程也能一边等 fetch 一边跑事件循环」的前提。
///
/// 等到之后的落地逻辑一字未改——载体换了，认领规则没换。
async fn receive(
    ctx: &mut RunnerCtx,
    bus: &mut IoBus,
    calls: &mut Vec<ProviderCall>,
    mcp_calls: &mut Vec<McpCall>,
    pending: &mut VecDeque<Event>,
) {
    match bus.receive(POLL_INTERVAL).await {
        // Provider replies are claimed by `(agent, attempt)` in one place. Late messages from an
        // abandoned attempt are expected and disappear without touching its same-agent retry.
        Some(IoMsg::Provider(message)) => {
            provider_message::land(ctx, calls, pending, message);
        }
        // MCP 第四路（043）落地：按 `(agent, call_id)` 认领在飞凭据 → 组一条工具结果
        // 事件（epoch 由凭据提供）喂回泵，过期与否交给 `Session::step` 的 epoch 闸。
        // 认不出（取消轮已划掉 / 迟到的重复回执）就丢，跟 provider 的 `take_call`
        // 不命中 provider attempt 同款。
        Some(IoMsg::McpDone {
            agent,
            call_id,
            content,
            is_error,
        }) => {
            if let Some(call) = mcp_call::take(mcp_calls, &agent, &call_id) {
                pending.push_back(mcp_call::finish(ctx, call, content, is_error));
            }
        }
        // 这一小段里没有消息（心跳把泵叫醒了）。回到循环顶部去扫截止线、看取消
        // 标志——那两件事都不会有人主动叫醒我们，见 `crate::heartbeat`。
        None => {}
    }
}

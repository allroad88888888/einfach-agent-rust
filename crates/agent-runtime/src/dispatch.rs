//! 一个 effect 怎么变成真实世界里的一件事。**四个变体全部处理，`match` 不加
//! `_`**（012 原文）：`Effect` 加新变体时编译器会在这里逼一个决定，不会静默落进
//! 一个「什么都不做」的兜底分支。
//!
//! # spawn 的截获点就在这里
//!
//! `srv:agent/spawn` 走的是 `Effect::ExecuteTool`（它对 core 而言就是一次普通的
//! 工具调用，这正是决策 20 想要的：spawn 天然进日志、天然有 undo 语义），但它
//! **不进 `ToolExecutor`**——它要改的是会话状态，而 executor 够不着 `Session`。
//! 所以在分派处按名字截下来，落到 `Session::spawn_child`（028 的命令，两道闸在
//! 它里面）。按工具名 match 在宿主侧是合法的：宿主本来就持有工具表，这里没有
//! 任何模型相关判断（红线 12 管的是 core，且管的是 provider 分支）。
//!
//! 截获**以工具表里有没有这个声明为准**：宿主没把 spawn 放进表，模型就看不见
//! 这个名字，万一它凭空猜出来一个，那就该跟别的不存在的工具一样落
//! `unknown_tool`——而不是在一个没打算开子 agent 的宿主上凭空长出一棵树。

use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::sync::mpsc::SyncSender;

use agent_core::{
    AgentId, ChildConfig, Effect, Epoch, Event, Reversibility, Session, ToolCallId,
};
use serde_json::Value;

use crate::ctx::RunnerCtx;
use crate::event::RunnerEvent;
use crate::io_thread::IoMsg;
use crate::provider_call::{self, ProviderCall};
use crate::skill::{self, SKILL_ACTIVATE, SKILL_DEACTIVATE};
use crate::spawn_tool::{self, SPAWN_TOOL};
use crate::subagent;
use crate::subtree::Subtree;
use crate::{persist, tool_exec};

/// 一个 effect 执行完之后泵要接着做什么。
///
/// 写成枚举而不是 `Option<Event>` + 若干 out 参数：四种结果互斥，枚举把「一个
/// effect 最多产出其中一件事」写进类型，而不是靠调用点自觉。
pub(crate) enum Dispatched {
    /// 纯副作用，没有后续事件（`Emit`）。
    Nothing,
    /// 立刻就有结果，喂回泵。
    Event(Event),
    /// 起了一次 provider 调用，凭据交给泵的在飞表。
    Call(ProviderCall),
    /// 取消：这一代在飞的一切作废，**连还没喂进去的待办事件一起**。
    ///
    /// 取消是会话级的（`Effect::CancelInFlight` 没有 agent 字段，epoch 是会话
    /// 世代——028 推给 029 的第 3 条），所以斩的也是全会话：光置取消标志只能斩掉
    /// 已经在飞的 HTTP 流，队列里那条「刚 spawn 出来的子 agent 的第一句话」不带
    /// epoch（用户意图不过闸），会绕过取消照常起飞。
    CancelAll,
}

pub(crate) fn run_effect(
    session: &mut Session,
    ctx: &mut RunnerCtx,
    subtree: &mut Subtree,
    tx: &SyncSender<IoMsg>,
    source: &AgentId,
    effect: Effect,
) -> Dispatched {
    match effect {
        Effect::CallProvider { agent, epoch } => {
            Dispatched::Call(provider_call::start(session, ctx, tx.clone(), agent, epoch))
        }
        Effect::ExecuteTool { agent, call_id, tool, input, epoch } => {
            if &*tool == SPAWN_TOOL && ctx.tools.declares(SPAWN_TOOL) {
                return spawn(session, ctx, subtree, &agent, call_id, &input, epoch);
            }
            // skill 激活/停用同款截获（039）：它们改会话状态（写 `SkillsActive`），
            // executor 够不着 `Session`。宿主没声明（没开 skill）就不截获，模型凭空
            // 猜出来的这个名字跟别的不存在的工具走同一条路（`unknown_tool`）。
            if (&*tool == SKILL_ACTIVATE || &*tool == SKILL_DEACTIVATE) && ctx.tools.declares(&tool) {
                return skill::intercept(session, ctx, &agent, call_id, &tool, &input, epoch);
            }
            // 027：发起时快照在这里造一次，`Irreversible` 的立刻登记——记录点
            // 必须在**派发**这一刻，而不是等结果落地才回头看，否则进程在工具
            // 跑到一半崩溃时，恢复出来的日志里压根没有这次调用「不可逆」的痕迹
            // （`mark_irreversible` 本身不落日志，落的是它让随后那条 `tool_result`
            // entry 带上的 `barrier` 位——见 `Session::mark_irreversible` 文档）。
            let request = ctx.tools.snapshot(&tool, Arc::clone(&input));
            if matches!(request.reversibility, Reversibility::Irreversible) {
                session.mark_irreversible(call_id.clone());
            }
            Dispatched::Event(tool_exec::execute(ctx, agent, call_id, request, epoch))
        }
        Effect::CancelInFlight { epoch: _ } => {
            ctx.cancel.store(true, Ordering::Relaxed);
            Dispatched::CancelAll
        }
        // `Notice` 没有 agent 字段，也不该有（029 §事件归属：别为多 agent 输出去改
        // 一个已经跨 SSE 的公开枚举）。归属从**这批 effect 出自谁的 `step`** 来，
        // 那是泵手上现成的事实，不用 core 多存一份。
        Effect::Emit(notice) => {
            ctx.emit(source, RunnerEvent::Notice(notice));
            Dispatched::Nothing
        }
    }
}

/// 截获一次 `srv:agent/spawn`。
///
/// 走通的话父那个槽**保持 `Pending`**——这个函数不产出任何喂给父的事件，只产出
/// 子 agent 的第一条 `UserInput`。父的等待因此不是一段等待代码，就是那个还没
/// 收敛的槽位（006 决策记录）。收敛发生在子 agent 落终态时，见 `crate::subtree`。
fn spawn(
    session: &mut Session,
    ctx: &mut RunnerCtx,
    subtree: &mut Subtree,
    parent: &AgentId,
    call_id: ToolCallId,
    input: &Arc<Value>,
    epoch: Epoch,
) -> Dispatched {
    // spawn 也是一次工具调用，该跟别的工具一样看得见「调了什么、参数是什么」。
    let request = ctx.tools.snapshot(SPAWN_TOOL, Arc::clone(input));
    ctx.emit(parent, RunnerEvent::ToolExecuting { call_id: call_id.clone(), request });

    let parsed = match spawn_tool::parse(input) {
        Ok(parsed) => parsed,
        Err(message) => return refuse(ctx, parent, call_id, epoch, message),
    };
    let parent_tools = subagent::allowed_names(session, ctx, parent);
    let tools_allowed = match parsed.tools {
        Some(wanted) => {
            if let Err(message) = spawn_tool::check_subset(&wanted, &parent_tools) {
                return refuse(ctx, parent, call_id, epoch, message);
            }
            wanted
        }
        None => parent_tools,
    };

    let child = match session.spawn_child(parent, ChildConfig { tools_allowed }) {
        Ok(child) => child,
        Err(refused) => return refuse(ctx, parent, call_id, epoch, spawn_tool::refusal_text(&refused)),
    };
    // `spawn_child` 是一条命令，落了一条 `Entry`——跟 `step` 之后一样立刻转发进
    // 持久化后端，否则进程在子 agent 干活期间崩溃，恢复出来的会话里会有一个
    // 「有工作痕迹但没有出生记录」的 agent。
    persist::sync(ctx, session);
    subtree.record(child.clone(), parent.clone(), call_id, epoch);

    // 任务文本 = 子 agent 的第一条 user 消息（029 §注意）。它经 `Session::step`
    // 的正门进去：子 agent 刚建好时槽位全是默认值，`Status` 就是 `Idle`，于是
    // 转移表 `Idle + UserInput` 这一格原样接住它、发出它自己的 `CallProvider`。
    // 「子 agent 怎么开始干活」因此没有专门的代码路径。
    Dispatched::Event(Event::UserInput { agent: child, text: parsed.task })
}

/// spawn 没做成：**`is_error` 的 tool_result 回给模型**（决策 20），不是 panic，
/// 也不是让这一轮卡住。父那个槽位照常收敛，loop 接着跑，模型看着这句话自己收敛。
fn refuse(
    ctx: &mut RunnerCtx,
    parent: &AgentId,
    call_id: ToolCallId,
    epoch: Epoch,
    message: String,
) -> Dispatched {
    ctx.emit(
        parent,
        RunnerEvent::ToolExecuted {
            call_id: call_id.clone(),
            tool: Arc::from(SPAWN_TOOL),
            output_len: message.len(),
            is_error: true,
        },
    );
    Dispatched::Event(Event::ToolFailed {
        agent: parent.clone(),
        epoch,
        call_id,
        error: Arc::from(message),
    })
}

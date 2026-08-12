//! 一个 effect 怎么变成真实世界里的一件事。**五个变体全部处理，`match` 不加
//! `_`**（012 原文，105 加 `Compact` 时这条规矩当场兑现了一次）：`Effect` 加新变体
//! 时编译器会在这里逼一个决定，不会静默落进一个「什么都不做」的兜底分支。
//!
//! # 会话状态类工具的截获点就在这里，**但实现不在**
//!
//! `srv:agent/spawn` / `srv:agent/status` / `srv:agent/collect` / `srv:skill/read`
//! 走的都是 `Effect::ExecuteTool`（对 core 而言就是普通的工具调用，这正是决策
//! 20 想要的：spawn 天然进日志、天然有 undo 语义），但它们**不进 `ToolExecutor`**
//! ——它们要碰的是会话状态或泵的记账，而 executor 够不着 `Session`、也够不着
//! `Subtree`。051/053/137 把它们一条条截到这个文件的手工 `if` 链里；147 把这条
//! 链换成一次查表（[`crate::intercept_registry`]）：这四条连同
//! `RunnerCtx::register_session_tool` 注册的扩展工具（146，决策 29）现在共用
//! 同一张装配期建好的表，`ctx.session_tool_registered` 命中即截获。按名字分流
//! 在宿主侧是合法的：宿主本来就持有工具表，这里没有任何模型相关判断（红线 12
//! 管的是 core，且管的是 provider 分支）。
//!
//! 这个文件只回答「这个名字归谁执行」，**怎么执行跟着工具自己走**——既有四条住
//! `crate::builtin_intercepts`（迁移前是 `spawn_tool::intercept` /
//! `status_tool::intercept` / `collect_tool::intercept` / `skill::read_intercept`
//! 各自的文件），扩展工具住 `crate::session_tool_ext`。
//!
//! 截获**以工具表里有没有这个声明为准**：宿主没把 spawn 放进表，模型就看不见
//! 这个名字，万一它凭空猜出来一个，那就该跟别的不存在的工具一样落
//! `unknown_tool`——而不是在一个没打算开子 agent 的宿主上凭空长出一棵树。
//!
//! `Effect::Compact` 不是一次工具调用（模型看不见它、也点不到它），但「怎么执行
//! 跟着自己那个模块走」的规矩一样适用：这里只把它路由给
//! [`compact_spawn::intercept`]（106），真正的摘要子 agent 怎么 spawn、任务文本
//! 怎么拼、失败路径怎么走，都在那个文件的模块文档里。

use std::sync::Arc;
use std::sync::atomic::Ordering;

use agent_core::{
    AgentId, Effect, Epoch, Event, Reversibility, Session, ToolCallId, ToolCallRequest,
};

use crate::compact_slot::CompactSlots;
use crate::compact_spawn;
use crate::ctx::RunnerCtx;
use crate::event::RunnerEvent;
use crate::intercept_registry;
use crate::io_bus::IoBus;
use crate::mcp_call::{self, McpCall};
use crate::provider_call::{self, ProviderCall};
use crate::subtree::Subtree;
use crate::tool_exec;

/// 一个 effect 执行完之后泵要接着做什么。
///
/// 写成枚举而不是 `Option<Event>` + 若干 out 参数：几种结果互斥，枚举把「一个
/// effect 的后续只有一种形态」写进类型，而不是靠调用点自觉。
pub(crate) enum Dispatched {
    /// 纯副作用，没有后续事件（`Emit`）。
    Nothing,
    /// 立刻就有结果，喂回泵。
    Event(Event),
    /// 一次 effect 立刻产出**两件事**，按给的顺序喂回泵。只有后台 spawn（052）
    /// 走这一条：它同时要收敛父的槽（`ToolResult`）和让子开工（`UserInput`），
    /// 而这两件事分属两个 agent，没法塞进一个事件里。
    ///
    /// 不把 [`Dispatched::Event`] 改成 `Vec`：那条路上全是「一个 effect 一个
    /// 结果」的调用点（tool/skill/status/refuse），改了之后每一处都要为一个
    /// 恒定长度 1 的 `vec![]` 付一次注意力。
    Events(Vec<Event>),
    /// 起了一次 provider 调用，凭据交给泵的在飞表。
    Call(ProviderCall),
    /// A transient-source request failed before it could produce a session event. Its original
    /// terminal reason belongs to the embedding host, not the session transition table.
    TransientSourceFailure(crate::TransientSourceFailure),
    /// 起了一次异步 MCP `tools/call`（第四路，043），凭据交给泵的 MCP 在飞表。
    /// 跟 [`Dispatched::Call`] 分两个变体：一个是工具结果、一个是模型响应，泵按
    /// 各自的键落地（`crate::mcp_call` / `crate::provider_call`）。
    McpCall(McpCall),
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
    compactions: &mut CompactSlots,
    bus: &IoBus,
    source: &AgentId,
    effect: Effect,
) -> Dispatched {
    match effect {
        Effect::CallProvider { agent, epoch } => {
            match provider_call::start(session, ctx, bus, agent, epoch) {
                Ok(call) => Dispatched::Call(call),
                Err(provider_call::StartFailure::Event(event)) => Dispatched::Event(event),
                Err(provider_call::StartFailure::TransientSource(failure)) => {
                    Dispatched::TransientSourceFailure(failure)
                }
            }
        }
        Effect::ExecuteTool {
            agent,
            call_id,
            tool,
            input,
            epoch,
        } => {
            // 146/147：截获式扩展工具注册表——装配期注册进来的工具，Rust 扩展
            // 访问会话状态的正门（决策 29）。既有四条（spawn/collect/status/
            // skill-read）与 `RunnerCtx::register_session_tool` 注册的扩展共用
            // 同一张表，`crate::builtin_intercepts`/`crate::session_tool_ext` 各
            // 管各的注册与执行细节，这里只回答「这个名字有没有登记」。命中即
            // 截获；未命中原路往下走（大概率落 unknown_tool——两条注册路径都
            // 保证「注册名 ⊆ declares()」，反过来「declares() 里的名字未必注册
            // 了截获」完全合法：一个只声明不截获的普通工具）。
            if ctx.session_tool_registered(&tool) {
                return intercept_registry::dispatch(
                    session, ctx, subtree, compactions, bus, &agent, call_id, &tool, input, epoch,
                );
            }
            // 027：发起时快照在这里造一次，`Irreversible` 的立刻登记——记录点
            // 必须在**派发**这一刻，而不是等结果落地才回头看，否则进程在工具
            // 跑到一半崩溃时，恢复出来的日志里压根没有这次调用「不可逆」的痕迹
            // （`mark_irreversible` 本身不落日志，落的是它让随后那条 `tool_result`
            // entry 带上的 `barrier` 位——见 `Session::mark_irreversible` 文档）。
            let table_declared = ctx.tools.declares(&tool);
            let request = ctx.tools.snapshot(&tool, Arc::clone(&input));
            if matches!(request.reversibility, Reversibility::Irreversible) {
                session.mark_irreversible(call_id.clone());
            }
            // MCP 第四路（docs/MCP.md §「dispatch 怎么分第四路」）：`mcp:` 前缀且工具表
            // 声明了它 → 起一次异步 `tools/call`，**不走 ctx.fs**（executor 够不着
            // `McpRegistry`，跟它够不着 `Session` 同理，spawn/skill 截获同款）。按工具
            // 名分派在宿主侧合法：这里没有任何模型相关判断（红线 12 管的是 core 里的
            // provider 分支）。snapshot + mark_irreversible 已在上面做过——readOnly 的
            // MCP 工具落 `Pure` 无屏障，非 readOnly 落 `Irreversible` 带屏障，复用
            // 020/027 的既有屏障机制，MCP 不新造。
            if tool.starts_with("mcp:") && table_declared {
                return start_mcp(ctx, bus, agent, call_id, request, epoch);
            }
            // 远端第五路（`web:` / `desk:`）：登记等待槽、把调用推给宿主，**挂起**
            // 不产事件。**只有部署期声明才放行**（141 删了「当前 agent 已激活的
            // host skill 声明」那条备选路径——decision 27 之后 skill 不再携带可
            // 执行的远端工具）；`location` 是**纯按名字**推的（`tool_table::location_of`：
            // `web:` 前缀就是 `Location::Web`），没有这道闸的话，模型只要吐一个
            // 工具表里根本没有的 `web:whatever/x` 就能给自己开一个永远等不到回传
            // 的槽：泵撞「在飞表空」收工返回 `ToolsPending`，宿主回命令队列等一个
            // 不会来的 `POST /tool_result`，会话**永久挂死且不报错**。没声明就落
            // 进下面那条既有的未知工具路（`ctx.fs.execute` 的 `unknown_tool`），
            // 模型看到 `is_error` 自纠——跟同样被编造出来的 `srv:` 名字待遇一致
            // （决策 20 的兜底）。
            if request.location.is_remote() && table_declared {
                let public_request = if crate::transient_source_policy::is_transient_source(&tool) {
                    crate::transient_source_policy::sanitize_request(&request)
                } else {
                    request
                };
                ctx.register_remote_tool(
                    agent.clone(),
                    call_id.clone(),
                    epoch,
                    public_request.clone(),
                );
                ctx.emit(
                    &agent,
                    RunnerEvent::ToolExecuting {
                        call_id,
                        request: public_request,
                    },
                );
                return Dispatched::Nothing;
            }
            Dispatched::Event(tool_exec::execute(ctx, agent, call_id, request, epoch))
        }
        // 106：spawn 一个窄范围子 agent，用它自己 `ChildConfig` 里的模型把
        // `[0, upto)` 那段历史读成一份摘要。真正的实现在 `compact_spawn::intercept`
        // ——这里只负责路由，跟上面几路工具截获同一个分工（模块文档已经说明）。
        Effect::Compact { agent, upto, epoch } => {
            compact_spawn::intercept(session, ctx, compactions, agent, upto, epoch)
        }
        Effect::CancelInFlight { epoch: _ } => {
            ctx.cancel.store(true, Ordering::Relaxed);
            ctx.discard_remote_tools();
            ctx.transient_sources.purge_all();
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

/// 起一次异步 MCP `tools/call`（第四路，043）。发起时快照 `request` 已经在调用点
/// 造好（`location` 恒 `Server`、`reversibility` 从 `mcp:` 映射查、屏障已按可逆性
/// 登记）。这里只做「看得见 + 起飞」：发一条 `ToolExecuting`（跟 spawn/remote 同款
/// 可见性），把 `McpRegistry`（红线 3：store 外，只传 `Arc`）交给背景线程起飞，返回
/// 在飞凭据给泵。
fn start_mcp(
    ctx: &mut RunnerCtx,
    bus: &IoBus,
    agent: AgentId,
    call_id: ToolCallId,
    request: ToolCallRequest,
    epoch: Epoch,
) -> Dispatched {
    ctx.emit(
        &agent,
        RunnerEvent::ToolExecuting {
            call_id: call_id.clone(),
            request: request.clone(),
        },
    );
    let call = mcp_call::start(
        bus.sender(),
        Arc::clone(&ctx.mcp),
        agent,
        call_id,
        request.tool,
        request.input,
        epoch,
        ctx.mcp_timeout,
    );
    Dispatched::McpCall(call)
}

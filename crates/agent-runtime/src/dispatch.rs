//! 一个 effect 怎么变成真实世界里的一件事。**四个变体全部处理，`match` 不加
//! `_`**（012 原文）：`Effect` 加新变体时编译器会在这里逼一个决定，不会静默落进
//! 一个「什么都不做」的兜底分支。
//!
//! # 会话状态类工具的截获点就在这里，**但实现不在**
//!
//! `srv:agent/spawn` / `srv:agent/status` / `srv:agent/collect` / skill 激活走的
//! 都是 `Effect::ExecuteTool`（它们对 core 而言就是普通的工具调用，这正是决策 20
//! 想要的：spawn 天然进日志、天然有 undo 语义），但它们**不进 `ToolExecutor`**
//! ——它们要碰的是会话状态或泵的记账，而 executor 够不着 `Session`、也够不着
//! `Subtree`。所以在分派处按名字截下来。按工具名 match 在宿主侧是合法的：宿主
//! 本来就持有工具表，这里没有任何模型相关判断（红线 12 管的是 core，且管的是
//! provider 分支）。
//!
//! 这个文件只回答「这个名字归谁执行」，**怎么执行跟着工具自己走**
//! （`spawn_tool::intercept` / `status_tool::intercept` / `collect_tool::intercept`
//! / `skill::intercept`）——051 立的规矩，053 的前置重构把 spawn 也搬了回去。
//!
//! 截获**以工具表里有没有这个声明为准**：宿主没把 spawn 放进表，模型就看不见
//! 这个名字，万一它凭空猜出来一个，那就该跟别的不存在的工具一样落
//! `unknown_tool`——而不是在一个没打算开子 agent 的宿主上凭空长出一棵树。

use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::sync::mpsc::SyncSender;

use agent_core::{
    AgentId, Effect, Epoch, Event, Reversibility, Session, ToolCallId, ToolCallRequest,
};

use crate::collect_tool::{self, COLLECT_TOOL};
use crate::ctx::RunnerCtx;
use crate::event::RunnerEvent;
use crate::io_thread::IoMsg;
use crate::mcp_call::{self, McpCall};
use crate::provider_call::{self, ProviderCall};
use crate::skill::{self, SKILL_ACTIVATE, SKILL_DEACTIVATE};
use crate::spawn_tool::{self, SPAWN_TOOL};
use crate::status_tool::{self, STATUS_TOOL};
use crate::subtree::Subtree;
use crate::tool_exec;
use crate::vision_tool;

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
    tx: &SyncSender<IoMsg>,
    source: &AgentId,
    effect: Effect,
) -> Dispatched {
    match effect {
        Effect::CallProvider { agent, epoch } => {
            // A retry starts a fresh attempt. Clear a previous vision timeout marker before
            // starting it so only the terminal attempt determines `vision_timeout`.
            subtree.record_provider_start(&agent);
            match provider_call::start(session, ctx, tx.clone(), agent, epoch) {
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
            if &*tool == vision_tool::VISION_INSPECT_TOOL {
                return if vision_tool::is_root(session, &agent) {
                    // 根即使在恢复/热更新后已经失去 `vision` binding，也要由专用
                    // 门面收口成稳定的 `vision_profile_unavailable`，不能掉进普通
                    // unknown-tool 路径。
                    vision_tool::intercept(session, ctx, subtree, &agent, call_id, &input, epoch)
                } else {
                    // reserved facade 永不下放；即使宿主表里伪造了同名声明，child
                    // 也不能借它进入本地、MCP 或远端执行链。
                    vision_tool::refuse_non_root(ctx, &agent, call_id, epoch)
                };
            }
            if &*tool == SPAWN_TOOL && ctx.tools.declares(SPAWN_TOOL) {
                return spawn_tool::intercept(
                    session, ctx, subtree, &agent, call_id, &input, epoch,
                );
            }
            // collect 同款截获（053）：它读的是**泵的记账**（`Subtree` 的 detached
            // 名单与 stash），executor 连这张表的存在都不知道。两条出路：子已经跑完
            // 躺在 stash 里 → 当场回写（领取即消费）；还在跑 → 绑一个槽到这次 collect
            // 的 `call_id` 上、返回 `Nothing`，父那个槽保持 `Pending` 等收割回写
            // ——**跟前台 spawn 逐字同一条路**，只是绑定的时机由模型自己选。
            if &*tool == COLLECT_TOOL && ctx.tools.declares(COLLECT_TOOL) {
                return collect_tool::intercept(
                    session, ctx, subtree, &agent, call_id, &input, epoch,
                );
            }
            // status 同款截获（051）：它要读的是**整棵会话的 agent 树**
            // （`Session::agent_tree`），executor 够不着 `Session`。跟上面两条不同
            // 的是它是一次**纯读**——当场算完当场回写，没有 Pending、没有在飞
            // 凭据、也没有 entry 要同步（`status_tool::intercept` 的文档记了为什么
            // 它不调 `persist::sync`）。收窄到「调用者的后代」也在那里（红线 10 只
            // 下读；core 只负责算权威的整棵树）。
            if &*tool == STATUS_TOOL && ctx.tools.declares(STATUS_TOOL) {
                return status_tool::intercept(session, ctx, &agent, call_id, &input, epoch);
            }
            // skill 激活/停用同款截获（039）：它们改会话状态（写 `SkillsActive`），
            // executor 够不着 `Session`。宿主没声明（没开 skill）就不截获，模型凭空
            // 猜出来的这个名字跟别的不存在的工具走同一条路（`unknown_tool`）。
            if (&*tool == SKILL_ACTIVATE || &*tool == SKILL_DEACTIVATE) && ctx.tools.declares(&tool)
            {
                return skill::intercept(session, ctx, &agent, call_id, &tool, &input, epoch);
            }
            // 027：发起时快照在这里造一次，`Irreversible` 的立刻登记——记录点
            // 必须在**派发**这一刻，而不是等结果落地才回头看，否则进程在工具
            // 跑到一半崩溃时，恢复出来的日志里压根没有这次调用「不可逆」的痕迹
            // （`mark_irreversible` 本身不落日志，落的是它让随后那条 `tool_result`
            // entry 带上的 `barrier` 位——见 `Session::mark_irreversible` 文档）。
            // 084：部署期 ToolTable 仍是第一优先级；只有表里没有这个名字时，才按
            // **当前 agent** 的激活集解析 host skill 自带的远端工具。解析不到就继续
            // 走既有 unknown_tool 路径，不能因为 `web:` / `desk:` 前缀凭空挂起。
            let table_declared = ctx.tools.declares(&tool);
            let active_skill_request = if table_declared {
                None
            } else {
                let active = session.active_skills_of(&agent);
                ctx.tools
                    .active_host_tool_request(&active, &tool, Arc::clone(&input))
            };
            let remotely_declared = table_declared || active_skill_request.is_some();
            let request = active_skill_request
                .unwrap_or_else(|| ctx.tools.snapshot(&tool, Arc::clone(&input)));
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
                return start_mcp(ctx, tx, agent, call_id, request, epoch);
            }
            // 远端第五路（`web:` / `desk:`）：登记等待槽、把调用推给宿主，**挂起**
            // 不产事件。部署期声明或当前 agent 已激活的 host skill 声明才放行；`location`
            // 是**纯按名字**推的（`tool_table::location_of`：`web:` 前缀就是
            // `Location::Web`），没有这道闸的话，模型只要吐一个工具表里根本没有的
            // `web:whatever/x` 就能给自己开一个永远等不到回传的槽：泵撞「在飞表空」
            // 收工返回 `ToolsPending`，宿主回命令队列等一个不会来的 `POST
            // /tool_result`，会话**永久挂死且不报错**。没声明就落进下面那条既有的
            // 未知工具路（`ctx.fs.execute` 的 `unknown_tool`），模型看到 `is_error`
            // 自纠——跟同样被编造出来的 `srv:` 名字待遇一致（决策 20 的兜底）。
            if request.location.is_remote() && remotely_declared {
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
    tx: &SyncSender<IoMsg>,
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
        tx.clone(),
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

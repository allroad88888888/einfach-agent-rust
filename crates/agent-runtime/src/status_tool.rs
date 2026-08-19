//! `srv:agent/status`：模型用来看这个会话里此刻**谁在干啥**的那个工具
//! （M8，docs/ORCHESTRATION.md §三；207 按决策 35 放开到整棵树）。
//!
//! # 它是 M7 那棵活树的**模型侧**对偶
//!
//! 046 的 [`Session::agent_tree`] 已经把整棵活 agent 树摆成一份纯派生快照，
//! 047/048/049 把它送给**人**看（CLI 面板、SSE 帧、网页树）。这个工具把同一份快照
//! 送给**模型**看——**一个新 atom / 新 primitive 都不加**，它整个就是 `agent_tree()`
//! 的一次读。于是 `/undo` 撤掉一轮 spawn 之后模型下一次 `status` 自动看不到那个子
//! agent：活性判定在 `live_agents` 那一层，这里没有一行代码认识「撤销」这件事。
//!
//! # 207 之前它只看得见自己的严格后代
//!
//! 那道收窄是**这个文件里的一道人为过滤**，不是 core 的可见性——`agent_tree()`
//! 本来就是全树的纯派生快照，从来不受红线 10 约束。决策 35 横读全开之后那道过滤
//! 没有了理由：兄弟看得见兄弟是这一波的行为核心，而这份清单里的 id 正是
//! `srv:agent/send`（206）与 `srv:agent/await`（212）的目标。
//!
//! **调用者自己现在也在清单里**（末尾标 `(你)`）。207 之前排除自己的理由是
//! 「它此刻的 activity 恒等于『正在跑 status』，是句废话」——在只看后代的年代成立，
//! 现在不成立了：一份**全树**清单里独独缺自己，模型没法从这份清单知道自己是谁、
//! 在哪一层。自己那几样真正有用的账（还剩几轮、还能开几个子）在 `srv:agent/self`
//! （issue 208），不在这里。
//!
//! **仍然只暴露 activity + task，不暴露任何 agent 的消息正文**（决策 35 §一：
//! `Messages` 在 core 层放行，但工具层不给模型开按槽位读它的入口）。正文是
//! `collect` 的事（053），走另一条路（宿主 harvest 回写）。
//!
//! # 红线 11：这段正文会进下一轮 prompt
//!
//! 渲染必须逐字节确定：[`all_agents`] 自己 `sort_by(AgentId)` 排一次（**不借**
//! `live_agents` 的排序承诺——那是被调方的文档，它改了这里不会红），全程 `Vec`
//! 没有 `HashMap`/`HashSet`。渲染那一半在 [`crate::status_render`]。

use std::sync::Arc;

use agent_core::{AgentId, AgentNode, AgentTree, Epoch, Session, ToolCallId, ToolSpec};
use serde_json::{Value, json};

use crate::ctx::RunnerCtx;
use crate::dispatch::Dispatched;
use crate::event::RunnerEvent;
use crate::reply;
use crate::status_render::{not_live, render};

/// 工具全名。`srv:` = 服务端本地执行（`Location::Server`，docs/TOOLS.md 的命名
/// 约定），`agent/status` = 这一族里的 status。
pub const STATUS_TOOL: &str = "srv:agent/status";

/// 喂给模型的声明。
pub fn status_spec() -> ToolSpec {
    ToolSpec {
        name: Arc::from(STATUS_TOOL),
        description: Arc::from(
            "看一眼这个会话里此刻谁在干啥——**整棵树**，不只是你开出来的子 agent：\
             你的子孙、你的上级、以及跟你平级的兄弟都在里面，你自己那行末尾标着 (你)。\
             **不阻塞**，当场返回。\n\
             每个 agent 给的是：id、深度、活动状态、以及它的任务。活动状态是\
             Idle（还没开始）/ Thinking（正在想）/ Working(工具名...)（正在跑这些工具）/ \
             Done（这一轮结束了）/ Failed(原因)（没走完）。\n\
             **这份清单里的 id 就是 srv:agent/send 的 to**——要跟谁说句话，先在这里\
             找到它的 id。\n\
             它**不返回任何 agent 的回答正文**。正文从哪来，取决于你当初怎么 spawn 的：\
             前台 spawn（缺省的 background=false，那次调用会等）的正文**就是那次 spawn \
             调用的结果**；`background=true` 开的那次 spawn 只回了一个 agent_id，它的\
             正文**要用 srv:agent/collect 去领**——在这里看到它 Done 不等于你已经拿到\
             答案了，没领的后台子 agent 会在你这一轮结束时被拆掉。\n\
             什么时候用：你并行拆了几个子任务，想知道谁还在跑、谁已经完了、谁失败了，\
             据此决定后面怎么安排（看到谁 Done 就先去 collect 谁）；或者你要给某个\
             agent 发条消息，先来这里拿它的 id。",
        ),
        schema: Arc::new(json!({
            "type": "object",
            "properties": {
                "id": {
                    "type": "string",
                    "description": "只看这个 agent 那一段子树（含它自己），比如 root/a1。它得是这个会话里活着的 agent。省略 = 看整棵树。"
                }
            }
        })),
    }
}

/// 截获一次 `srv:agent/status`。
///
/// **当场回写、无 Pending、无在飞凭据**：它是一次纯读，结果在这个函数里就算完了。
/// 也**不调 `persist::sync`**（spawn/skill 那两条截获都调了）——那两条各自落了一条
/// `Entry`（`spawn_child` / `activate_skill` 是命令），status 一条命令都没发，
/// 没有任何东西需要同步进持久化后端。
///
/// 失败（入参写错、`id` 不在活树上……）→ `is_error` 的 tool_result 喂回模型让它
/// 自己收敛（决策 20 的哲学，跟 spawn/collect 一致），不 panic、不卡住这一轮。
pub(crate) fn intercept(
    session: &Session,
    ctx: &mut RunnerCtx,
    agent: &AgentId,
    call_id: ToolCallId,
    input: &Arc<Value>,
    epoch: Epoch,
) -> Dispatched {
    // status 也是一次工具调用，该跟别的工具一样看得见「调了什么、参数是什么」。
    let request = ctx.tools.snapshot(STATUS_TOOL, Arc::clone(input));
    ctx.emit(
        agent,
        RunnerEvent::ToolExecuting {
            call_id: call_id.clone(),
            request,
        },
    );

    match observe(&session.agent_tree(), agent, input) {
        Ok(body) => reply::ok(ctx, agent, call_id, epoch, STATUS_TOOL, body),
        Err(message) => reply::refuse(ctx, agent, call_id, epoch, STATUS_TOOL, message),
    }
}

/// 一棵权威的树 + 谁在调 + 模型给的入参 → 给模型看的一段正文（或一句拒绝）。
///
/// `caller` 现在**只用来标记那一行是「你」**，不再参与收窄（207，决策 35）：
/// 视野是整棵活树，`id` 那条路只是在树上再取一段子树。
pub(crate) fn observe(tree: &AgentTree, caller: &AgentId, input: &Value) -> Result<String, String> {
    let all = all_agents(tree);
    let Some(focus) = parse(input)? else {
        return Ok(render(&all, caller));
    };
    if !all.iter().any(|node| node.id == focus) {
        return Err(not_live(&focus, &all));
    }
    let scoped: Vec<&AgentNode> = all
        .into_iter()
        .filter(|node| node.id == focus || focus.is_ancestor_of(&node.id))
        .collect();
    Ok(render(&scoped, caller))
}

/// 模型给的入参解析结果：要看哪一段。`None` = 没给 `id`（看整棵树）。
///
/// **错误一律是给模型看的文本**（跟 `spawn_tool::parse` 一致）：入参是模型写的，
/// 写错了该让它自己看见并改（003 的哲学）。
pub(crate) fn parse(input: &Value) -> Result<Option<AgentId>, String> {
    match input.get("id") {
        // 缺省与显式 `null` 是同一件事（模型两种都会写）。
        None | Some(Value::Null) => Ok(None),
        Some(Value::String(id)) if id.trim().is_empty() => {
            Err("status 失败：id 是空的。要看整棵树就把 id 省掉。".to_string())
        }
        Some(Value::String(id)) => Ok(Some(AgentId::new(id.trim()))),
        Some(_) => Err("status 失败：id 得是字符串（agent 的路径 id，比如 root/a1）。".to_string()),
    }
}

/// 这个会话里全部活着的 agent，按 `AgentId` 路径排序。
///
/// **207 起不过滤**：`agent_tree()` 已经只装活的（`live_agents`），这里要的就是全部。
/// 之前那道 `is_descendant_of(caller)` 是本文件的一道人为收窄，决策 35 之后没有理由
/// 再留（见模块文档）。
///
/// 这里的 `sort_by` 是红线 11 的落点。`live_agents()` 今天确实是排序的
/// （`BTreeSet` + `sort()`），但那是**被调方**的实现承诺——它哪天改了，坏的是
/// 这段进 prompt 的字节而不是它自己的测试。确定性要在用得着它的地方自己保证。
fn all_agents(tree: &AgentTree) -> Vec<&AgentNode> {
    let mut out: Vec<&AgentNode> = tree.nodes.iter().collect();
    out.sort_by(|a, b| a.id.cmp(&b.id));
    out
}

#[cfg(test)]
#[path = "status_tool_tests.rs"]
mod tests;

#[cfg(test)]
#[path = "status_spec_tests.rs"]
mod spec_tests;

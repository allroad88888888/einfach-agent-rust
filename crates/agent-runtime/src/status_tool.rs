//! `srv:agent/status`：模型用来在子 agent 还在跑的时候看它们此刻在干啥的那个工具
//! （M8，docs/ORCHESTRATION.md §三）。
//!
//! # 它是 M7 那棵活树的**模型侧**对偶
//!
//! 046 的 [`Session::agent_tree`] 已经把整棵活 agent 树摆成一份纯派生快照，
//! 047/048/049 把它送给**人**看（CLI 面板、SSE 帧、网页树）。这个工具把同一份快照
//! 送给**模型**看——**一个新 atom / 新 primitive 都不加**，它整个就是 `agent_tree()`
//! 的一次读。于是 `/undo` 撤掉一轮 spawn 之后模型下一次 `status` 自动看不到那个子
//! agent：活性判定在 `live_agents` 那一层，这里没有一行代码认识「撤销」这件事。
//!
//! **只暴露 activity + task，不暴露子 agent 的消息正文**（ORCHESTRATION §三/五）。
//! 正文是 `collect` 的事（053），走另一条路（宿主 harvest 回写）。这条边界不是
//! 谨慎，是可见性本身：`Messages` 槽是 Upward-only，父下读子的正文在 core 那层就
//! 拿不到；`status` 读的 `Status` 是 Downward-visible，所以它合法（红线 10）。
//!
//! # 收窄住在宿主，不住 core
//!
//! core 算**权威的整棵树**，宿主把它收窄成**这次调用者看得到的那一段**。
//! 「谁在调、他能看到哪些」是宿主的编排问题（只有宿主知道这次 `ExecuteTool` 出自
//! 哪个 agent），core 不该为此多一个「按调用者过滤」的参数——那是把宿主的视角概念
//! 推进一个纯读层（红线 12 的精神：core 不替上层做判断）。
//!
//! 收窄的形状让红线 10 **由构造保证**而不是由检查保证：[`observe`] 先算出
//! 「调用者的全部后代」这一个集合，`id` 那条路只是**在这个集合里再过滤一次**——
//! 结构上无从放大视野，不存在「校验漏了一条分支就横读」的可能。
//!
//! # 红线 11：这段正文会进下一轮 prompt
//!
//! 它是 tool_result，从此原样躺在调用者的历史里进每一次后续请求。所以渲染必须
//! 逐字节确定：[`descendants`] 自己 `sort_by(AgentId)` 排一次（**不借**
//! `live_agents` 的排序承诺——那是被调方的文档，它改了这里不会红），全程 `Vec`
//! 没有 `HashMap`/`HashSet`，`task` 的压平与截断都是纯函数。

use std::fmt::Write as _;
use std::sync::Arc;

use agent_core::{
    AgentActivity, AgentId, AgentNode, AgentTree, Epoch, Session, ToolCallId, ToolSpec,
};
use serde_json::{Value, json};

use crate::ctx::RunnerCtx;
use crate::dispatch::Dispatched;
use crate::event::RunnerEvent;
use crate::reply;

/// 工具全名。`srv:` = 服务端本地执行（`Location::Server`，docs/TOOLS.md 的命名
/// 约定），`agent/status` = 这一族里的 status。
pub const STATUS_TOOL: &str = "srv:agent/status";

/// `task` 进正文时保留多少个**字符**（不是字节——按字节切会切碎中文）。
///
/// 截断而不是原样带上：spawn 的任务文本可以很长，而这段正文每一轮都会重进
/// prompt。一行看得出「这个子 agent 在做哪件事」就够了，看全文该去看 spawn 那次
/// 调用的入参——它本来就在同一段历史里。
const TASK_CHARS: usize = 100;

/// 喂给模型的声明。
pub fn status_spec() -> ToolSpec {
    ToolSpec {
        name: Arc::from(STATUS_TOOL),
        description: Arc::from(
            "看一眼你 spawn 出来的子 agent 此刻在干啥。**不阻塞**，当场返回。\n\
             返回你子树里每个后代的：id、深度、活动状态、以及它的任务。活动状态是\
             Idle（还没开始）/ Thinking（正在想）/ Working(工具名...)（正在跑这些工具）/ \
             Done（这一轮结束了）/ Failed(原因)（没走完）。\n\
             它**不返回子 agent 的回答正文**。正文从哪来，取决于你当初怎么 spawn 的：\
             前台 spawn（缺省的 background=false，那次调用会等）的正文**就是那次 spawn \
             调用的结果**；`background=true` 开的那次 spawn 只回了一个 agent_id，它的\
             正文**要用 srv:agent/collect 去领**——在这里看到它 Done 不等于你已经拿到\
             答案了，没领的后台子 agent 会在你这一轮结束时被拆掉。\n\
             什么时候用：你并行拆了几个子任务，想知道谁还在跑、谁已经完了、谁失败了，\
             据此决定后面怎么安排（看到谁 Done 就先去 collect 谁）。",
        ),
        schema: Arc::new(json!({
            "type": "object",
            "properties": {
                "id": {
                    "type": "string",
                    "description": "只看这个 agent 那一段子树（含它自己），比如 root/a1。必须是你的后代——你看不到自己的祖先和兄弟。省略 = 看你自己的全部后代。"
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
/// 失败（入参写错、`id` 不是自己的后代……）→ `is_error` 的 tool_result 喂回模型让它
/// 自己收敛（决策 20 的哲学，跟 spawn/skill 一致），不 panic、不卡住这一轮。
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
/// **红线 10 由构造保证**：`mine` 是「调用者的严格后代」这一个集合，`id` 那条路
/// 只在它里面再过滤一次，放不大。
pub(crate) fn observe(tree: &AgentTree, caller: &AgentId, input: &Value) -> Result<String, String> {
    let mine = descendants(tree, caller);
    let Some(focus) = parse(input)? else {
        return Ok(render(&mine));
    };
    if !focus.is_descendant_of(caller) {
        // 祖先、兄弟、别的树上的 id，以及**调用者自己**都落在这里。自己也拒是
        // 刻意的：规则「id 必须是你的后代」一条没有例外，比多一个「除非是你自己」
        // 的旁支好记，而拒绝文本会直接告诉它省掉 id 就是它想要的那件事。
        return Err(not_a_descendant(caller, &focus, &mine));
    }
    if !mine.iter().any(|node| node.id == focus) {
        return Err(not_live(&focus, &mine));
    }
    let scoped: Vec<&AgentNode> = mine
        .into_iter()
        .filter(|node| node.id == focus || focus.is_ancestor_of(&node.id))
        .collect();
    Ok(render(&scoped))
}

/// 模型给的入参解析结果：要看哪一段。`None` = 没给 `id`（看自己的全部后代）。
///
/// **错误一律是给模型看的文本**（跟 `spawn_tool::parse` 一致）：入参是模型写的，
/// 写错了该让它自己看见并改（003 的哲学）。
pub(crate) fn parse(input: &Value) -> Result<Option<AgentId>, String> {
    match input.get("id") {
        // 缺省与显式 `null` 是同一件事（模型两种都会写）。
        None | Some(Value::Null) => Ok(None),
        Some(Value::String(id)) if id.trim().is_empty() => {
            Err("status 失败：id 是空的。要看自己的全部后代就把 id 省掉。".to_string())
        }
        Some(Value::String(id)) => Ok(Some(AgentId::new(id.trim()))),
        Some(_) => Err("status 失败：id 得是字符串（agent 的路径 id，比如 root/a1）。".to_string()),
    }
}

/// `caller` 的**严格**后代，按 `AgentId` 路径排序。
///
/// 严格 = 调用者自己不在里面（`is_descendant_of` 本身就是严格的）：它不需要别人
/// 告诉它自己在干啥，而且它此刻的 activity 恒等于「正在跑 status」，是句废话。
///
/// 这里的 `sort_by` 是红线 11 的落点。`live_agents()` 今天确实是排序的
/// （`BTreeSet` + `sort()`），但那是**被调方**的实现承诺——它哪天改了，坏的是
/// 这段进 prompt 的字节而不是它自己的测试。确定性要在用得着它的地方自己保证。
fn descendants<'a>(tree: &'a AgentTree, caller: &AgentId) -> Vec<&'a AgentNode> {
    let mut out: Vec<&AgentNode> = tree
        .nodes
        .iter()
        .filter(|node| node.id.is_descendant_of(caller))
        .collect();
    out.sort_by(|a, b| a.id.cmp(&b.id));
    out
}

/// 收窄之后的那几个节点 → 给模型读的正文。**一个后代一行**，字段顺序固定
/// （id / depth / activity / task），空集也有话说。
fn render(nodes: &[&AgentNode]) -> String {
    if nodes.is_empty() {
        return "你现在没有子 agent：还没 spawn 过，或者它们已经被撤销了。".to_string();
    }
    let mut out = format!("你的子 agent（{} 个，只列你自己的后代）：", nodes.len());
    for node in nodes {
        // `write!` 往 `String` 里写不会失败，这个 `Result` 没有可处理的分支。
        let _ = write!(
            out,
            "\n{} depth={} {} task={}",
            node.id.as_str(),
            node.depth,
            activity(&node.activity),
            task(node.task.as_deref()),
        );
    }
    out
}

/// [`AgentActivity`] → 一个词。变体名原样用（Idle/Thinking/Working/Done/Failed），
/// 跟 docs/ORCHESTRATION.md §三那张表逐字对得上，模型读到的和文档写的是同一套词。
fn activity(activity: &AgentActivity) -> String {
    match activity {
        AgentActivity::Idle => "Idle".to_string(),
        AgentActivity::Thinking => "Thinking".to_string(),
        // 工具名一时推不出来时 `tools` 可以是空的（`AgentActivity::Working` 的
        // 文档）——那就只说「在忙」，不写一对空括号糊弄。
        AgentActivity::Working { tools } if tools.is_empty() => "Working".to_string(),
        AgentActivity::Working { tools } => format!("Working({})", tools.join(",")),
        AgentActivity::Done { truncated: false } => "Done".to_string(),
        AgentActivity::Done { truncated: true } => "Done(truncated)".to_string(),
        AgentActivity::Failed { reason } => format!("Failed({})", one_line(reason)),
    }
}

/// 任务文本 → 一行。压平换行 + 按字符截断（见 [`TASK_CHARS`]）。
///
/// 没有 user 消息就是没有（`AgentNode.task` 的 `None`）——不用 id 或工具名顶替，
/// 那样「没写任务」和「任务恰好是空字符串」在模型眼里会长得一样。
fn task(task: Option<&str>) -> String {
    let Some(text) = task else {
        return "(无)".to_string();
    };
    let flat = one_line(text);
    let mut out: String = flat.chars().take(TASK_CHARS).collect();
    if flat.chars().count() > TASK_CHARS {
        out.push('…');
    }
    out
}

/// 压成一行：控制字符（换行/回车/制表）一律换成空格。**一个后代一行**是这段正文
/// 的全部结构，任务文本里带个换行就能把它拆成两行、让模型读出一个不存在的 agent。
fn one_line(text: &str) -> String {
    text.chars()
        .map(|c| if c.is_control() { ' ' } else { c })
        .collect()
}

/// 横读/上读被拒。**说清是谁、并把「你能看的是哪些」一并给出**——模型才知道下一步
/// 该问谁（跟 `spawn_tool::check_subset` 同一个写法）。
fn not_a_descendant(caller: &AgentId, focus: &AgentId, mine: &[&AgentNode]) -> String {
    format!(
        "status 失败：{} 不是你（{}）的后代。你只能看自己子树里的 agent，看不到自己、\
         祖先和兄弟。{}",
        focus.as_str(),
        caller.as_str(),
        you_can_see(mine),
    )
}

/// 形状对（确实是自己的后代）但树上没有：没 spawn 过，或者那一轮被撤销了。
fn not_live(focus: &AgentId, mine: &[&AgentNode]) -> String {
    format!(
        "status 失败：{} 不在你的活子树上——没 spawn 出来过，或者它那一轮已经被撤销了。{}",
        focus.as_str(),
        you_can_see(mine),
    )
}

/// 拒绝文本的后半句。它也进 prompt，所以顺序照样是 [`descendants`] 排好的那个。
fn you_can_see(mine: &[&AgentNode]) -> String {
    if mine.is_empty() {
        return "你现在一个子 agent 都没有。".to_string();
    }
    format!(
        "你能看的是：{}。省略 id 可以一次看全。",
        mine.iter()
            .map(|node| node.id.as_str())
            .collect::<Vec<_>>()
            .join("、"),
    )
}

#[cfg(test)]
#[path = "status_tool_tests.rs"]
mod tests;

//! `srv:agent/status` 的**渲染半边**：一棵收窄好的树 → 一段给模型读的正文
//! （207 从 `status_tool.rs` 拆出来，红线 9）。
//!
//! 拆的判据是职责不是行数：那边回答「这次调用该看到哪些节点」（收窄与拒绝的**判定**），
//! 这边回答「看到的这些怎么写成字」（**渲染**）。两件事各自能一句话说清。
//!
//! # 红线 11：这段正文会进下一轮 prompt
//!
//! 它是 tool_result，从此原样躺在调用者的历史里进每一次后续请求。所以这里的每个
//! 函数都必须是纯函数、逐字节确定：**没有时间戳、没有随机 id、没有依赖容器迭代
//! 顺序的地方**。节点顺序由调用方排好（`status_tool::all_agents` 自己 `sort_by`），
//! 这里原样输出。

use std::fmt::Write as _;

use agent_core::{AgentActivity, AgentId, AgentNode};

/// `task` 进正文时保留多少个**字符**（不是字节——按字节切会切碎中文）。
///
/// 截断而不是原样带上：spawn 的任务文本可以很长，而这段正文每一轮都会重进
/// prompt。一行看得出「这个 agent 在做哪件事」就够了，看全文该去看 spawn 那次
/// 调用的入参——它本来就在同一段历史里。
pub(crate) const TASK_CHARS: usize = 100;

/// 收窄之后的那几个节点 → 给模型读的正文。**一个 agent 一行**，字段顺序固定
/// （id / depth / activity / task），空集也有话说。
///
/// `caller` 那一行末尾标 `(你)`：207 把视野放开到整棵树之后，调用者自己也在清单里
/// ——一份全树清单里独独缺自己，模型没法从这份清单知道自己是谁、在哪一层；
/// 而列进来却不标出来，它又分不清哪一行是自己。这个标记跟 id 一样逐字节确定。
pub(crate) fn render(nodes: &[&AgentNode], caller: &AgentId) -> String {
    if nodes.is_empty() {
        return "这个会话现在没有活着的 agent。".to_string();
    }
    let mut out = format!("这个会话现在的 agent（{} 个）：", nodes.len());
    for node in nodes {
        // `write!` 往 `String` 里写不会失败，这个 `Result` 没有可处理的分支。
        let _ = write!(
            out,
            "\n{} depth={} {} task={}{}",
            node.id.as_str(),
            node.depth,
            activity(&node.activity),
            task(node.task.as_deref()),
            if &node.id == caller { " (你)" } else { "" },
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

/// 压成一行：控制字符（换行/回车/制表）一律换成空格。**一个 agent 一行**是这段正文
/// 的全部结构，任务文本里带个换行就能把它拆成两行、让模型读出一个不存在的 agent。
fn one_line(text: &str) -> String {
    text.chars()
        .map(|c| if c.is_control() { ' ' } else { c })
        .collect()
}

/// 形状对但树上没有：没 spawn 过，或者那一轮被撤销了。
///
/// **把「你能看的是哪些」一并给出**——模型才知道下一步该问谁（跟
/// `spawn_tool::check_subset` 同一个写法：拒绝要给出下一步，不是只说「不行」）。
pub(crate) fn not_live(focus: &AgentId, all: &[&AgentNode]) -> String {
    format!(
        "status 失败：{} 不在这个会话的活 agent 里——没 spawn 出来过，\
         或者它那一轮已经被撤销了。{}",
        focus.as_str(),
        you_can_see(all),
    )
}

/// 拒绝文本的后半句。它也进 prompt，所以顺序照样是调用方排好的那个（红线 11）。
fn you_can_see(all: &[&AgentNode]) -> String {
    if all.is_empty() {
        return "这个会话现在一个活 agent 都没有。".to_string();
    }
    format!(
        "现在活着的是：{}。省略 id 可以一次看全。",
        all.iter()
            .map(|node| node.id.as_str())
            .collect::<Vec<_>>()
            .join("、"),
    )
}

#[cfg(test)]
#[path = "status_render_tests.rs"]
mod tests;

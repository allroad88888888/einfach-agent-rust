//! `ext:stats/report` 的正文：一份 `Session` + 谁在调 → 一段给模型读的字节。
//!
//! 拆成独立文件（一个文件一件事）：[`crate::ext_stats`] 管「一个扩展包长什么样、
//! 怎么装、钩子往哪写」，这里只管**账本怎么渲染成字节**——纯函数，单测直接喂一个
//! `Session` 断言字节，不需要工具表、不需要 `RunnerCtx`。
//!
//! # 三条纪律，按 docs/EXTENSIONS.md §四 的原文办
//!
//! 1. **红线 10（按调用者收窄）**：`agent_tree()` 给的是权威的整棵树，这里只列
//!    调用者自己 + 它的**严格后代**（照 `status_tool::observe` 的先例）。**消息
//!    条数只报调用者自己那一份**：`Slot::Messages` 是 Upward-only（`cross_read.rs`
//!    那张可见性表），父往下读子的正文在 core 那层本来就被 `read_descendant` 拒，
//!    扩展绕过那个口自己去 `messages_of(child)`，拿到的正是「机制没拦住、纪律不许」
//!    的东西——数一数条数也一样，它泄露的是子 agent 干了多少活。
//! 2. **红线 11（逐字节确定）**：一个 `HashMap`/`HashSet` 都没有——label 分布是
//!    `Vec` 排序去重，agent 一行一个按 `AgentId` 排序，全程无时钟、无随机。同一份
//!    状态渲染两次逐字节相同，否则每一轮都在全价重算前缀。
//! 3. **决策 19（工具结果 32 KiB 上限）**：agent 列表截到 [`AGENT_LINES`] 行、每行
//!    task 截到 [`TASK_CHARS`] 字符，最后再对整段做一次兜底截断（[`BODY_BYTES`]）
//!    ——长会话下自己收，不指望 core 兜底截得好看。
//!
//! # 数字取的是**生效**那一段，不是物理条数
//!
//! `history().entries()` 里躺着 undo 之后还能 redo 回来的尾巴（`History` 的语义），
//! `cursor()` 才是「此刻真的算数的条数」。报告主句用生效段——于是 `/undo` 一撤，
//! 模型下一次问「这个会话干了什么」，数字自己就回落了（149 验收第 2 条要演的正是
//! 这件事，扩展这一侧一行认识「撤销」的代码都没有）。物理条数与可 redo 条数在第二
//! 行如实并列，不藏。

use std::fmt::Write as _;

use agent_core::{AgentActivity, AgentId, AgentNode, Session, Undoability};

/// 子 agent 那一行里 task 保留多少个**字符**（不是字节——按字节切会切碎中文）。
/// 比 `status_tool` 的 100 更短：这份报告是汇总，看全文该去看 spawn 那次调用。
const TASK_CHARS: usize = 60;

/// 子 agent 最多列几行，超出只报数。
const AGENT_LINES: usize = 20;

/// 整段正文的兜底上限（决策 19 的 32 KiB 之内自己先收）。
const BODY_BYTES: usize = 8 * 1024;

/// 报告里的那几个数字。审计钩子把它记进账（[`crate::ext_stats::Ledger`]），
/// 单测拿它断言「undo 之后数字回落」而不用去 diff 一整段文本。
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub struct Counts {
    /// 生效段里出现过几个不同的 `turn_id`。
    pub turns: usize,
    /// 生效 entry 数（`Session::cursor`）。
    pub effective: usize,
    /// 物理 entry 数（含还能 redo 回来的尾巴）。
    pub entries: usize,
    /// 调用者自己 + 它的严格后代。
    pub agents: usize,
    /// 生效段里 `tool_result` / `tool_failed` 两种 label 的条数之和。
    pub tool_calls: usize,
}

/// 渲染一次报告：给模型看的正文 + 同一次读算出来的数字。
///
/// **纯读**：一条 command 都不发，`Session` 因此只需要 `&`（`SessionToolFn` 那层
/// 给的是 `&mut`，这里主动收窄——签名本身就是「这个工具 `Pure` 的举证」的一部分）。
pub fn render(session: &Session, caller: &AgentId) -> (String, Counts) {
    let tree = session.agent_tree();
    let mut mine: Vec<&AgentNode> = tree
        .nodes
        .iter()
        .filter(|node| node.id.is_descendant_of(caller))
        .collect();
    // 自己排一次，不借 `live_agents()` 的排序承诺（同 `status_tool::descendants`
    // 的判据：那是被调方的文档，它改了坏的是这段进 prompt 的字节）。
    mine.sort_by(|a, b| a.id.cmp(&b.id));

    let counts = count(session, mine.len() + 1);
    let redoable = counts.entries - counts.effective;
    let mut out = String::new();
    let _ = write!(
        out,
        "本会话至今：{} 轮、{} 条 entry、{} 个 agent、工具调用 {} 次。",
        counts.turns, counts.effective, counts.agents, counts.tool_calls,
    );
    let _ = write!(
        out,
        "\n账本：turn_id={}，entry 生效 {} / 物理 {}（可 redo {}），epoch={}，屏障 {} 处。",
        session.turn_id(),
        counts.effective,
        counts.entries,
        redoable,
        session.epoch().0,
        barriers(session),
    );
    let _ = write!(out, "\nentry 分布：{}", label_histogram(session));
    let _ = write!(
        out,
        "\n你自己（{}）：消息 {} 条，{}。",
        caller.as_str(),
        session.messages_of(caller).len(),
        activity(
            &tree
                .nodes
                .iter()
                .find(|n| &n.id == caller)
                .map(|n| &n.activity)
        ),
    );
    let _ = write!(out, "\n{}", children(&mine));
    (clamp(out), counts)
}

/// 生效段（前 `cursor()` 条）上的一次性统计。`agents` 由调用方给——收窄是
/// [`render`] 的事，这里只数账本。`pub(crate)`：153 起 [`crate::ext_stats`] 的
/// `TurnEnd` 审计钩子也现读一次账本，复用同一份计数逻辑而不是另写一份
/// （它不按调用者收窄——钩子没有「调用者」这个概念，见那边的模块文档）。
pub(crate) fn count(session: &Session, agents: usize) -> Counts {
    let effective = session.cursor();
    let mut turns: Vec<u64> = Vec::new();
    let mut tool_calls = 0usize;
    for entry in session.history().entries().take(effective) {
        if turns.last() != Some(&entry.meta.turn_id) {
            // entry 按 turn 成段追加，`last` 比一下就够去重；真乱序了下面
            // `dedup` 之前的 `sort` 也兜得住。
            turns.push(entry.meta.turn_id);
        }
        if matches!(entry.meta.label, "tool_result" | "tool_failed") {
            tool_calls += 1;
        }
    }
    turns.sort_unstable();
    turns.dedup();
    Counts {
        turns: turns.len(),
        effective,
        entries: session.history_len(),
        agents,
        tool_calls,
    }
}

/// 生效段里带屏障（不可越过的调用，红线 6）的条数。
///
/// 199 的三态之后这里只数 [`Undoability::Blocked`]：`Hooked` 不是屏障——它碰了外部
/// 世界**但交回了还原函数**，撤得掉，数进来会让「这个会话有几处撤不回去」这个统计
/// 凭空变大。判据是「挡不挡 undo」，不是「碰没碰外部世界」。
fn barriers(session: &Session) -> usize {
    session
        .history()
        .entries()
        .take(session.cursor())
        .filter(|entry| entry.meta.undoability == Undoability::Blocked)
        .count()
}

/// label 分布，**按 label 名排序**（红线 11：`HashMap` 的迭代顺序会让同一份状态
/// 渲染出两种字节）。空会话说「（空）」，不留一个光秃秃的冒号。
fn label_histogram(session: &Session) -> String {
    let mut labels: Vec<&'static str> = session
        .history()
        .entries()
        .take(session.cursor())
        .map(|entry| entry.meta.label)
        .collect();
    if labels.is_empty() {
        return "（空）".to_string();
    }
    labels.sort_unstable();
    let mut parts: Vec<String> = Vec::new();
    let mut run = 1usize;
    for i in 1..=labels.len() {
        if i < labels.len() && labels[i] == labels[i - 1] {
            run += 1;
            continue;
        }
        parts.push(format!("{}×{}", labels[i - 1], run));
        run = 1;
    }
    parts.join("、")
}

/// 后代那一段。**一个后代一行**，字段顺序固定；超过 [`AGENT_LINES`] 只报数。
fn children(mine: &[&AgentNode]) -> String {
    if mine.is_empty() {
        return "你现在没有子 agent：还没 spawn 过，或者它们那一轮已经被撤销了。".to_string();
    }
    let mut out = format!("你的子 agent（{} 个，只列你自己的后代）：", mine.len());
    for node in mine.iter().take(AGENT_LINES) {
        let _ = write!(
            out,
            "\n{} 深度{} {} task={}",
            node.id.as_str(),
            node.depth,
            activity(&Some(&node.activity)),
            task(node.task.as_deref()),
        );
    }
    if mine.len() > AGENT_LINES {
        let _ = write!(out, "\n……还有 {} 个没列出来。", mine.len() - AGENT_LINES);
    }
    out
}

/// [`AgentActivity`] → 一个词，用词与 `status_tool::activity` 逐字相同——模型在
/// 两个工具里读到的是同一套词汇。`None` = 这个 id 不在活树上（调用者自己被
/// despawn 之后仍然可能调进来一次）。
fn activity(activity: &Option<&AgentActivity>) -> String {
    match activity {
        None => "（不在活树上）".to_string(),
        Some(AgentActivity::Idle) => "Idle".to_string(),
        Some(AgentActivity::Thinking) => "Thinking".to_string(),
        Some(AgentActivity::Working { tools }) if tools.is_empty() => "Working".to_string(),
        Some(AgentActivity::Working { tools }) => format!("Working({})", tools.join(",")),
        Some(AgentActivity::Done { truncated: false }) => "Done".to_string(),
        Some(AgentActivity::Done { truncated: true }) => "Done(truncated)".to_string(),
        Some(AgentActivity::Failed { reason }) => format!("Failed({})", one_line(reason)),
    }
}

/// 任务文本 → 一行：压平控制字符 + 按字符截断。压平是结构性的——一个后代一行是
/// 这段正文的全部结构，task 里带个换行就能让模型读出一个不存在的 agent。
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

fn one_line(text: &str) -> String {
    text.chars()
        .map(|c| if c.is_control() { ' ' } else { c })
        .collect()
}

/// 兜底截断（决策 19）。按**字符**边界切，切完补一句说明——模型看到「被截断了」
/// 比看到半个 UTF-8 序列或者一段没有下文的文本强。
fn clamp(body: String) -> String {
    if body.len() <= BODY_BYTES {
        return body;
    }
    let mut out = String::with_capacity(BODY_BYTES + 32);
    for ch in body.chars() {
        if out.len() + ch.len_utf8() > BODY_BYTES {
            break;
        }
        out.push(ch);
    }
    out.push_str("\n……（报告过长，已截断）");
    out
}

#[cfg(test)]
#[path = "ext_stats_report_tests.rs"]
mod tests;

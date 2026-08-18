//! `srv:agent/self` 的渲染那一半：[`SelfFacts`] → 一段给模型看的正文。
//!
//! 拆出来跟 [`crate::status_render`] 同一条理由：**取数**（要读哪几个槽位、
//! 上限从哪来）与**措辞**（这几个数怎么说成一段话）是两件事，而后者是红线 11
//! 的落点——这段字节会进下一轮 prompt，必须逐字节确定。拆开之后措辞可以被
//! 单元测试逐字钉住，不必先造一个 `Session`。
//!
//! # 这里没有时钟、没有计数器、没有随机
//!
//! 同一份 [`SelfFacts`] 渲染两次逐字节相同（红线 11）。**「这是哪一刻的数」
//! 靠一句话说出来，不靠时间戳**——写进时间戳等于让每一轮的这段 tool_result
//! 都不一样，而它进的是历史、要被缓存。

use agent_core::AgentId;

/// 一个 agent 此刻自己的账。全部是 `Copy` 的小值 + 一个 id：取数与措辞之间
/// 只传值，渲染函数够不着 `Session`，也就写不出「渲染时顺手再读一格」。
#[derive(Clone, PartialEq, Eq, Debug)]
pub(crate) struct SelfFacts {
    pub id: AgentId,
    /// root = 0。
    pub depth: usize,
    pub max_depth: usize,
    pub turns_used: u32,
    pub max_turns: u32,
    pub retries_used: u32,
    pub max_retries: u32,
    /// **活着的**直接子 agent 数（despawn 掉一个就空出一格，同 `spawn_child`
    /// 那道闸数的东西）。
    pub children_live: usize,
    pub max_children: usize,
    /// 这个 agent 这一轮看得到几个工具。**只回条数不回名单**：名字全在工具表里、
    /// 已经在 prompt 最前面了，重列一遍是纯浪费 token，而且两份会不一致。
    pub tools: usize,
    /// 上下文压过没有。**只回布尔不回正文**：摘要正文是压缩边界那一侧的账，
    /// 塞进 tool_result 等于让同一段文字在 prompt 里出现两次。
    pub compacted: bool,
}

/// 一份账 → 一段正文。
pub(crate) fn render(f: &SelfFacts) -> String {
    let mut out = String::new();
    out.push_str(&format!(
        "你是 {}（第 {} 层，root 是第 0 层）。\
         **下面每个数都是你调用这一刻的**——它们之后还会变，要最新的就再调一次。\n",
        f.id.as_str(),
        f.depth,
    ));
    out.push_str(&format!("- 本轮已经请求 {} 次，{}\n", f.turns_used, turns(f)));
    out.push_str(&format!(
        "- 当前这条失败-重试链连续失败 {} 次，上限 {} 次。\n",
        f.retries_used, f.max_retries,
    ));
    out.push_str(&format!(
        "- 你有 {} 个活着的直接子 agent，{}\n",
        f.children_live,
        children(f),
    ));
    out.push_str(&format!("- {}\n", depth(f)));
    out.push_str(&format!("- 你这一轮看得到 {} 个工具。\n", f.tools));
    out.push_str(&format!("- 上下文压缩：{}\n", compaction(f)));
    out
}

/// 轮次那一行的后半句。**撞顶要说清后果**——「还剩 0 次」跟「再说一句就被切断」
/// 是同一个事实的两种说法，只有后一种模型会据此收敛。
fn turns(f: &SelfFacts) -> String {
    let left = f.max_turns.saturating_sub(f.turns_used);
    if left == 0 {
        return format!(
            "上限 {} 次**已经用完**——你这一轮不会再有下一次请求了，\
             现在就把结论给出来，别再调工具。",
            f.max_turns,
        );
    }
    if left == 1 {
        return format!(
            "上限 {} 次，**只剩最后 1 次**——下一次请求就是你这一轮的最后一次，\
             用它给结论，别用它调工具。",
            f.max_turns,
        );
    }
    format!("上限 {} 次，还剩 {} 次。", f.max_turns, left)
}

/// 子数那一行的后半句。
fn children(f: &SelfFacts) -> String {
    let left = f.max_children.saturating_sub(f.children_live);
    if left == 0 {
        return format!(
            "上限 {} 个**已经满了**——再 spawn 会被拒，\
             先等手上的做完（做完的子不占格子）。",
            f.max_children,
        );
    }
    format!("上限 {} 个，还能再开 {} 个。", f.max_children, left)
}

/// 深度那一行。整行而不是后半句：撞顶时说的是另一件事（不是「还能几层」，
/// 是「一层都不能了」）。
fn depth(f: &SelfFacts) -> String {
    let left = f.max_depth.saturating_sub(f.depth);
    if left == 0 {
        return format!(
            "你已经在最深一层了（整棵树最深 {} 层）——**你 spawn 不出子 agent**，\
             这件事得你自己做。",
            f.max_depth,
        );
    }
    format!(
        "你脚下还能再往下 {} 层（整棵树最深 {} 层）。",
        left, f.max_depth,
    )
}

/// 压缩那一行的后半句。
fn compaction(f: &SelfFacts) -> &'static str {
    if f.compacted {
        "压过——你历史里靠前的一段已经被换成摘要了，\
         那一段的原文你现在看不到（要原文就问上一级，别假装记得）。"
    } else {
        "没压过，你看到的历史就是全部。"
    }
}

#[cfg(test)]
#[path = "self_render_tests.rs"]
mod tests;

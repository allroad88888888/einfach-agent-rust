//! `srv:agent/notes` 的渲染那一半：一张草稿纸 → 一段给模型看的正文，
//! 以及 core 的拒绝 → 一句给模型看的话。
//!
//! 拆出来跟 [`crate::status_render`] / [`crate::self_render`] 同一条理由：
//! **取数**与**措辞**是两件事，而后者是红线 11 的落点——这段字节会进下一轮
//! prompt，必须逐字节确定。这里的确定性由 [`Notes`] 自己保证（`BTreeMap`，
//! 迭代恒是 key 升序），本文件只负责不往里掺时钟、随机或调用序号。

use agent_core::{MAX_NOTES, NOTE_KEY_CAP, NOTE_VALUE_CAP, NoteDenied};
use agent_core::value::notes::Notes;

/// 整张草稿纸 → 正文。空表也给一段正文（不是空字符串）：**「查到了，里面是空的」
/// 跟「这个工具坏了」在模型眼里必须分得开**。
pub(crate) fn render(notes: &Notes) -> String {
    if notes.is_empty() {
        return format!(
            "你的草稿纸是空的（最多能记 {MAX_NOTES} 条）。\
             要记点东西用 {}，它只有你看得见。",
            crate::NOTES_SET_TOOL,
        );
    }
    let mut out = format!(
        "你的草稿纸有 {} 条（上限 {MAX_NOTES} 条），按 key 排：\n",
        notes.len(),
    );
    for (key, value) in notes {
        out.push_str(&format!("- {key}: {value}\n"));
    }
    out
}

/// core 的拒绝 → 给模型看的话。
///
/// **每一条都要给出下一步**，不是只说「不行」——照 `send_tool::explain` /
/// `status_render::not_live` 的既有写法。模型拿不到下一步就只会换个写法再撞一次。
pub(crate) fn explain(denied: &NoteDenied) -> String {
    match denied {
        NoteDenied::EmptyKey => {
            "记笔记失败：key 是空的。给它起个名字，比如 \"下一步\"。".to_string()
        }
        NoteDenied::KeyTooLong { bytes, max } => format!(
            "记笔记失败：key 有 {bytes} 字节，上限 {max}。\
             **key 是标签不是正文**——把长的那段挪到 value 里去，key 留个短名字。\
             （这里不替你截断：截短的 key 是另一个名字，你下一轮拿原来的名字查不到。）"
        ),
        NoteDenied::ValueTooLong { bytes, max } => format!(
            "记笔记失败：value 有 {bytes} 字节，上限 {max}。分成几条记，或者只记结论。"
        ),
        NoteDenied::TooManyNotes { live, max } => format!(
            "记笔记失败：草稿纸已经有 {live} 条，上限 {max} 条。\
             **先删几条**（把 value 设成 null 就是删），或者改写一条已经有的\
             ——覆盖已有的 key 不占新格子，撞顶之后照样改得动。"
        ),
        NoteDenied::NotInSession { agent } => {
            format!("记笔记失败：{} 不在这个会话里。", agent.as_str())
        }
        NoteDenied::NotLive { agent } => {
            format!("记笔记失败：{} 已经不在活 agent 里了。", agent.as_str())
        }
    }
}

/// 写成功的回执。
///
/// **说清它去哪了**：草稿纸不自动进 prompt（见 `notes_tool` 模块文档），
/// 模型得知道「记下了」不等于「下一轮我会看见」。
pub(crate) fn wrote(key: &str, truncated_from: Option<usize>) -> String {
    let mut out = format!("记下了：{key}。");
    if let Some(original) = truncated_from {
        out.push_str(&format!(
            "**正文被截断了**——你给了 {original} 字节，上限 {NOTE_VALUE_CAP}，\
             只留了前面那段。要完整的就拆成几条记。",
        ));
    }
    out.push_str("它不会自动出现在你以后的对话里，要看得自己调 ");
    out.push_str(crate::NOTES_TOOL);
    out.push('。');
    out
}

/// 删成功的回执。删一条本来就不存在的 key 也走这里——**幂等是刻意的**，
/// 删两次收到一句错误只会让模型以为出了别的问题。
pub(crate) fn removed(key: &str) -> String {
    format!("删掉了：{key}（本来就没有的话，现在也没有）。")
}

/// key 上限的公开引用点，只给 spec 文案用——描述里那个数字必须跟真正拦人的
/// 是同一个（同 `with_spawn` 那条既有耦合的理由）。
pub(crate) const KEY_CAP: usize = NOTE_KEY_CAP;

#[cfg(test)]
#[path = "notes_render_tests.rs"]
mod tests;

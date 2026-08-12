//! `SkillRegistry::index_text` + `srv:skill/index` 的声明（138）。**它不是
//! dispatch 截获的对象**（这个工具不碰会话状态，不需要宿主侧特权）——139 起它挂
//! 在 `ToolTable::with_skills` 的 `SessionStart` 时机区（`tool_table_skill.rs`），
//! 135 的开局驱动在新建会话那一刻跑它一次，产出落进 `Session` 的前缀块，这个文件
//! 只交付文本产出与 spec 两件东西，不管「谁在什么时候调它」。
//!
//! # 141：这是唯一的索引路径
//!
//! 039 期的老索引路径（`skill_index_chunk`，激活/停用措辞、手动塞进
//! `Ingredients::system`）已随 141 删掉，[`SkillRegistry::index_text`] 是常驻索引
//! 唯一的产出口。

use std::fmt::Write as _;
use std::sync::Arc;

use agent_core::ToolSpec;
use serde_json::json;

use super::SkillRegistry;

/// 工具全名。139 起挂进 `ToolTable::with_skills` 的 `SessionStart` 时机区。
pub const SKILL_INDEX: &str = "srv:skill/index";

/// 索引工具的声明：无入参。
pub fn index_spec() -> ToolSpec {
    ToolSpec {
        name: Arc::from(SKILL_INDEX),
        description: Arc::from(
            "列出当前可用的 skill：每行「id — 描述」。想看某个 skill 的完整\
             操作说明，拿这里的 id 去调 srv:skill/read。",
        ),
        schema: Arc::new(json!({ "type": "object", "properties": {} })),
    }
}

impl SkillRegistry {
    /// 索引文本：首行固定引导语，之后每个**非 hidden**（142）skill 一行
    /// `<id> — <description>`，按 id 字典序（`BTreeMap` 迭代序天然满足，红线
    /// 11）。空 registry、或全部 skill 都 hidden → 空串（135 的规则：空文本不
    /// 产生 system 前缀块）。
    ///
    /// description 里带的换行**折成空格**：一行一个 skill 是这段文本唯一的
    /// 结构，一个 skill 的描述跨了两行，模型读到的会是「多了一个不存在的
    /// skill」。
    pub fn index_text(&self) -> Arc<str> {
        let mut visible = self
            .skills
            .values()
            .filter(|skill| !skill.hidden)
            .peekable();
        if visible.peek().is_none() {
            return Arc::from("");
        }
        let mut out = String::from("以下 skills 可用 srv:skill/read 按 id 取全文：");
        for skill in visible {
            out.push('\n');
            out.push_str(&skill.id.0);
            out.push_str(" — ");
            let _ = write!(out, "{}", one_line(&skill.description));
        }
        Arc::from(out)
    }
}

/// 压成一行：换行/回车/制表这类控制字符换成空格（跟 `status_tool::one_line`
/// 同一个理由——这段文本的行结构就是它唯一的解析规则）。
fn one_line(text: &str) -> String {
    text.chars()
        .map(|c| if c.is_control() { ' ' } else { c })
        .collect()
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use agent_core::SkillId;

    use super::*;
    use crate::skill::Skill;

    fn registry(skills: Vec<(&str, &str, bool)>) -> SkillRegistry {
        let mut map = BTreeMap::new();
        for (id, description, hidden) in skills {
            map.insert(
                Arc::from(id),
                Skill {
                    id: SkillId::new(id),
                    description: Arc::from(description),
                    body: Arc::from("正文不该出现在索引里"),
                    tools: Vec::new(),
                    hidden,
                },
            );
        }
        SkillRegistry { skills: map }
    }

    #[test]
    fn empty_registry_is_empty_text() {
        assert_eq!(&*registry(vec![]).index_text(), "");
    }

    #[test]
    fn one_skill_renders_one_line() {
        let text = registry(vec![("foo", "一个技能", false)]).index_text();
        assert_eq!(
            &*text,
            "以下 skills 可用 srv:skill/read 按 id 取全文：\nfoo — 一个技能"
        );
    }

    /// N 个 skill：行序按 id 字典序，跟插入顺序无关。
    #[test]
    fn many_skills_are_ordered_by_id() {
        let text = registry(vec![
            ("zeta", "z", false),
            ("alpha", "a", false),
            ("mid", "m", false),
        ])
        .index_text();
        let lines: Vec<&str> = text.lines().skip(1).collect();
        assert_eq!(lines, vec!["alpha — a", "mid — m", "zeta — z"]);
    }

    /// 两次调用逐字节相同（红线 11）。
    #[test]
    fn two_calls_produce_identical_bytes() {
        let reg = registry(vec![("a", "d1", false), ("b", "d2", false)]);
        assert_eq!(reg.index_text(), reg.index_text());
    }

    /// 输出不含任何正文字节——只有 id + description。
    #[test]
    fn output_never_contains_body_bytes() {
        let text = registry(vec![("foo", "描述", false)]).index_text();
        assert!(!text.contains("正文不该出现在索引里"));
    }

    /// description 带换行：折成一行，索引仍然一个 skill 一行。
    #[test]
    fn newline_in_description_is_flattened() {
        let text = registry(vec![("foo", "第一行\n第二行", false)]).index_text();
        assert_eq!(text.lines().count(), 2, "引导语 + 一个 skill，不能多出一行");
        assert!(text.contains("第一行 第二行"));
    }

    /// 142：hidden 的 skill 不进索引；全 hidden → 空文本。
    #[test]
    fn hidden_skills_are_filtered_out() {
        let text = registry(vec![("visible", "v", false), ("secret", "s", true)]).index_text();
        assert!(text.contains("visible"));
        assert!(!text.contains("secret"));

        let all_hidden = registry(vec![("a", "d", true), ("b", "d", true)]).index_text();
        assert_eq!(&*all_hidden, "");
    }
}

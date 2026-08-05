//! `/agents`（047）的渲染器：把 [`AgentTree`] 打成一段缩进文本。**纯格式化**——
//! 不碰 `Session`/`Store`，输入就是 `Session::agent_tree()` 的返回值，输出是一段
//! 可以直接 `println!` 的多行文本。
//!
//! 跟 048/049 的 web 树面板共用同一份 `agent_tree()` 数据
//! （`docs/OBSERVABILITY.md` §「snapshot，不是 reconstruct」：树由 core 权威算，
//! 这个模块只画，不重建）——所以这里只允许格式化逻辑，一行状态判断都不该有。
//!
//! 每行：`<缩进×depth><短 id> [<activity>] · <task 截断>`。

use agent_core::{AgentActivity, AgentId, AgentNode, AgentTree};

/// 单层缩进的宽度。
const INDENT_UNIT: &str = "  ";

/// task 文本超过这个字符数就截断（`…`收尾）。纯展示用的行宽控制，跟
/// `agent_core::limits::truncate_tool_output` 的字节级截断不是一回事——那个是
/// 给模型看的上限，这个只是不让一个终端行被撑爆。
const TASK_DISPLAY_MAX_CHARS: usize = 60;

/// 渲染整棵树成多行文本（行间用 `\n` 分隔，不带结尾换行——调用方 `println!` 自己
/// 补一个）。空树（理论上不会发生：`live_agents` 至少有 root）渲成空字符串。
pub fn render_agent_tree(tree: &AgentTree) -> String {
    tree.nodes
        .iter()
        .map(render_line)
        .collect::<Vec<_>>()
        .join("\n")
}

fn render_line(node: &AgentNode) -> String {
    let indent = INDENT_UNIT.repeat(node.depth as usize);
    let id = short_id(&node.id);
    let activity = describe_activity(&node.activity);
    let task = describe_task(node.task.as_deref());
    format!("{indent}{id} [{activity}] · {task}")
}

/// 短 id：去掉 `root` 那一段前缀，root 本身仍显示 `root`。跟
/// `print::events::prefix` 同一条理由——「最后一段」会撞（`root/a1/a1` 与
/// `root/a2/a1` 最后一段都是 `a1`），去掉 root 前缀之后剩下的整条尾巴才唯一。
fn short_id(id: &AgentId) -> &str {
    match id.as_str().split_once(agent_core::AGENT_PATH_SEP) {
        None => id.as_str(),
        Some((_root, tail)) => tail,
    }
}

/// [`AgentActivity`] 的可读呈现。
fn describe_activity(activity: &AgentActivity) -> String {
    match activity {
        AgentActivity::Idle => "Idle".to_string(),
        AgentActivity::Thinking => "Thinking".to_string(),
        AgentActivity::Working { tools } if tools.is_empty() => "Working".to_string(),
        AgentActivity::Working { tools } => format!("Working({})", tools.join(", ")),
        AgentActivity::Done { truncated: false } => "Done".to_string(),
        AgentActivity::Done { truncated: true } => "Done(truncated)".to_string(),
        AgentActivity::Failed { reason } => format!("Failed({reason})"),
    }
}

/// task 文本：折叠内部空白（含换行——多行的首条 user 消息不该把树render撑成
/// 多行），再按字符数截断。没有 task 就是一个占位符，不用别的字段顶替
/// （`AgentNode.task` 自己的文档：`None` 和「写了但是空字符串」不该长得一样，
/// 这里延续同一条原则）。
fn describe_task(task: Option<&str>) -> String {
    match task {
        None => "(无任务文本)".to_string(),
        Some(text) => truncate_chars(&collapse_whitespace(text), TASK_DISPLAY_MAX_CHARS),
    }
}

fn collapse_whitespace(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn truncate_chars(s: &str, max_chars: usize) -> String {
    if s.chars().count() <= max_chars {
        return s.to_string();
    }
    let truncated: String = s.chars().take(max_chars).collect();
    format!("{truncated}…")
}

#[cfg(test)]
mod tests {
    use agent_core::AgentId;

    use super::*;

    fn node(id: AgentId, task: Option<&str>, activity: AgentActivity) -> AgentNode {
        let parent = id.parent();
        let depth = id.depth() as u32;
        AgentNode {
            id,
            parent,
            depth,
            task: task.map(str::to_string),
            activity,
        }
    }

    /// 只有 root 一格：一行，短 id 就是 `root`，没有缩进。
    #[test]
    fn root_only_renders_a_single_unindented_line() {
        let tree = AgentTree {
            nodes: vec![node(
                AgentId::root(),
                Some("帮我读一下这个文件"),
                AgentActivity::Idle,
            )],
        };
        assert_eq!(render_agent_tree(&tree), "root [Idle] · 帮我读一下这个文件");
    }

    /// root + 两个 depth1 子：子行各缩进一层，短 id 不带 `root/` 前缀。
    #[test]
    fn root_with_two_children_indents_them_one_level() {
        let root = AgentId::root();
        let c1 = root.child(1);
        let c2 = root.child(2);
        let tree = AgentTree {
            nodes: vec![
                node(root, Some("总任务"), AgentActivity::Thinking),
                node(c1, Some("子任务一"), AgentActivity::Idle),
                node(
                    c2,
                    Some("子任务二"),
                    AgentActivity::Done { truncated: false },
                ),
            ],
        };
        let rendered = render_agent_tree(&tree);
        let lines: Vec<&str> = rendered.lines().collect();
        assert_eq!(lines.len(), 3);
        assert_eq!(lines[0], "root [Thinking] · 总任务");
        assert_eq!(lines[1], "  a1 [Idle] · 子任务一");
        assert_eq!(lines[2], "  a2 [Done] · 子任务二");
    }

    /// 深两层：缩进按 depth 累加，短 id 去掉 root 前缀之后仍是整条尾巴
    /// （不是只取最后一段——`root/a1/a1` 与 `root/a2/a1` 最后一段会撞）。
    #[test]
    fn grandchild_indents_two_levels_and_keeps_full_tail_as_short_id() {
        let root = AgentId::root();
        let c1 = root.child(1);
        let gc = c1.child(1);
        let tree = AgentTree {
            nodes: vec![
                node(root, None, AgentActivity::Idle),
                node(c1.clone(), None, AgentActivity::Idle),
                node(gc, Some("孙任务"), AgentActivity::Idle),
            ],
        };
        let rendered = render_agent_tree(&tree);
        let lines: Vec<&str> = rendered.lines().collect();
        assert_eq!(lines[2], "    a1/a1 [Idle] · 孙任务");
    }

    /// 各种 activity 的可读呈现，逐个断言。
    #[test]
    fn activity_variants_render_as_expected() {
        assert_eq!(describe_activity(&AgentActivity::Idle), "Idle");
        assert_eq!(describe_activity(&AgentActivity::Thinking), "Thinking");
        assert_eq!(
            describe_activity(&AgentActivity::Working { tools: vec![] }),
            "Working"
        );
        assert_eq!(
            describe_activity(&AgentActivity::Working {
                tools: vec!["srv:shell/exec".to_string()]
            }),
            "Working(srv:shell/exec)"
        );
        assert_eq!(
            describe_activity(&AgentActivity::Working {
                tools: vec!["a".to_string(), "b".to_string()]
            }),
            "Working(a, b)"
        );
        assert_eq!(
            describe_activity(&AgentActivity::Done { truncated: false }),
            "Done"
        );
        assert_eq!(
            describe_activity(&AgentActivity::Done { truncated: true }),
            "Done(truncated)"
        );
        assert_eq!(
            describe_activity(&AgentActivity::Failed {
                reason: "cancelled".to_string()
            }),
            "Failed(cancelled)"
        );
    }

    /// 没有 task 就是占位符，不是空字符串——`None` 和「写了但恰好是空串」不该
    /// 长得一样。
    #[test]
    fn missing_task_shows_a_placeholder_not_an_empty_string() {
        assert_eq!(describe_task(None), "(无任务文本)");
    }

    /// 超长 task 按字符数截断并带省略号；没超过上限的原样保留。
    #[test]
    fn long_task_is_truncated_with_an_ellipsis() {
        let long = "字".repeat(TASK_DISPLAY_MAX_CHARS + 10);
        let out = describe_task(Some(&long));
        assert_eq!(out.chars().count(), TASK_DISPLAY_MAX_CHARS + 1); // +1 是省略号
        assert!(out.ends_with('…'));

        let exact = "字".repeat(TASK_DISPLAY_MAX_CHARS);
        assert_eq!(describe_task(Some(&exact)), exact);
    }

    /// 多行 task（首条 user 消息本身带换行）折叠成一行，不撑爆树的排版。
    #[test]
    fn multiline_task_collapses_into_a_single_line() {
        assert_eq!(
            describe_task(Some("第一行\n第二行\n\n第三行")),
            "第一行 第二行 第三行"
        );
    }
}

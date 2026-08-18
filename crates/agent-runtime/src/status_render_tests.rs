//! `status_render` 的单元测试（207 随源文件一起从 `status_tool_tests.rs` 拆出来，
//! 红线 9——那个文件当时 392 行）。
//!
//! 这里测的是**渲染**这一半：一组现成的节点 → 一段字节确定的正文。
//! 「这次调用该看到哪些节点」是隔壁 `status_tool_tests.rs` 的事。

use super::*;

use agent_core::AgentActivity;

/// 造一格快照。`parent`/`depth` 从 id 现推——它们在 `agent_tree()` 里本来就是
/// `AgentId` 的投影，测试里手写一遍只会造出树里不可能出现的组合。
fn node(id: &str, activity: AgentActivity, task: Option<&str>) -> AgentNode {
    let id = AgentId::new(id);
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

fn body_of(nodes: &[AgentNode], caller: &str) -> String {
    let refs: Vec<&AgentNode> = nodes.iter().collect();
    render(&refs, &AgentId::new(caller))
}

/// 一个 agent 一行是这段正文的全部结构：任务文本里带换行也不许把它拆成两行
/// （否则模型会读出一个不存在的 agent）。
#[test]
fn a_task_with_newlines_is_flattened_to_stay_one_line_per_agent() {
    let nodes = vec![
        node("root", AgentActivity::Idle, None),
        node(
            "root/a1",
            AgentActivity::Idle,
            Some("第一行\n第二行\r\n第三行"),
        ),
    ];
    let body = body_of(&nodes, "root");
    assert_eq!(body.lines().count(), 3, "标题一行 + 一个 agent 一行：{body}");
    assert!(body.contains("第一行 第二行"), "{body}");
}

/// 长任务按**字符**截断（按字节切会切碎中文），并留一个看得出「还有」的记号。
#[test]
fn a_long_task_is_truncated_by_characters_with_a_marker() {
    let long = "很".repeat(TASK_CHARS + 20);
    let nodes = vec![node("root/a1", AgentActivity::Idle, Some(&long))];
    let body = body_of(&nodes, "root");
    let line = body.lines().nth(1).unwrap();
    let rendered = line.split_once("task=").unwrap().1;
    assert_eq!(
        rendered.chars().count(),
        TASK_CHARS + 1,
        "{TASK_CHARS} 个字符 + 一个省略号"
    );
    assert!(rendered.ends_with('…'), "{rendered}");
}

/// 五个 activity 变体的字面写法跟 docs/ORCHESTRATION.md §三那张表逐字对得上，
/// 且 depth / task 都在行上。
#[test]
fn every_activity_variant_has_a_stable_spelling() {
    let nodes = vec![
        node("root/a1", AgentActivity::Idle, None),
        node("root/a2", AgentActivity::Thinking, Some("想")),
        node(
            "root/a3",
            AgentActivity::Working {
                tools: vec!["srv:fs/read".into(), "srv:fs/list".into()],
            },
            Some("跑"),
        ),
        node(
            "root/a4",
            AgentActivity::Working { tools: Vec::new() },
            Some("忙"),
        ),
        node(
            "root/a5",
            AgentActivity::Done { truncated: false },
            Some("完"),
        ),
        node(
            "root/a6",
            AgentActivity::Done { truncated: true },
            Some("完"),
        ),
        node(
            "root/a7",
            AgentActivity::Failed {
                reason: "provider error: Auth".to_string(),
            },
            Some("砸"),
        ),
    ];
    // caller 取一个不在清单里的 id，免得 `(你)` 标记混进这几条字面断言。
    let body = body_of(&nodes, "root");
    let lines: Vec<&str> = body.lines().skip(1).collect();
    assert_eq!(lines[0], "root/a1 depth=1 Idle task=(无)");
    assert_eq!(lines[1], "root/a2 depth=1 Thinking task=想");
    assert_eq!(
        lines[2],
        "root/a3 depth=1 Working(srv:fs/read,srv:fs/list) task=跑"
    );
    assert_eq!(lines[3], "root/a4 depth=1 Working task=忙");
    assert_eq!(lines[4], "root/a5 depth=1 Done task=完");
    assert_eq!(lines[5], "root/a6 depth=1 Done(truncated) task=完");
    assert_eq!(
        lines[6],
        "root/a7 depth=1 Failed(provider error: Auth) task=砸"
    );
}

/// **调用者那一行标 `(你)`，别人不标**（207）：全树清单里模型得分得出哪个是自己。
/// 标记是行尾追加的，不影响前四段的解析。
#[test]
fn only_the_callers_own_line_is_marked() {
    let nodes = vec![
        node("root", AgentActivity::Idle, None),
        node("root/a1", AgentActivity::Idle, None),
        node("root/a2", AgentActivity::Idle, None),
    ];
    let body = body_of(&nodes, "root/a1");
    let marked: Vec<&str> = body.lines().filter(|l| l.ends_with(" (你)")).collect();
    assert_eq!(marked.len(), 1, "只该有一行被标：{body}");
    assert!(marked[0].starts_with("root/a1 "), "{body}");
}

/// 空集也得有一句话，不能回一段空正文让模型猜。
#[test]
fn an_empty_set_still_says_something() {
    let body = body_of(&[], "root");
    assert!(body.contains("没有活着的 agent"), "{body}");
    assert_eq!(body.lines().count(), 1, "{body}");
}

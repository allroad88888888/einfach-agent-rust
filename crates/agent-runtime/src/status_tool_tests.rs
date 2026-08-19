//! `status_tool` 的单元测试（红线 9：从 `status_tool.rs` 挪出来，源文件只留实现，
//! 跟既有的 `tool_table_tests.rs` / `ctx_tests.rs` 同款 `#[path]` 子模块）。
//!
//! 这里测的是**收窄与拒绝的判定**这条纯函数链（`observe` 吃一棵现成的 `AgentTree`，
//! 不需要 `Session`、不需要泵）。渲染那一半在 `status_render_tests.rs`（207 拆的）。
//! 「真的有两个子在跑时父调 status 拿到什么」是 `tests/status_indep_*.rs` 那三个
//! 端到端用例的事。
//!
//! **207 改写了这个文件的一半。** 决策 35 之前 `status` 只看得见调用者的严格后代，
//! 这里断言的是「上读 / 横读 / 自读全拒」；横读全开之后视野是整棵活树，
//! 那几条断言的是**相反**的事。

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

/// 两条分支各带一个孙子的一棵树：
///
/// ```text
/// root
/// ├── root/a1        Thinking      任务 A
/// │   └── root/a1/a1 Working       任务 A1
/// └── root/a2        Done          任务 B
///     └── root/a2/a1 Failed        任务 B1
/// ```
fn sample() -> AgentTree {
    AgentTree {
        nodes: vec![
            node(
                "root",
                AgentActivity::Working {
                    tools: vec![crate::SPAWN_TOOL.to_string()],
                },
                Some("总任务"),
            ),
            node("root/a1", AgentActivity::Thinking, Some("任务 A")),
            node(
                "root/a1/a1",
                AgentActivity::Working {
                    tools: vec!["srv:fs/read".to_string()],
                },
                Some("任务 A1"),
            ),
            node(
                "root/a2",
                AgentActivity::Done { truncated: false },
                Some("任务 B"),
            ),
            node(
                "root/a2/a1",
                AgentActivity::Failed {
                    reason: "cancelled".to_string(),
                },
                Some("任务 B1"),
            ),
        ],
    }
}

/// 正文里每一行开头那个 id。`root/a1` 是 `root/a1/a1` 的**子串**，
/// `body.contains("root/a1")` 这种断言在这棵树上是假绿灯——所以断言一律走这里，
/// 逐行取第一个字段比集合。
fn listed_ids(body: &str) -> Vec<&str> {
    body.lines()
        .skip(1)
        .map(|line| line.split(' ').next().unwrap())
        .collect()
}

fn observe_ok(tree: &AgentTree, caller: &str, input: Value) -> String {
    observe(tree, &AgentId::new(caller), &input).expect("该是一次成功的观测")
}

const WHOLE_TREE: [&str; 5] = ["root", "root/a1", "root/a1/a1", "root/a2", "root/a2/a1"];

// ── 收窄：整棵活树（207，决策 35）─────────────────────────────────────────────

/// 省略 `id`：**整棵树**，含调用者自己，顺序是 `AgentId` 路径序。
#[test]
fn omitting_id_lists_the_whole_live_tree_including_the_caller() {
    let body = observe_ok(&sample(), "root", json!({}));
    assert_eq!(listed_ids(&body), WHOLE_TREE);
    assert!(body.starts_with("这个会话现在的 agent（5 个"), "{body}");
}

/// **207 的行为核心**：一个子 agent 现在看得见祖先、兄弟、以及兄弟的孩子。
/// 决策 35 之前这里断言的是它只看得见 `root/a1/a1` 一个。
#[test]
fn a_child_now_sees_the_whole_tree_not_just_its_own_branch() {
    let body = observe_ok(&sample(), "root/a1", json!({}));
    assert_eq!(listed_ids(&body), WHOLE_TREE);
}

/// 叶子也一样看得见全树——它不再是「一个后代都没有」的那种空清单。
#[test]
fn a_leaf_sees_everyone_too() {
    let body = observe_ok(&sample(), "root/a1/a1", json!({}));
    assert_eq!(listed_ids(&body), WHOLE_TREE);
}

/// 给了 `id`：只看那一段，**含它自己**（问的就是「它在干啥」）。
#[test]
fn an_explicit_id_scopes_the_view_to_that_subtree_including_itself() {
    let body = observe_ok(&sample(), "root", json!({ "id": "root/a2" }));
    assert_eq!(listed_ids(&body), vec!["root/a2", "root/a2/a1"]);
}

/// **兄弟的 id 不再被拒**——这份清单里的 id 就是 `srv:agent/send` 的 `to`。
#[test]
fn a_siblings_id_is_no_longer_refused() {
    let body = observe_ok(&sample(), "root/a1", json!({ "id": "root/a2" }));
    assert_eq!(listed_ids(&body), vec!["root/a2", "root/a2/a1"]);
}

/// 祖先的 id 也不再被拒。
#[test]
fn an_ancestors_id_is_no_longer_refused() {
    let body = observe_ok(&sample(), "root/a1/a1", json!({ "id": "root/a1" }));
    assert_eq!(listed_ids(&body), vec!["root/a1", "root/a1/a1"]);
}

/// 问自己也不再被拒：它就是「以我为根的那一段」。
#[test]
fn asking_about_yourself_is_allowed_now() {
    let body = observe_ok(&sample(), "root/a1", json!({ "id": "root/a1" }));
    assert_eq!(listed_ids(&body), vec!["root/a1", "root/a1/a1"]);
}

/// 前后空白是模型常写出来的东西，不该因此判成「不在树上」。
#[test]
fn an_id_with_surrounding_whitespace_still_resolves() {
    let body = observe_ok(&sample(), "root", json!({ "id": "  root/a2  " }));
    assert_eq!(listed_ids(&body), vec!["root/a2", "root/a2/a1"]);
}

// ── 拒绝：只剩「不在活树上」一种 ─────────────────────────────────────────────

fn refusal(caller: &str, id: &str) -> String {
    observe(&sample(), &AgentId::new(caller), &json!({ "id": id })).expect_err("该被拒绝")
}

/// 没 spawn 过、那一轮被撤销了、或者干脆是别的会话那棵树上的 id——**三种情况现在
/// 落同一条拒绝路径**（207 之前「不是你的后代」是单独一条）。拒绝文本要点名是哪个
/// id，并把「现在活着的是哪些」一并给出，模型才知道下一步该问谁。
#[test]
fn an_id_absent_from_the_live_tree_says_so_and_names_what_is_alive() {
    for id in ["root/a9", "other/a1", "root/a1/a9"] {
        let err = refusal("root/a1", id);
        assert!(err.contains(id), "拒绝文本该点名是哪个 id：{err}");
        assert!(err.contains("活 agent 里"), "{err}");
        assert!(
            err.contains("root/a2/a1"),
            "拒绝文本该告诉它现在活着的是哪些：{err}"
        );
    }
}

// ── 入参解析 ───────────────────────────────────────────────────────────────

/// 缺省与显式 `null` 是同一件事（模型两种都会写）：看整棵树。
#[test]
fn a_missing_or_null_id_means_the_whole_tree() {
    assert!(parse(&json!({})).unwrap().is_none());
    assert!(parse(&json!({ "id": null })).unwrap().is_none());
    assert_eq!(
        parse(&json!({ "id": "root/a1" }))
            .unwrap()
            .unwrap()
            .as_str(),
        "root/a1"
    );
}

#[test]
fn a_non_string_or_blank_id_is_a_message_for_the_model_not_a_panic() {
    assert!(parse(&json!({ "id": 7 })).is_err());
    assert!(parse(&json!({ "id": ["root/a1"] })).is_err());
    assert!(parse(&json!({ "id": "   " })).is_err());
}

// ── 红线 11：逐字节确定 ────────────────────────────────────────────────────

/// 同一棵树两次序列化，**字节相同**。这段正文是 tool_result，从此每一轮都躺在
/// prompt 里——飘一个字节就是前缀缓存全丢（DeepSeek 上 120 倍）。
#[test]
fn the_same_tree_renders_to_the_same_bytes_twice() {
    let tree = sample();
    let first = observe_ok(&tree, "root", json!({}));
    let second = observe_ok(&tree, "root", json!({}));
    assert_eq!(first.as_bytes(), second.as_bytes());
}

/// **节点进来的顺序不影响出去的字节**：渲染前自己排一次序。`live_agents()` 今天
/// 是排好的，但那是被调方的实现承诺——这条断言证明这里不靠它。
#[test]
fn a_shuffled_node_order_renders_to_the_very_same_bytes() {
    let sorted = observe_ok(&sample(), "root", json!({}));

    let mut shuffled = sample();
    shuffled.nodes.reverse();
    assert_eq!(
        observe_ok(&shuffled, "root", json!({})).as_bytes(),
        sorted.as_bytes()
    );

    let mut rotated = sample();
    rotated.nodes.swap(1, 3);
    rotated.nodes.swap(0, 4);
    assert_eq!(
        observe_ok(&rotated, "root", json!({})).as_bytes(),
        sorted.as_bytes()
    );
}

/// **同一棵树，换个人调，节点顺序不变**（207）：视野是全树之后，清单本身不该随
/// 调用者变——变的只有哪一行末尾标 `(你)`。顺序一漂就是每个 agent 各持一份不同的
/// 前缀，红线 11 的账在多 agent 会话里翻倍。
#[test]
fn the_listing_order_does_not_depend_on_who_is_asking() {
    let tree = sample();
    let by_root = observe_ok(&tree, "root", json!({}));
    let by_child = observe_ok(&tree, "root/a2/a1", json!({}));
    assert_eq!(listed_ids(&by_root), listed_ids(&by_child));
    assert_ne!(by_root, by_child, "但 (你) 该标在不同的行上");
}

// ── 渲染出来的行的形状 ─────────────────────────────────────────────────────

/// 深度是真的深度，不是「1」写死。
#[test]
fn depth_comes_from_the_path() {
    let body = observe_ok(&sample(), "root", json!({}));
    assert!(body.contains("root/a1/a1 depth=2 "), "{body}");
}

/// **任何 agent 的消息正文都不在这里**（决策 35 §一：`Messages` 在 core 层放行，
/// 但工具层不给模型开按槽位读它的入口；正文是 collect 的事）。`AgentNode` 压根
/// 没有装正文的字段——这条断言守的是「将来别人给它加一个」。
#[test]
fn the_body_carries_activity_and_task_only() {
    // caller 取一个不在树上的 id，免得 `(你)` 标记混进字段计数。
    let body = observe_ok(&sample(), "nobody", json!({}));
    for line in body.lines().skip(1) {
        let fields: Vec<&str> = line.splitn(4, ' ').collect();
        assert_eq!(
            fields.len(),
            4,
            "一行只有 id / depth / activity / task 四段：{line}"
        );
        assert!(fields[1].starts_with("depth="), "{line}");
        assert!(fields[3].starts_with("task="), "{line}");
    }
}

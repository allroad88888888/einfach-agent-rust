//! `status_tool` 的单元测试（红线 9：从 `status_tool.rs` 挪出来，源文件只留实现，
//! 跟既有的 `tool_table_tests.rs` / `ctx_tests.rs` 同款 `#[path]` 子模块）。
//!
//! 这里测的是**收窄 + 渲染**这条纯函数链（`observe` 吃一棵现成的 `AgentTree`，
//! 不需要 `Session`、不需要泵）。「真的有两个子在跑时父调 status 拿到什么」是
//! `tests/status_indep_*.rs` 那三个端到端用例的事。

use super::*;

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

// ── 收窄：只往下读（红线 10）────────────────────────────────────────────────

/// root 省略 `id`：四个后代全在，**自己不在**（它不需要别人告诉它自己在干啥），
/// 顺序是 `AgentId` 路径序。
#[test]
fn omitting_id_lists_every_strict_descendant_of_the_caller() {
    let body = observe_ok(&sample(), "root", json!({}));
    assert_eq!(
        listed_ids(&body),
        vec!["root/a1", "root/a1/a1", "root/a2", "root/a2/a1"]
    );
    assert!(body.starts_with("你的子 agent（4 个"), "{body}");
}

/// **红线 10 的正面用例**：`root/a1` 只看得到自己那一支的孙子——祖先（root）、
/// 兄弟（root/a2）和兄弟的孩子（root/a2/a1）一个都不该出现。
#[test]
fn a_child_sees_only_its_own_branch_never_its_ancestor_or_siblings() {
    let body = observe_ok(&sample(), "root/a1", json!({}));
    assert_eq!(listed_ids(&body), vec!["root/a1/a1"]);
}

/// 叶子没有后代：空集也得有一句话，不能回一段空正文让模型猜。
#[test]
fn a_leaf_gets_a_sentence_not_an_empty_body() {
    let body = observe_ok(&sample(), "root/a1/a1", json!({}));
    assert!(body.contains("没有子 agent"), "{body}");
    assert!(listed_ids(&body).is_empty(), "{body}");
}

/// 给了 `id`：只看那一段，**含它自己**（问的就是「它在干啥」）。
#[test]
fn an_explicit_id_scopes_the_view_to_that_subtree_including_itself() {
    let body = observe_ok(&sample(), "root", json!({ "id": "root/a2" }));
    assert_eq!(listed_ids(&body), vec!["root/a2", "root/a2/a1"]);
}

/// 前后空白是模型常写出来的东西，不该因此判成「不是我的后代」。
#[test]
fn an_id_with_surrounding_whitespace_still_resolves() {
    let body = observe_ok(&sample(), "root", json!({ "id": "  root/a2  " }));
    assert_eq!(listed_ids(&body), vec!["root/a2", "root/a2/a1"]);
}

// ── 拒绝：上读 / 横读 / 自读 / 不在树上 ─────────────────────────────────────

fn refusal(caller: &str, id: &str) -> String {
    observe(&sample(), &AgentId::new(caller), &json!({ "id": id })).expect_err("该被拒绝")
}

/// 上读（祖先）、横读（兄弟、兄弟的孩子、别的树）全拒，而且**拒绝文本里带上
/// 「你能看的是哪些」**——模型才知道下一步该问谁。
#[test]
fn reading_upward_or_sideways_is_refused_and_the_refusal_names_what_is_visible() {
    for id in ["root", "root/a2", "root/a2/a1", "other/a1"] {
        let err = refusal("root/a1", id);
        assert!(err.contains(id), "拒绝文本该点名是哪个 id：{err}");
        assert!(
            err.contains("root/a1/a1"),
            "拒绝文本该告诉它能看的是哪些：{err}"
        );
    }
}

/// 自读也拒：规则「id 必须是你的后代」一条没有例外。拒绝文本里得有那句出路
/// （省掉 id）——否则模型只会换个写法再撞一次。
#[test]
fn asking_about_yourself_is_refused_with_a_way_out() {
    let err = refusal("root/a1", "root/a1");
    assert!(err.contains("省略 id"), "{err}");
}

/// 形状对但树上没有（没 spawn 过 / 那一轮被撤销了）：跟「不是你的后代」分开报，
/// 两件事模型的下一步不一样。
#[test]
fn an_id_shaped_like_a_descendant_but_absent_from_the_live_tree_says_so() {
    let err = refusal("root", "root/a9");
    assert!(err.contains("活子树"), "{err}");
    assert!(err.contains("root/a9"), "{err}");
}

/// 一个后代都没有的调用者被拒时，也得说清「你现在一个都没有」。
#[test]
fn a_leaf_that_asks_about_someone_else_is_told_it_has_none() {
    let err = observe(
        &sample(),
        &AgentId::new("root/a1/a1"),
        &json!({ "id": "root/a2" }),
    )
    .expect_err("该被拒绝");
    assert!(err.contains("一个子 agent 都没有"), "{err}");
}

// ── 入参解析 ───────────────────────────────────────────────────────────────

/// 缺省与显式 `null` 是同一件事（模型两种都会写）：看自己的全部后代。
#[test]
fn a_missing_or_null_id_means_the_callers_own_subtree() {
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

/// 一个后代一行是这段正文的全部结构：任务文本里带换行也不许把它拆成两行
/// （否则模型会读出一个不存在的 agent）。
#[test]
fn a_task_with_newlines_is_flattened_to_stay_one_line_per_descendant() {
    let tree = AgentTree {
        nodes: vec![
            node("root", AgentActivity::Idle, None),
            node(
                "root/a1",
                AgentActivity::Idle,
                Some("第一行\n第二行\r\n第三行"),
            ),
        ],
    };
    let body = observe_ok(&tree, "root", json!({}));
    assert_eq!(body.lines().count(), 2, "标题一行 + 一个后代一行：{body}");
    assert!(body.contains("第一行 第二行"), "{body}");
}

/// 长任务按**字符**截断（按字节切会切碎中文），并留一个看得出「还有」的记号。
#[test]
fn a_long_task_is_truncated_by_characters_with_a_marker() {
    let long = "很".repeat(TASK_CHARS + 20);
    let tree = AgentTree {
        nodes: vec![
            node("root", AgentActivity::Idle, None),
            node("root/a1", AgentActivity::Idle, Some(&long)),
        ],
    };
    let body = observe_ok(&tree, "root", json!({}));
    let line = body.lines().nth(1).unwrap();
    let rendered = line.split_once("task=").unwrap().1;
    assert_eq!(
        rendered.chars().count(),
        TASK_CHARS + 1,
        "{TASK_CHARS} 个字符 + 一个省略号"
    );
    assert!(rendered.ends_with('…'), "{rendered}");
}

// ── 渲染的字面形状 ─────────────────────────────────────────────────────────

/// 五个 activity 变体的字面写法跟 docs/ORCHESTRATION.md §三那张表逐字对得上，
/// 且 depth / task 都在行上。
#[test]
fn every_activity_variant_has_a_stable_spelling() {
    let tree = AgentTree {
        nodes: vec![
            node("root", AgentActivity::Idle, None),
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
        ],
    };
    let body = observe_ok(&tree, "root", json!({}));
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

/// 深度是真的深度，不是「1」写死。
#[test]
fn depth_comes_from_the_path() {
    let body = observe_ok(&sample(), "root", json!({}));
    assert!(body.contains("root/a1/a1 depth=2 "), "{body}");
}

/// **子 agent 的消息正文不在这里**（ORCHESTRATION §三/五：正文是 collect 的事）。
/// `AgentNode` 压根没有装正文的字段——这条断言守的是「将来别人给它加一个」。
#[test]
fn the_body_carries_activity_and_task_only() {
    let body = observe_ok(&sample(), "root", json!({}));
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

// ── 工具说明书（071）─────────────────────────────────────────────────────────

/// 工具声明的固定事实：全名、可选的 `id`，以及描述里那句分水岭——**不返回子的
/// 正文**，而且「正文从哪来」必须**分前台/后台两种 spawn 说**。
///
/// 051 写的原文是「正文会在那次 spawn 调用的结果里回到你这里」，052 加了
/// `background=true` 之后它成了假话（后台 spawn 只回 `{"agent_id":...}`，正文要
/// 用 `collect` 领）。这段字符串每一轮都进 prompt，模型每次都读到并可能照它办事，
/// 所以它需要一条测试守着。
///
/// **断的是关键子串，不是整段文案。** 措辞会随经验改（它就是拿来调的），逐字
/// 断言只会让下一个改文案的人顺手把测试一起改掉——那条测试从此什么都不守。
/// 这里断的是「说法和 052/053 的实际行为对不对得上」：提没提 `collect`、有没有
/// 把两种 spawn 分开。工具名走 [`crate::COLLECT_TOOL`] 常量而不是手抄字符串，
/// 于是「collect 改了名而这段描述没跟上」也一样红。
#[test]
fn the_spec_tells_the_model_where_a_childs_answer_actually_comes_from() {
    let spec = status_spec();
    let text = &*spec.description;
    assert_eq!(&*spec.name, STATUS_TOOL);
    assert!(text.contains("不返回子 agent 的回答正文"), "{text}");
    // 两种 spawn 得各说各的，不能只留一句对其中一种成立的话。
    assert!(
        text.contains("前台"),
        "前台那条路（正文从 spawn 槽回来）得说：{text}"
    );
    assert!(text.contains("background=true"), "后台那条路得点名：{text}");
    // 后台子的正文只有这一条出路，描述里必须点名它。
    assert!(
        text.contains(crate::COLLECT_TOOL),
        "后台子的正文要用 collect 领：{text}"
    );
    assert_eq!(spec.schema["properties"]["id"]["type"], "string");
    assert!(
        spec.schema["required"].is_null(),
        "id 是可选的，不该有 required"
    );
}

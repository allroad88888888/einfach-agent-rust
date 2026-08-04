//! [`ToolTable::without_builtins`] 的单测（076）。
//!
//! 每一条都能说出「改坏哪一行 → 它怎么红」：
//!
//! | 测试 | 它看住的那一行 |
//! |---|---|
//! | [`a_disabled_name_leaves_the_table_entirely`] | `specs.retain(..)`（删掉 → 关了等于没关） |
//! | [`the_survivors_keep_their_original_order`] | `retain` 换成「重建 + 排序」之类 → 红线 11 的前缀当场漂 |
//! | [`an_empty_switch_changes_nothing`] | 早返回那一支 + `retain` 的谓词取反 |
//! | [`an_unknown_name_is_silently_ignored_here`] | 有人在这里加了 `panic!`/`debug_assert!` |
//! | [`a_shuffled_or_duplicated_switch_gives_the_same_table`] | 判据从「集合成员」滑成「顺序/计数」 |
//! | [`the_reversibility_maps_lose_the_same_names`] | 两行映射 `retain` 少写一行 |

use std::sync::Arc;

use agent_core::{Reversibility, ToolSpec};

use super::*;

fn names(table: &ToolTable) -> Vec<String> {
    table.specs().iter().map(|s| s.name.to_string()).collect()
}

fn off(list: &[&str]) -> Vec<Arc<str>> {
    list.iter().map(|n| Arc::from(*n)).collect()
}

/// 关掉的那个**连名字都不在表里**——`specs()` 少一项、`declares()` 为假。
///
/// `declares()` 那半条不是重复：spawn 的截获闸问的正是它（`crate::dispatch`），
/// 一个「渲染时才滤」的实现会让 `specs()` 少一项但 `declares()` 仍为真，模型硬猜
/// 一个它从没见过的名字照样能凭空长出一棵子 agent 树。
#[test]
fn a_disabled_name_leaves_the_table_entirely() {
    let full = ToolTable::with_shell().with_spawn(agent_core::AgentLimits::default()).with_status();
    assert!(full.declares("srv:agent/spawn"), "夹具前提：这一档本来有 spawn");

    let reduced = ToolTable::with_shell()
        .with_spawn(agent_core::AgentLimits::default())
        .with_status()
        .without_builtins(&off(&["srv:agent/spawn"]));

    assert!(!reduced.declares("srv:agent/spawn"), "关掉之后 declares 必须为假——spawn 的截获闸问的就是它");
    assert!(!names(&reduced).contains(&"srv:agent/spawn".to_string()));
    assert!(reduced.declares("srv:agent/status"), "只关点名的那一个，别的一件不许少");
}

/// 幸存下来的那些**保持五档原有的相对次序**（红线 11：既有顺序是契约）。
#[test]
fn the_survivors_keep_their_original_order() {
    let baseline = names(&ToolTable::standard());
    let reduced = names(&ToolTable::standard().without_builtins(&off(&["read_file", "save_file"])));

    let expected: Vec<String> = baseline.iter().filter(|n| *n != "read_file" && *n != "save_file").cloned().collect();
    assert_eq!(reduced, expected, "剔除不许顺带重排——工具表在 prompt 最前面（红线 11）");
}

/// 空开关 = 空操作：表跟没调过这个函数**完全一样**。
///
/// 「不带这个字段时工具表与今天逐字节相同」这条验收的最小落点（字节那一半在
/// `tests/disabled_builtins_are_byte_deterministic.rs`）。
#[test]
fn an_empty_switch_changes_nothing() {
    for build in [ToolTable::builtin as fn() -> ToolTable, ToolTable::with_shell, ToolTable::standard_local, ToolTable::standard] {
        assert_eq!(names(&build().without_builtins(&[])), names(&build()), "空开关不该动任何一项");
    }
}

/// 认不出的名字在这里**静默跳过**：这一步每次开会话都跑，作者早不在场
/// （报错的位置在 HTTP 路由，069 §拍板「在最早能报给作者的点上失败」）。
#[test]
fn an_unknown_name_is_silently_ignored_here() {
    let baseline = names(&ToolTable::with_shell());
    let reduced = names(&ToolTable::with_shell().without_builtins(&off(&["srv:nope/nope", "web:crm/lookup"])));
    assert_eq!(reduced, baseline, "认不出的名字不该改变任何东西，也不该 panic");
}

/// 判据是**集合成员关系**：同一份名单换个顺序、或者多写一个重复项，出来的表一样。
///
/// 红线 11 在这一层的形状——列表顺序不可以泄漏进工具表（进而泄漏进 prompt 字节）。
#[test]
fn a_shuffled_or_duplicated_switch_gives_the_same_table() {
    let one = names(&ToolTable::standard().without_builtins(&off(&["read_file", "save_file", "list_files"])));
    let two = names(&ToolTable::standard().without_builtins(&off(&["save_file", "list_files", "read_file"])));
    let three = names(&ToolTable::standard().without_builtins(&off(&["list_files", "read_file", "read_file", "save_file"])));

    assert_eq!(one, two, "关闭列表换个顺序就换一张表 = 顺序泄漏进 prompt（红线 11）");
    assert_eq!(one, three, "同一个名字写两遍跟写一遍是同一个意思");
}

/// 两张可逆性映射跟着一起剔：表里没有 spec、映射里却还留着一条，正是 075 的
/// `push_spec` 花一整段说明去避免的那种隐式耦合。
///
/// 这一条走的是「调用顺序不该被依赖」那一支——生产里 `without_builtins` 排在
/// `with_mcp`/`with_host_tools` 之前，此刻两张映射本来就是空的；这里故意反着接，
/// 逼实现自己保证一致，而不是靠「目前的调用顺序恰好安全」。
#[test]
fn the_reversibility_maps_lose_the_same_names() {
    let spec = ToolSpec {
        name: Arc::from("web:crm/lookup"),
        description: Arc::from("查档案"),
        schema: Arc::new(serde_json::json!({ "type": "object" })),
    };
    let table = ToolTable::builtin()
        .with_host_tools(vec![(spec, Reversibility::Pure)])
        .without_builtins(&off(&["web:crm/lookup"]));

    assert!(!table.declares("web:crm/lookup"));
    assert_eq!(
        table.snapshot("web:crm/lookup", Arc::new(serde_json::json!({}))).reversibility,
        Reversibility::Irreversible,
        "spec 剔了、可逆性映射还留着 = 一条查得到却调不出来的幽灵记录"
    );
}

//! 099 验收：`SendPlan` 类型自身的行为契约——构造、清除、推进边界的不变量，
//! 以及序列化形状（逐字节确定、不含摘要正文这个大值）。
//!
//! 投影函数 `project` 的行为单独测，见 `send_plan_project.rs`——这个文件只管
//! `SendPlan` 自己，不喂它完整历史。

use std::sync::Arc;

use imbl::Vector;

use agent_core::ids::{SummaryId, ToolCallId};
use agent_core::value::message::Message;
use agent_core::value::send_plan::{SendPlan, project};

fn id(s: &str) -> ToolCallId {
    ToolCallId::new(s)
}

fn sid(s: &str) -> SummaryId {
    SummaryId::new(s)
}

/// 恒等元：`new()` 就是「不清任何东西、边界 0、无摘要」，且从没被压过。
#[test]
fn new_is_the_identity_plan() {
    let plan = SendPlan::new();
    assert!(plan.cleared().is_empty());
    assert_eq!(plan.boundary(), 0);
    assert!(plan.summary().is_none());
    assert!(plan.is_pristine());
}

#[test]
fn default_equals_new() {
    assert_eq!(SendPlan::default(), SendPlan::new());
}

/// `is_pristine`：任何一次成功的变更之后都不再是 pristine——`encode` 的快路
/// 判断要能靠它分辨「真的没压过」和「压过又变回原样」。
#[test]
fn is_pristine_flips_false_after_any_mutation() {
    let mut cleared = SendPlan::new();
    assert!(cleared.is_pristine());
    cleared.clear_tool_results([id("call_1")]);
    assert!(!cleared.is_pristine());

    let mut advanced = SendPlan::new();
    advanced.advance_boundary(1, None).unwrap();
    assert!(!advanced.is_pristine());
}

/// 幂等：已经在已清列表里的 id 再清一次不重复加入。
#[test]
fn clear_tool_results_is_idempotent() {
    let mut plan = SendPlan::new();
    plan.clear_tool_results([id("call_1")]);
    plan.clear_tool_results([id("call_1")]);
    assert_eq!(plan.cleared().to_vec(), vec![id("call_1")]);
}

/// 保持**首次加入的顺序**（红线 11）：已存在的 id 原地不动，新的追加在最后——
/// 不是按值排序，不是最近使用排到前面。
#[test]
fn clear_tool_results_keeps_first_insertion_order() {
    let mut plan = SendPlan::new();
    plan.clear_tool_results([id("z"), id("a")]);
    assert_eq!(plan.cleared().to_vec(), vec![id("z"), id("a")]);

    plan.clear_tool_results([id("z"), id("m")]);
    assert_eq!(plan.cleared().to_vec(), vec![id("z"), id("a"), id("m")]);
}

/// 行为化验证「不是 HashMap/HashSet」：同一个 id 集合，加入顺序不同 →
/// `SendPlan` 不相等、序列化结果也不同。`BTreeSet` 会把两种插入顺序都归一成
/// 同一个排序结果，`HashSet` 的迭代顺序跟插入顺序无关且不稳定——两者都会让
/// 这条测试要么恒等、要么随机抖动；只有保序容器（`Vec` + 手动去重）会稳定地
/// 把插入顺序如实序列化出来，这正是接口文档「保持首次加入的顺序」的字面要求。
#[test]
fn insertion_order_survives_serialization_not_a_set() {
    let mut plan_ab = SendPlan::new();
    plan_ab.clear_tool_results([id("call_a"), id("call_b")]);

    let mut plan_ba = SendPlan::new();
    plan_ba.clear_tool_results([id("call_b"), id("call_a")]);

    assert_ne!(plan_ab, plan_ba, "插入顺序不同应该是不同的 SendPlan 值");

    let s_ab = serde_json::to_string(&plan_ab).unwrap();
    let s_ba = serde_json::to_string(&plan_ba).unwrap();
    assert_ne!(s_ab, s_ba, "序列化结果要如实反映插入顺序，不能被归一化掉");
}

/// 正常前进：边界与摘要引用一起改——一次调用改两个字段，不留中间态。
#[test]
fn advance_boundary_moves_forward_and_sets_summary() {
    let mut plan = SendPlan::new();
    plan.advance_boundary(3, Some(sid("sum_1"))).unwrap();
    assert_eq!(plan.boundary(), 3);
    assert_eq!(plan.summary(), Some(&sid("sum_1")));
}

/// 往回退返回 `Err`，边界原地不动——不静默忽略。
#[test]
fn advance_boundary_rejects_going_backwards() {
    let mut plan = SendPlan::new();
    plan.advance_boundary(5, None).unwrap();

    assert!(plan.advance_boundary(3, None).is_err());
    assert_eq!(plan.boundary(), 5, "拒绝之后边界必须原地不动");
    assert_eq!(plan.summary(), None, "拒绝的调用不能把摘要也悄悄改了");
}

/// 原地不算前进：`next == boundary()` 落在「`next <= boundary()` 返回 `Err`」
/// 那一半里，不能因为「没往回退」就放行。
#[test]
fn advance_boundary_rejects_staying_in_place() {
    let mut plan = SendPlan::new();
    plan.advance_boundary(5, None).unwrap();

    assert!(plan.advance_boundary(5, Some(sid("sum_1"))).is_err());
    assert_eq!(plan.boundary(), 5);
    assert_eq!(plan.summary(), None);
}

/// 恒等元的边界是 0，`advance_boundary(0, ..)` 同样落在 `next <= boundary()`
/// 里——不能因为「还没推进过」就把 0 当成允许的起点。
#[test]
fn advance_boundary_rejects_zero_on_pristine_plan() {
    let mut plan = SendPlan::new();
    assert!(plan.advance_boundary(0, None).is_err());
    assert_eq!(plan.boundary(), 0);
    assert!(plan.is_pristine(), "被拒绝的调用不能弄脏 pristine 状态");
}

/// 一次成功的 `advance_boundary` 之后，另一次严格更大的调用可以再前进。
#[test]
fn advance_boundary_allows_further_forward_moves() {
    let mut plan = SendPlan::new();
    plan.advance_boundary(2, None).unwrap();
    plan.advance_boundary(5, Some(sid("sum_2"))).unwrap();
    assert_eq!(plan.boundary(), 5);
    assert_eq!(plan.summary(), Some(&sid("sum_2")));
}

#[test]
fn serde_roundtrip_with_content() {
    let mut plan = SendPlan::new();
    plan.clear_tool_results([id("call_1"), id("call_2")]);
    plan.advance_boundary(4, Some(sid("sum_1"))).unwrap();

    let s = serde_json::to_string(&plan).unwrap();
    let back: SendPlan = serde_json::from_str(&s).unwrap();
    assert_eq!(back, plan);
}

/// 恒等元本身也要能 serde 往返——它不是一条特殊路径。
#[test]
fn serde_roundtrip_pristine() {
    let plan = SendPlan::new();
    let s = serde_json::to_string(&plan).unwrap();
    let back: SendPlan = serde_json::from_str(&s).unwrap();
    assert_eq!(back, plan);
}

/// 摘要正文不在 `SendPlan` 里（红线 5）：`SendPlan` 只存一个 `SummaryId`，
/// `project` 的 `summary_text` 参数是**调用方另外传入**的，不会被吸收进
/// `SendPlan` 本身。用 100 字节和 10KB 两份摘要正文各喂一次 `project`，
/// 之后 `SendPlan` 自己的序列化结果必须原地不变——如果摘要正文哪天不小心
/// 混进了 `SendPlan`，这条会先炸。
#[test]
fn serialized_size_does_not_grow_with_summary_body_length() {
    let mut plan = SendPlan::new();
    plan.advance_boundary(2, Some(sid("sum_1"))).unwrap();
    let before = serde_json::to_vec(&plan).unwrap();

    let empty_history: Vector<Message> = Vector::new();
    let short_summary: Arc<str> = Arc::from("x".repeat(100));
    let long_summary: Arc<str> = Arc::from("x".repeat(10_000));

    let _ = project(&empty_history, &plan, Some(&short_summary));
    let after_short = serde_json::to_vec(&plan).unwrap();

    let _ = project(&empty_history, &plan, Some(&long_summary));
    let after_long = serde_json::to_vec(&plan).unwrap();

    assert_eq!(before.len(), after_short.len());
    assert_eq!(before.len(), after_long.len());
    assert_eq!(after_short, after_long);
}

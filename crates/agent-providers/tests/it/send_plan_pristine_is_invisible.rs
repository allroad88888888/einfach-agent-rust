//! Issue 100 验收里最要紧的一条：pristine 的 `SendPlan` 下，`encode` 输出与
//! **不经过投影时**逐字节相同——不用这个功能就该逐字节不变（065/067 同款
//! 纪律），已有的一整批 golden / determinism 测试因此不用改一个字。
//!
//! 这条测试不手抄任何期望字节串（抄一份期望值，抄错一个字都测不出来）。做法是
//! **结构性比较两条路**，输入完全一样：
//!
//! - 「不经过投影」这条路，用的正是 `encode_determinism.rs` / `three_providers.rs`
//!   已经在用、100 之前就存在的 [`support::ingredients`]——history 直接喂给
//!   `Ingredients`，不碰 `send_plan`。
//! - 「经过投影，但 plan 是 pristine」这条路，先把**同一份** history 真的过一遍
//!   [`project`]`(&history, &SendPlan::new(), None)`，拿到它的真实输出接进
//!   `Ingredients` 再 `encode`——不是假定"099 证过恒等元"就跳过这一步去抄一份
//!   等于原样的历史，而是让 100 接的这条线自己产出结果。
//!
//! 两条路算出来的 body 必须逐字节相等：如果 100 的接线在 pristine 时引入了任何
//! 无条件的形状变化（哪怕一个字节），这条就会红，而且红的时候不需要预先写好
//! "该是什么"，两条路自己就会在 diff 里说清楚哪里不一样。

use imbl::Vector;

use agent_core::value::send_plan::project;
use agent_core::{Message, RequestIntent, SendPlan, SystemChunk, ToolSpec};
use agent_providers::Ingredients;

use crate::support;

fn rich_system() -> Vec<SystemChunk> {
    vec![
        support::sys_chunk("base", "你是一个称职的助手。"),
        support::sys_chunk("skill:fs", "读写文件时先确认路径存在。"),
    ]
}

fn rich_messages() -> Vec<Message> {
    vec![
        support::user_text(1, "帮我看看北京天气"),
        support::assistant_text(2, "好的，我先查一下。"),
    ]
}

fn rich_tools() -> Vec<ToolSpec> {
    vec![
        support::tool_spec("srv:fs/read", "read a file", support::schema_order_a()),
        support::tool_spec(
            "srv:fs/write",
            "write a file",
            serde_json::json!({"type": "object"}),
        ),
    ]
}

#[test]
fn pristine_send_plan_matches_encode_without_any_projection_across_three_providers() {
    let system = rich_system();
    let messages = rich_messages();
    let tools = rich_tools();
    let late_tools: Vec<ToolSpec> = vec![];

    // 「投影，但 plan 是 pristine」这条路的输入：把同一份 messages 真的过一遍
    // `project`，不是假定它等于原样就跳过这一步。
    let history: Vector<Message> = Vector::from(messages.clone());
    let projected = project(&history, &SendPlan::new(), None);

    for (family, provider) in support::providers() {
        let config = support::config_for_family(family);

        // 「不经过投影」——100 之前就存在的构造器，原样喂 messages。
        let before = support::ingredients(
            &system,
            &messages,
            &tools,
            &late_tools,
            &config,
            RequestIntent::Free,
            None,
        );
        let body_before = provider.encode(&before).body;

        // 「经过投影，plan pristine」——同样的 system/tools/config，只有
        // messages 换成 `project` 的真实输出。
        let after = Ingredients {
            system: &system,
            messages: &projected,
            tools: &tools,
            late_tools: &late_tools,
            late_system: &[],
            config: &config,
            intent: RequestIntent::Free,
            prev_prefix: None,
        };
        let body_after = provider.encode(&after).body;

        assert_eq!(
            body_before, body_after,
            "{family}：pristine SendPlan 下，投影过的 encode 输出该跟不投影时逐字节相同"
        );
    }
}

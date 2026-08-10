//! 反向锁：**压缩轮照样进第 3 层的滚动窗口**，不许被豁免。
//!
//! 这是主会话 2026-08-10 回退一处越界改动之后补的锁。103 实现期间，`guard.rs`
//! 一度按 103 早期一条写错的验收，把 `DriftVerdict::Expected`（也就是压缩轮）
//! 整轮排除出窗口。回退的理由不是「原验收写错了」这么轻——那个排除会**开一个
//! 正好落在灾难场景上的盲区**：
//!
//! 压缩要是因为 bug 变成每轮都开火（「每轮改中段、每轮全价」，096 决策记录里
//! 反复点的那个形态，在 DeepSeek 上一次压缩≈120 轮命中的钱），那就是**每一轮
//! 都判 `Expected`** → 每一轮都被排除 → 窗口里一条观测都没有 →
//! 第 3 层永远不告警。**唯一能抓这个形态的一层，恰恰在这个形态下失明。**
//!
//! 而一次性的压缩代价本来就已经被容忍过一次了：`DEFAULT_CONSECUTIVE_ALERT` 是 3，
//! `cache/window.rs` 的模块文档写着「单轮低命中是正常现象（换前缀、压缩、第一次
//! 见这个变体）。连续三轮说明不是一次性代价」。再排除一次就是重复计算这份容忍。
//!
//! 真正只该豁免的是**失明轮**（`TurnHit::Blind`，provider 根本没报 `cached`），
//! 那由 `TurnHit::from_usage` 自己判，不在 `guard.rs`。
//!
//! 跟 `prefix_intent_single_compaction_round_is_not_chronic.rs` 是一对：
//! 那条钉「一轮压缩不算慢性」，这条钉「连着压三轮就是慢性，必须报」。
//! **只有那一条的话，一个「压缩轮永不进窗口」的实现照样全绿。**

use agent_core::cache::WindowVerdict;
use agent_core::{AgentId, Session};
use agent_runtime::{RunnerEvent, run_turn};

use crate::support::{self, ScriptedResponse};

fn sse_text_with_hit(text: &str, prompt: u32, cached: u32) -> ScriptedResponse {
    let miss = prompt.saturating_sub(cached);
    let content_line = format!(
        r#"data: {{"choices":[{{"index":0,"delta":{{"role":"assistant","content":"{text}"}},"finish_reason":null}}]}}"#
    );
    let usage_line = format!(
        r#"data: {{"choices":[{{"index":0,"delta":{{"content":""}},"finish_reason":"stop"}}],"usage":{{"prompt_tokens":{prompt},"completion_tokens":10,"prompt_cache_hit_tokens":{cached},"prompt_cache_miss_tokens":{miss}}}}}"#
    );
    ScriptedResponse::Sse(vec![
        Box::leak(content_line.into_boxed_str()),
        Box::leak(usage_line.into_boxed_str()),
        "data: [DONE]",
    ])
}

fn last_window(events: &[RunnerEvent]) -> WindowVerdict {
    events
        .iter()
        .filter_map(|e| match e {
            RunnerEvent::TurnGuard { report, .. } => Some(report.window),
            _ => None,
        })
        .last()
        .expect("该有 GuardReport")
}

/// 连续三轮都压缩、都 0% 命中 → **必须**报 `ChronicMiss`。
///
/// 这正是「压缩因为 bug 每轮开火」的形态。压缩轮要是被豁免出窗口，
/// 这里会一条观测都没有、永远不告警，测试当场红。
#[test]
fn three_consecutive_compaction_rounds_at_zero_hit_do_trigger_chronic_miss() {
    let dir = support::temp_dir("prefix-intent-compaction-still-enters-window");

    let port = support::spawn_scripted_server(vec![
        sse_text_with_hit("垫底的一轮", 500, 400), // 80%，健康，给后面留东西可推
        sse_text_with_hit("压缩轮 1", 500, 0),
        sse_text_with_hit("压缩轮 2", 500, 0),
        sse_text_with_hit("压缩轮 3", 500, 0),
    ]);
    let (mut ctx, events) = support::build_ctx(port, &dir);
    let mut session = Session::new(AgentId::root());
    let root = AgentId::root();

    run_turn(&mut session, &mut ctx, "第一句话").expect("轮 0 不该是 source failure");

    // 连着三轮：每轮都把边界往前推一格（每轮 `SendPlan` 都变 → 每轮都判
    // `PrefixIntent::Intentional` → 每轮 drift 都是 `Expected`），且每轮 0% 命中。
    for round in 1..=3 {
        let history_len = session.messages_of(&root).len();
        session.begin_turn();
        session
            .advance_boundary(&root, history_len, None)
            .unwrap_or_else(|e| panic!("第 {round} 轮推边界到 {history_len} 该被接受：{e:?}"));
        run_turn(&mut session, &mut ctx, "继续")
            .unwrap_or_else(|e| panic!("第 {round} 轮不该是 source failure：{e:?}"));
    }

    let events = events.borrow();
    let window = last_window(&events);
    assert!(
        matches!(window, WindowVerdict::ChronicMiss { streak: 3, .. }),
        "连着三轮压缩全 0% 命中必须报 ChronicMiss——压缩轮要是被豁免出窗口，\
         「压缩每轮开火」这个最贵的 bug 就永远没人抓得住。实际：{window:?}\n{events:#?}"
    );
}

//! 103 验收第四条（主会话 2026-08-10 改判后的版本，见下方「跟原验收的出入」）：
//! 压缩轮单独不触发第 3 层（滚动窗口）的 `ChronicMiss`——一次压缩的全价代价，
//! 紧跟一轮恢复正常的高命中，不该被误判成慢性失效。
//!
//! # 跟原验收的出入
//!
//! 103 原验收写的是「压缩后紧跟两轮**低**命中，不触发 `ChronicMiss`」。这跟
//! 024 已经落地、未改动的 `check_window`（`cache/window.rs`）矛盾：**只有失明轮**
//! （`TurnHit::Blind`，provider 压根没报 `cached`）既不进窗口也不打断连续性
//! （`window.rs:130` 模块文档 + `blind_turns_neither_count_nor_break_the_streak`
//! 单测）。压缩轮报的是 `Some(0)`，是 `TurnHit::Observed`，是货真价实的低命中
//! 轮，**本来就该计入连续计数**——压缩之后还连着两轮低命中，说明这次压缩没
//! 解决问题，那正是「慢性失效」该报的场景，`DEFAULT_CONSECUTIVE_ALERT = 3`
//! 已经给了「压缩本身的一次性代价」一轮的容忍空间。
//!
//! 主会话核实后判定原验收写错，改成本文件这条：**只有一轮压缩、紧跟一轮
//! 恢复正常的高命中**，才是「一次性代价不该被误判」这句话真正想说的东西。

use agent_core::{AgentId, Session};
use agent_runtime::{RunnerEvent, run_turn};

use crate::support::{self, ScriptedResponse};

/// `sse_text` 的参数化版本：命中/总量都由调用方给，好精确控制每一轮落在
/// 「低命中」（< 50%）还是「健康」（>= 50%）哪一边。
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

#[test]
fn a_single_compaction_round_does_not_trigger_chronic_miss() {
    let dir = support::temp_dir("prefix-intent-single-compaction-not-chronic");

    let port = support::spawn_scripted_server(vec![
        sse_text_with_hit("健康的一轮", 500, 400), // 80%，垫底，给压缩轮留东西可推
        sse_text_with_hit("压缩轮回复", 500, 0),   // 0%，压缩轮本身的全价代价
        sse_text_with_hit("恢复正常的一轮", 500, 400), // 80%，压缩之后立刻恢复健康
    ]);
    let (mut ctx, events) = support::build_ctx(port, &dir);
    let mut session = Session::new(AgentId::root());
    let root = AgentId::root();

    run_turn(&mut session, &mut ctx, "第一句话").expect("轮 0 不该是 source failure");

    let history_len = session.messages_of(&root).len();
    session.begin_turn();
    session
        .advance_boundary(&root, history_len, None)
        .expect("边界从 0 推到 history_len 该被接受");
    run_turn(&mut session, &mut ctx, "继续").expect("压缩轮不该是 source failure");

    session.begin_turn();
    run_turn(&mut session, &mut ctx, "压缩后恢复正常").expect("轮 B 不该是 source failure");

    let events = events.borrow();
    let last_guard = events
        .iter()
        .filter_map(|e| match e {
            RunnerEvent::TurnGuard { report, .. } => Some(report),
            _ => None,
        })
        .next_back()
        .unwrap_or_else(|| panic!("轮 B 该有一份 GuardReport：{events:#?}"));

    assert!(
        !matches!(
            last_guard.window,
            agent_core::cache::WindowVerdict::ChronicMiss { .. }
        ),
        "压缩轮自己那一次全价不该在紧跟一轮健康命中之后还被算成慢性失效。实际：{:?}\n{events:#?}",
        last_guard.window
    );
}

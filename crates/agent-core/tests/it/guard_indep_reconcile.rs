//! 独立测试：缓存兜底第 2 层，预测 vs 真实对账（issue 024）。
//!
//! 只经公开 API（`agent_core::cache::{reconcile, ReconcileVerdict, ReconcileParams}`）
//! 验证五态逐一 + 边界，以及 022 的两轮真实回放数字。

use agent_core::cache::{ReconcileParams, ReconcileVerdict, reconcile};

/// `None`：这家没报 `cached`，第 2 层本轮不工作——不管 `predicted` 是不是 0，
/// 都必须落 `Blind`，绝不能被 `unwrap_or(0)` 折算成「没命中」。
#[test]
fn cached_none_is_blind_regardless_of_prediction() {
    let params = ReconcileParams::default();

    // 022 真实冷启动那一轮：predicted=0，这家没报 cached。
    assert_eq!(
        reconcile(0, None, params),
        ReconcileVerdict::Blind { predicted: 0 }
    );

    // 有预测但这家依然没报：仍然是 Blind，不是「预测但没对上」。
    assert_eq!(
        reconcile(512, None, params),
        ReconcileVerdict::Blind { predicted: 512 }
    );
}

/// `Some(0)`：这家明确报了「真的没命中」，走的是对账/缺口路径，
/// 跟 `None` 必须是两条不同的分支。
#[test]
fn cached_some_zero_with_prediction_goes_through_shortfall_path() {
    let params = ReconcileParams::default();
    let verdict = reconcile(1000, Some(0), params);
    assert_eq!(
        verdict,
        ReconcileVerdict::Shortfall {
            predicted: 1000,
            actual: 0,
            gap: 1000
        }
    );
    assert_eq!(verdict.shortfall_percent(), Some(100));
}

/// `predicted == 0` 且 `cached == Some(n)`：无预测态，不判，但能读出实际命中数
/// （常见于「此前一次调用焐热了缓存」——022 的冷启动那一轮正是这样）。
#[test]
fn no_prediction_still_surfaces_actual_hit() {
    let params = ReconcileParams::default();
    assert_eq!(
        reconcile(0, Some(512), params),
        ReconcileVerdict::NoPrediction { actual: 512 }
    );
}

/// `predicted == 0` 且 `cached == None`：两个「没数据」的信号同时出现时，
/// 失明优先——没有真实数字，连「无预测」都谈不上。
#[test]
fn blind_takes_priority_over_no_prediction() {
    let params = ReconcileParams::default();
    assert_eq!(
        reconcile(0, None, params),
        ReconcileVerdict::Blind { predicted: 0 }
    );
}

/// 缺口恰好 30%：阈值是「超过」才告警，边界算一致。
#[test]
fn gap_at_exactly_threshold_is_a_match() {
    let params = ReconcileParams::default();
    // predicted=1000, actual=700 → gap=300 → 300/1000 = 30%，不超过阈值。
    assert_eq!(
        reconcile(1000, Some(700), params),
        ReconcileVerdict::Match {
            predicted: 1000,
            actual: 700
        }
    );
}

/// 缺口超过 30%：告警，且带缺口数字。
#[test]
fn gap_over_threshold_is_shortfall_with_gap_number() {
    let params = ReconcileParams::default();
    // predicted=1000, actual=690 → gap=310 → 31%，超过阈值。
    let verdict = reconcile(1000, Some(690), params);
    assert_eq!(
        verdict,
        ReconcileVerdict::Shortfall {
            predicted: 1000,
            actual: 690,
            gap: 310
        }
    );
    assert_eq!(verdict.shortfall_percent(), Some(31));

    let text = verdict.to_string();
    assert!(
        text.contains("310"),
        "Shortfall 的措辞里应带缺口数字，实际: {text:?}"
    );
}

/// 好于预期（actual > predicted）：022 教训——不该出现「告警」「对不上」，
/// 否则读起来像出了事，实际是省了钱。
#[test]
fn better_than_expected_does_not_read_like_an_alert() {
    let params = ReconcileParams::default();
    let verdict = reconcile(500, Some(600), params);
    assert_eq!(
        verdict,
        ReconcileVerdict::BetterThanExpected {
            predicted: 500,
            actual: 600,
            surplus: 100
        }
    );

    let text = verdict.to_string();
    assert!(
        !text.contains("告警"),
        "好于预期的措辞里不该出现「告警」，实际: {text:?}"
    );
    assert!(
        !text.contains("对不上"),
        "好于预期的措辞里不该出现「对不上」，实际: {text:?}"
    );
}

/// 022 真实两轮回放：冷启动 (predicted=0, actual=512) 与稳定轮
/// (predicted=512, actual=512) 都不应产生告警（Shortfall）。
#[test]
fn issue_022_two_round_replay_produces_no_alert() {
    let params = ReconcileParams::default();

    let round1 = reconcile(0, Some(512), params);
    assert!(
        !matches!(round1, ReconcileVerdict::Shortfall { .. }),
        "第 1 轮冷启动不该告警，实际: {round1:?}"
    );
    assert_eq!(round1, ReconcileVerdict::NoPrediction { actual: 512 });

    let round2 = reconcile(512, Some(512), params);
    assert!(
        !matches!(round2, ReconcileVerdict::Shortfall { .. }),
        "第 2 轮预测与实际一致不该告警，实际: {round2:?}"
    );
    assert_eq!(
        round2,
        ReconcileVerdict::Match {
            predicted: 512,
            actual: 512
        }
    );
}

/// 纯函数：同一组输入调用两次必须完全一样（红线 1）。
#[test]
fn reconcile_is_pure_same_input_same_output() {
    let params = ReconcileParams::default();
    let a = reconcile(1000, Some(690), params);
    let b = reconcile(1000, Some(690), params);
    assert_eq!(a, b);
}

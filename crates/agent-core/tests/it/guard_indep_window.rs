//! 独立测试：缓存兜底第 3 层，滚动窗口命中率（issue 024）。
//!
//! 只经公开 API（`agent_core::cache::{check_window, TurnHit, WindowParams,
//! WindowVerdict}`）验证：十轮全命中不告警、连续三轮 0 命中告警、失明轮不进
//! 窗口也不打断连续性、cached>prompt 的畸形输入不 panic。

use agent_core::cache::{check_window, TurnHit, WindowParams, WindowVerdict};

#[test]
fn ten_full_hits_do_not_alert() {
    let history = vec![
        TurnHit::Observed {
            prompt: 1000,
            cached: 1000
        };
        10
    ];
    let verdict = check_window(&history, WindowParams::default());
    match verdict {
        WindowVerdict::Healthy {
            turns,
            hit_percent,
            low_streak
        } => {
            assert_eq!(turns, 10);
            assert_eq!(hit_percent, 100);
            assert_eq!(low_streak, 0);
        }
        other => panic!("十轮全命中应为 Healthy，实际: {other:?}"),
    }
    assert!(!matches!(verdict, WindowVerdict::ChronicMiss { .. }));
}

#[test]
fn three_consecutive_zero_hits_alert() {
    let history = vec![
        TurnHit::Observed {
            prompt: 1000,
            cached: 0
        };
        3
    ];
    let verdict = check_window(&history, WindowParams::default());
    assert_eq!(
        verdict,
        WindowVerdict::ChronicMiss {
            streak: 3,
            turns: 3,
            hit_percent: 0
        }
    );
}

/// 两轮 0 命中 + 一轮失明(None) + 一轮 0 命中：失明轮不打断连续性——
/// 实做记录明说：折算成 0 命中的话，不报 cached 的那家会被判成天天缓存全崩，
/// 所以失明轮对这一层「不存在」，连续计数照样是 3。
#[test]
fn blind_turn_does_not_break_the_streak() {
    let history = vec![
        TurnHit::Observed {
            prompt: 1000,
            cached: 0
        },
        TurnHit::Observed {
            prompt: 1000,
            cached: 0
        },
        TurnHit::Blind,
        TurnHit::Observed {
            prompt: 1000,
            cached: 0
        },
    ];
    let verdict = check_window(&history, WindowParams::default());
    assert_eq!(
        verdict,
        WindowVerdict::ChronicMiss {
            streak: 3,
            turns: 3,
            hit_percent: 0
        },
        "失明轮不该打断连续 0 命中的计数"
    );
}

/// 失明轮不进窗口统计：窗口大小按「有观测的轮次」计，失明轮不占位。
/// 用一个远小于默认窗口的自定义窗口（2）来放大这个效果：中间穿插的失明轮
/// 如果占了位，2 个观测轮次会被挤出窗口之外；实际不该发生。
#[test]
fn blind_turns_do_not_occupy_window_slots() {
    let params = WindowParams {
        window: 2,
        low_hit_percent: 50,
        consecutive_alert: 3
    };
    let history = vec![
        TurnHit::Blind,
        TurnHit::Observed {
            prompt: 1000,
            cached: 1000
        },
        TurnHit::Blind,
        TurnHit::Observed {
            prompt: 1000,
            cached: 900
        },
        TurnHit::Blind,
    ];
    let verdict = check_window(&history, params);
    match verdict {
        WindowVerdict::Healthy { turns, .. } => {
            assert_eq!(turns, 2, "窗口大小按有观测的轮次计，失明轮不占位")
        }
        other => panic!("期望 Healthy 且 turns=2，实际: {other:?}"),
    }
}

/// 全失明：第 3 层不工作，跟「命中率 0%」是两件事。
#[test]
fn all_blind_history_is_no_data_not_zero_hit_rate() {
    let history = vec![TurnHit::Blind; 3];
    let verdict = check_window(&history, WindowParams::default());
    assert_eq!(verdict, WindowVerdict::NoData { skipped: 3 });
}

/// cached > prompt 的畸形输入（上游解析问题）不能让这一层 panic，
/// 命中率按 prompt 封顶而不是报出 300% 这种没法读的数。
#[test]
fn malformed_cached_greater_than_prompt_does_not_panic() {
    let malformed = TurnHit::Observed {
        prompt: 10,
        cached: 20
    };
    assert_eq!(malformed.hit_percent(), Some(100));

    let history = vec![malformed; 5];
    // 这里只要不 panic 就是通过；顺带确认判读能正常给出结果。
    let verdict = check_window(&history, WindowParams::default());
    match verdict {
        WindowVerdict::Healthy { hit_percent, .. } => assert_eq!(hit_percent, 100),
        other => panic!("畸形输入不该被判成低命中，实际: {other:?}"),
    }
}

/// 纯函数：同一份历史重放两次必须得出同一个告警（红线 1，按轮计数不按时间）。
#[test]
fn check_window_is_pure_same_input_same_output() {
    let history = vec![
        TurnHit::Observed {
            prompt: 1000,
            cached: 0
        };
        3
    ];
    let a = check_window(&history, WindowParams::default());
    let b = check_window(&history, WindowParams::default());
    assert_eq!(a, b);
}

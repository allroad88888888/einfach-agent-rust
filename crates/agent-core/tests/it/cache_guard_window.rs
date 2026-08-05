//! issue 024 验收 · 兜底**第 3 层**：滚动窗口的命中率。
//!
//! 前两层漏掉的慢性失效只有攒够几轮才看得出来。这里最要紧的一条是
//! **失明轮不进窗口**——把「这家没报」算成 0 命中，这一层就会对着那家天天告警，
//! 然后没人再看它。

use agent_core::TokenUsage;
use agent_core::cache::{TurnHit, WindowParams, WindowVerdict, check_window};

fn hit(prompt: u32, cached: u32) -> TurnHit {
    TurnHit::Observed { prompt, cached }
}

/// 验收：十轮全命中不告警。
#[test]
fn ten_warm_turns_do_not_alert() {
    let history = vec![hit(5000, 4800); 10];
    let v = check_window(&history, WindowParams::default());
    assert_eq!(
        v,
        WindowVerdict::Healthy {
            turns: 10,
            hit_percent: 96,
            low_streak: 0
        }
    );
    assert!(!v.to_string().contains("告警"), "{v}");
}

/// 验收：连续三轮 0 命中告警。
#[test]
fn three_consecutive_zero_hit_turns_alert() {
    let history = vec![hit(5000, 0); 3];
    let v = check_window(&history, WindowParams::default());
    assert_eq!(
        v,
        WindowVerdict::ChronicMiss {
            streak: 3,
            turns: 3,
            hit_percent: 0
        }
    );
    assert!(v.to_string().contains("告警"), "{v}");
}

/// 两轮低命中还不到告警线——单轮低命中是正常现象（换前缀、压缩、第一次见这个
/// 变体），K 轮才说明前缀根本没在复用。
#[test]
fn two_low_turns_are_not_yet_chronic() {
    let history = vec![hit(5000, 4800), hit(5000, 0), hit(5000, 0)];
    let v = check_window(&history, WindowParams::default());
    assert_eq!(
        v,
        WindowVerdict::Healthy {
            turns: 3,
            hit_percent: 32,
            low_streak: 2
        }
    );
    // 还没告警，但要说得出「已经连着两轮了」。
    assert!(v.to_string().contains('2'), "{v}");
}

/// 验收：**失明轮不进窗口**——既不算好也不算坏，只能不算。
/// 三段分别管：不占窗口位、不被当成 0 命中、不打断连续性。
#[test]
fn blind_turns_never_enter_the_window() {
    // 1. 全失明 → 本层不工作，不是「命中率 0%」。
    let all_blind = vec![TurnHit::Blind; 5];
    let v = check_window(&all_blind, WindowParams::default());
    assert_eq!(v, WindowVerdict::NoData { skipped: 5 });
    assert!(v.to_string().contains("不工作"), "{v}");

    // 2. 十轮好 + 三轮失明 → 还是那十轮，命中率不被稀释。
    let mut mixed = vec![hit(5000, 4800); 10];
    mixed.extend(vec![TurnHit::Blind; 3]);
    assert_eq!(
        check_window(&mixed, WindowParams::default()),
        WindowVerdict::Healthy {
            turns: 10,
            hit_percent: 96,
            low_streak: 0
        }
    );

    // 3. 失明轮夹在中间，不打断「连续三轮低命中」：它对这一层是不存在的。
    let interleaved = vec![
        hit(5000, 0),
        TurnHit::Blind,
        hit(5000, 0),
        TurnHit::Blind,
        hit(5000, 0),
    ];
    assert_eq!(
        check_window(&interleaved, WindowParams::default()),
        WindowVerdict::ChronicMiss {
            streak: 3,
            turns: 3,
            hit_percent: 0
        }
    );
}

/// 窗口只看最近 N 轮：更早的坏轮次要淡出，否则一次事故会永久拉低报出来的命中率。
#[test]
fn window_only_looks_at_the_last_n_turns() {
    let mut history = vec![hit(5000, 0); 3]; // 三轮 0 命中，早就过去了
    history.extend(vec![hit(5000, 5000); 10]); // 之后十轮全命中
    assert_eq!(
        check_window(&history, WindowParams::default()),
        WindowVerdict::Healthy {
            turns: 10,
            hit_percent: 100,
            low_streak: 0
        }
    );
}

/// 空历史、`prompt == 0` 的退化轮：不判，也不许 panic 或除零。
#[test]
fn degenerate_inputs_do_not_panic() {
    assert_eq!(
        check_window(&[], WindowParams::default()),
        WindowVerdict::NoData { skipped: 0 }
    );
    assert_eq!(
        check_window(&[hit(0, 0)], WindowParams::default()),
        WindowVerdict::NoData { skipped: 1 }
    );
    assert_eq!(hit(0, 0).hit_percent(), None);
    // cached 大于 prompt（上游解析出了问题）时封顶 100%，不报出没法读的数字。
    assert_eq!(hit(100, 999).hit_percent(), Some(100));
}

/// 窗口大小、低命中门槛、连续轮数都是参数，默认值不是硬编码在判读里。
#[test]
fn window_thresholds_are_parameters() {
    let history = vec![hit(5000, 4800), hit(5000, 0), hit(5000, 0)];
    let k2 = WindowParams {
        consecutive_alert: 2,
        ..WindowParams::default()
    };
    assert_eq!(
        check_window(&history, k2),
        WindowVerdict::ChronicMiss {
            streak: 2,
            turns: 3,
            hit_percent: 32
        }
    );

    // 低命中门槛提到 97%：96% 的十轮全变成「低命中」。
    let strict = WindowParams {
        low_hit_percent: 97,
        ..WindowParams::default()
    };
    assert_eq!(
        check_window(&[hit(5000, 4800); 10], strict),
        WindowVerdict::ChronicMiss {
            streak: 10,
            turns: 10,
            hit_percent: 96
        }
    );

    // 窗口缩到 2：只看最近两轮，更早的那轮好成绩进不来。
    let small = WindowParams {
        window: 2,
        ..WindowParams::default()
    };
    assert_eq!(
        check_window(&history, small),
        WindowVerdict::Healthy {
            turns: 2,
            hit_percent: 0,
            low_streak: 2
        }
    );
}

/// usage 到观测的映射：`None` → 失明，`Some(0)` → 真的没命中，两条路。
#[test]
fn turn_hit_from_usage_splits_none_and_zero() {
    let none = TokenUsage {
        prompt: 500,
        completion: 10,
        cached: None,
    };
    let zero = TokenUsage {
        prompt: 500,
        completion: 10,
        cached: Some(0),
    };
    assert_eq!(TurnHit::from_usage(&none), TurnHit::Blind);
    assert_eq!(TurnHit::from_usage(&none).hit_percent(), None);
    assert_eq!(
        TurnHit::from_usage(&zero),
        TurnHit::Observed {
            prompt: 500,
            cached: 0
        }
    );
    assert_eq!(TurnHit::from_usage(&zero).hit_percent(), Some(0));
    assert_ne!(TurnHit::from_usage(&none), TurnHit::from_usage(&zero));
}

/// 窗口按**轮次**计数不按时间：同一份历史重放两次结果必须一模一样（红线 1）。
#[test]
fn layer3_is_pure() {
    let history = vec![
        hit(5000, 4800),
        TurnHit::Blind,
        hit(5000, 0),
        hit(0, 0),
        hit(300, 0),
    ];
    let first = check_window(&history, WindowParams::default());
    for _ in 0..1_000 {
        assert_eq!(check_window(&history, WindowParams::default()), first);
    }
}

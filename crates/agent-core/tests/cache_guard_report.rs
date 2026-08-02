//! issue 024 验收 · **三层共存且各自可分辨**。
//!
//! 验收原文：「三层的告警各自可分辨，不是同一个布尔」。分辨发生在**类型上**——
//! 下面的 `match` 穷举三个变体，多一层少一层都编译不过，不靠在字符串里找关键字。

use agent_core::cache::{
    DriftVerdict, GuardAlert, GuardLayer, GuardReport, PrefixIntent, ReconcileParams,
    ReconcileVerdict, TurnHit, WindowParams, WindowVerdict, check_drift, check_window, reconcile,
};
use agent_core::{Segment, TokenUsage};

#[test]
fn three_layers_are_distinguishable_by_type() {
    let report = GuardReport {
        drift: DriftVerdict::Unexpected { segment: Segment::Tools },
        reconcile: ReconcileVerdict::Shortfall { predicted: 5120, actual: 0, gap: 5120 },
        window: WindowVerdict::ChronicMiss { streak: 3, turns: 10, hit_percent: 4 },
    };

    let alerts = report.alerts();
    assert_eq!(alerts.len(), 3);

    let mut seen = Vec::new();
    for alert in &alerts {
        match alert {
            GuardAlert::UnexpectedDrift { segment } => {
                assert_eq!(*segment, Segment::Tools);
                seen.push(1);
            }
            GuardAlert::CacheShortfall { gap, .. } => {
                assert_eq!(*gap, 5120);
                seen.push(2);
            }
            GuardAlert::ChronicMiss { streak, .. } => {
                assert_eq!(*streak, 3);
                seen.push(3);
            }
        }
    }
    assert_eq!(seen, vec![1, 2, 3]);
    assert_eq!(
        alerts.iter().map(GuardAlert::layer).collect::<Vec<_>>(),
        vec![GuardLayer::PreFlight, GuardLayer::Reconcile, GuardLayer::Window]
    );
    assert_eq!(alerts.iter().map(|a| a.layer().number()).collect::<Vec<_>>(), vec![1, 2, 3]);
}

/// 一层告警不牵连另外两层：只有第 2 层出事时，报告里只有第 2 层那一条。
#[test]
fn one_layer_alerting_does_not_drag_the_others() {
    let report = GuardReport {
        drift: DriftVerdict::Clean,
        reconcile: ReconcileVerdict::Shortfall { predicted: 5120, actual: 0, gap: 5120 },
        window: WindowVerdict::Healthy { turns: 10, hit_percent: 96, low_streak: 0 },
    };
    assert!(report.has_alert());
    assert_eq!(report.alerts().len(), 1);
    assert_eq!(report.alerts()[0].layer(), GuardLayer::Reconcile);
}

/// **失明不是告警**：这家不报 cached、窗口还没数据，`alerts()` 是空的——
/// 但那不等于「都正常」，所以 Display 必须把两层的失明说出来。
#[test]
fn blind_layers_are_reported_but_not_alerted() {
    let report = GuardReport {
        drift: DriftVerdict::Clean,
        reconcile: ReconcileVerdict::Blind { predicted: 512 },
        window: WindowVerdict::NoData { skipped: 3 },
    };
    assert!(!report.has_alert());
    assert!(report.alerts().is_empty());

    let text = report.to_string();
    assert_eq!(text.lines().count(), 3, "{text}");
    assert_eq!(text.matches("不工作").count(), 2, "两层失明各说一次：{text}");
}

/// 三层各自的判读同时存在于一份报告里，Display 三行都打——只打告警那层，
/// 人就不知道另外两层是好是失明。
#[test]
fn report_prints_all_three_layers() {
    let report = GuardReport {
        drift: DriftVerdict::Clean,
        reconcile: ReconcileVerdict::Match { predicted: 512, actual: 512 },
        window: WindowVerdict::Healthy { turns: 4, hit_percent: 91, low_streak: 0 },
    };
    let text = report.to_string();
    assert_eq!(text.lines().count(), 3, "{text}");
    assert!(text.contains("发前比对"), "{text}");
    assert!(text.contains("对账"), "{text}");
    assert!(text.contains("滚动窗口"), "{text}");
}

/// 判读结果要能进日志与快照（红线 3 的精神）：整份报告 serde 往返不掉字段。
#[test]
fn report_roundtrips_through_serde() {
    let report = GuardReport {
        drift: DriftVerdict::Expected { segment: Segment::History },
        reconcile: ReconcileVerdict::BetterThanExpected {
            predicted: 512,
            actual: 5120,
            surplus: 4608,
        },
        window: WindowVerdict::ChronicMiss { streak: 4, turns: 10, hit_percent: 12 },
    };
    let s = serde_json::to_string(&report).unwrap();
    assert_eq!(serde_json::from_str::<GuardReport>(&s).unwrap(), report);
}

/// 宿主一轮下来的完整走法：encode 后先比对（钱还没花），响应回来再对账 + 进窗口。
/// 字节级的 drift 计算是 adapter 的活，验收在 `cache_guard_preflight.rs`。
#[test]
fn one_turn_end_to_end() {
    // 1. 发出去之前。本轮只是追加消息，没打算改前缀，adapter 却报 History 漂了。
    let drift = check_drift(Some(Segment::History), PrefixIntent::Reuse);

    // 2. 响应回来。
    let usage = TokenUsage { prompt: 2432, completion: 88, cached: Some(2048) };
    let reconciled = reconcile(2048, usage.cached, ReconcileParams::default());
    let history = vec![TurnHit::Observed { prompt: 2000, cached: 1900 }, TurnHit::from_usage(&usage)];
    let window = check_window(&history, WindowParams::default());

    let report = GuardReport { drift, reconcile: reconciled, window };
    assert_eq!(report.alerts(), vec![GuardAlert::UnexpectedDrift { segment: Segment::History }]);
    assert_eq!(report.alerts()[0].layer().number(), 1, "钱还没花的那一层");
}

/// 真实两轮（022 实做记录）在这三层里走一遍，一条告警都不该有。
/// 第 1 轮 predicted=0 / actual=512（冷启动，此前一次同前缀调用焐热了缓存），
/// 第 2 轮 predicted=512 / actual=512（596 按块向下取整）。
#[test]
fn recorded_real_two_turns_are_quiet() {
    let p = ReconcileParams::default();

    let turn1 = TokenUsage { prompt: 587, completion: 44, cached: Some(512) };
    let r1 = GuardReport {
        drift: check_drift(None, PrefixIntent::Reuse),
        reconcile: reconcile(0, turn1.cached, p),
        window: check_window(&[TurnHit::from_usage(&turn1)], WindowParams::default()),
    };
    assert!(r1.alerts().is_empty(), "{r1}");
    assert_eq!(r1.reconcile, ReconcileVerdict::NoPrediction { actual: 512 });

    let turn2 = TokenUsage { prompt: 596, completion: 40, cached: Some(512) };
    let r2 = GuardReport {
        drift: check_drift(None, PrefixIntent::Reuse),
        reconcile: reconcile(512, turn2.cached, p),
        window: check_window(
            &[TurnHit::from_usage(&turn1), TurnHit::from_usage(&turn2)],
            WindowParams::default(),
        ),
    };
    assert!(r2.alerts().is_empty(), "{r2}");
    assert_eq!(r2.reconcile, ReconcileVerdict::Match { predicted: 512, actual: 512 });
}

//! 独立测试：三层聚合与 `GuardReport` / `GuardAlert`（issue 024）。
//!
//! 覆盖验收：三层告警在类型上可分辨（`match` 穷举编译器保证）、
//! 失明/无预测不出现在 `alerts()` 里、`GuardReport` serde 往返。

use agent_core::cache::{
    DriftVerdict, GuardAlert, GuardLayer, GuardReport, ReconcileVerdict, WindowVerdict
};
use agent_core::Segment;

/// 三层告警在类型上可分辨：这个 match **没有通配符** `_`，如果 `GuardAlert`
/// 将来多一个变体、或者少一个，这里编译不过——编译器就是那道保证。
#[test]
fn guard_alert_variants_are_exhaustively_matchable_by_layer() {
    let alerts = vec![
        GuardAlert::UnexpectedDrift {
            segment: Segment::Tools
        },
        GuardAlert::CacheShortfall {
            predicted: 1000,
            actual: 500,
            gap: 500
        },
        GuardAlert::ChronicMiss {
            streak: 3,
            turns: 10,
            hit_percent: 10
        }
    ];

    for alert in &alerts {
        let layer = match alert {
            GuardAlert::UnexpectedDrift { .. } => GuardLayer::PreFlight,
            GuardAlert::CacheShortfall { .. } => GuardLayer::Reconcile,
            GuardAlert::ChronicMiss { .. } => GuardLayer::Window
        };
        assert_eq!(layer, alert.layer(), "match 穷举得出的层号应和 alert.layer() 一致");
    }

    assert_eq!(alerts[0].layer().number(), 1);
    assert_eq!(alerts[1].layer().number(), 2);
    assert_eq!(alerts[2].layer().number(), 3);
}

/// 失明 / 无预测 / 窗口无数据都不是异常，不该混进 `alerts()`——
/// 混进去会让人去修一个不存在的 bug。
#[test]
fn blind_and_no_data_states_do_not_appear_in_alerts() {
    let report = GuardReport {
        drift: DriftVerdict::Clean,
        reconcile: ReconcileVerdict::Blind { predicted: 512 },
        window: WindowVerdict::NoData { skipped: 3 }
    };
    assert!(report.alerts().is_empty(), "失明/无数据不该产生告警");
    assert!(!report.has_alert());

    let report2 = GuardReport {
        drift: DriftVerdict::Expected {
            segment: Segment::System
        },
        reconcile: ReconcileVerdict::NoPrediction { actual: 512 },
        window: WindowVerdict::Healthy {
            turns: 4,
            hit_percent: 92,
            low_streak: 0
        }
    };
    assert!(report2.alerts().is_empty(), "无预测/健康窗口不该产生告警");
    assert!(!report2.has_alert());
}

/// 三层同时告警时，`alerts()` 要把三条都报出来，且各自可辨认。
#[test]
fn all_three_layers_alerting_produces_three_distinct_alerts() {
    let report = GuardReport {
        drift: DriftVerdict::Unexpected {
            segment: Segment::History
        },
        reconcile: ReconcileVerdict::Shortfall {
            predicted: 1000,
            actual: 500,
            gap: 500
        },
        window: WindowVerdict::ChronicMiss {
            streak: 3,
            turns: 10,
            hit_percent: 20
        }
    };
    assert!(report.has_alert());
    let alerts = report.alerts();
    assert_eq!(alerts.len(), 3);
    assert_eq!(alerts[0].layer(), GuardLayer::PreFlight);
    assert_eq!(alerts[1].layer(), GuardLayer::Reconcile);
    assert_eq!(alerts[2].layer(), GuardLayer::Window);
}

/// `GuardReport` 必须能 serde 往返——它是要跨进程/跨轮传的判读结果。
#[test]
fn guard_report_serde_roundtrip() {
    let report = GuardReport {
        drift: DriftVerdict::Unexpected {
            segment: Segment::Tools
        },
        reconcile: ReconcileVerdict::Shortfall {
            predicted: 1000,
            actual: 500,
            gap: 500
        },
        window: WindowVerdict::ChronicMiss {
            streak: 3,
            turns: 10,
            hit_percent: 20
        }
    };

    let json = serde_json::to_string(&report).expect("GuardReport 必须能序列化");
    let restored: GuardReport =
        serde_json::from_str(&json).expect("序列化产物必须能反序列化回同一个类型");
    assert_eq!(report, restored, "往返之后必须逐字段相等");
}

/// 静默态（失明/无预测/无数据）同样要能 serde 往返——不只是告警态。
#[test]
fn guard_report_serde_roundtrip_for_silent_states() {
    let report = GuardReport {
        drift: DriftVerdict::Clean,
        reconcile: ReconcileVerdict::Blind { predicted: 0 },
        window: WindowVerdict::NoData { skipped: 5 }
    };
    let json = serde_json::to_string(&report).expect("GuardReport 必须能序列化");
    let restored: GuardReport =
        serde_json::from_str(&json).expect("序列化产物必须能反序列化回同一个类型");
    assert_eq!(report, restored);
}

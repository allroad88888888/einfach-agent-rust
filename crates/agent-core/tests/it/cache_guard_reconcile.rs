//! issue 024 验收 · 兜底**第 2 层**：预测 vs 真实的对账。
//!
//! 五种情况各有各的断言。**折算是这一层唯一的死法**：把 `None` 当 0、把「好于
//! 预期」当「对不上」、把冷启动当缺口，都会让这一层要么天天假告警、要么永远沉默。

use agent_core::cache::{DriftVerdict, WindowVerdict};
use agent_core::cache::{GuardReport, ReconcileParams, ReconcileVerdict, reconcile};

/// 验收：命中数对得上时不告警。真实第 2 轮的数字（预测 512 / 实际 512）。
#[test]
fn matching_prediction_is_silent() {
    let v = reconcile(512, Some(512), ReconcileParams::default());
    assert_eq!(
        v,
        ReconcileVerdict::Match {
            predicted: 512,
            actual: 512
        }
    );
    assert!(v.shortfall_percent().is_none());
    assert!(v.to_string().contains("一致"), "{v}");
    assert!(!v.to_string().contains("告警"), "{v}");
}

/// 验收：差 30% 以上时告警，带缺口数字。
#[test]
fn shortfall_beyond_threshold_alerts_with_numbers() {
    let v = reconcile(5120, Some(1024), ReconcileParams::default());
    assert_eq!(
        v,
        ReconcileVerdict::Shortfall {
            predicted: 5120,
            actual: 1024,
            gap: 4096
        }
    );
    assert_eq!(v.shortfall_percent(), Some(80));
    assert!(v.to_string().contains("4096"), "{v}");
}

/// 30% 边界两侧：恰好 30% 不告警，多缺一个 token 就告警。
/// 边界必须有确定的一边，否则同一个数字两次读代码会得出不同结论。
#[test]
fn thirty_percent_boundary_has_a_definite_side() {
    let p = ReconcileParams::default();
    assert_eq!(
        reconcile(1000, Some(700), p),
        ReconcileVerdict::Match {
            predicted: 1000,
            actual: 700
        }
    );
    assert_eq!(
        reconcile(1000, Some(699), p),
        ReconcileVerdict::Shortfall {
            predicted: 1000,
            actual: 699,
            gap: 301
        }
    );
}

/// **好于预期不该吓人**（022 的措辞教训）：实际比预测多是信息级，不是告警，
/// 措辞里不许出现「告警」「对不上」。
#[test]
fn better_than_expected_is_information_not_alarm() {
    let v = reconcile(512, Some(5120), ReconcileParams::default());
    assert_eq!(
        v,
        ReconcileVerdict::BetterThanExpected {
            predicted: 512,
            actual: 5120,
            surplus: 4608
        }
    );

    let report = GuardReport {
        drift: DriftVerdict::Clean,
        reconcile: v,
        window: WindowVerdict::NoData { skipped: 0 },
    };
    assert!(report.alerts().is_empty(), "好于预期不是告警");

    let text = v.to_string();
    assert!(!text.contains("告警"), "{text}");
    assert!(!text.contains("对不上"), "{text}");
    assert!(text.contains("好于预期"), "{text}");
}

/// 验收：**字段缺失 vs 字段为 0，两种输入走不同分支，有各自的断言。**
/// 折算成 0 的话，不报 cached 的那家会被判成「缓存全崩」，而且天天崩。
#[test]
fn missing_field_and_explicit_zero_take_different_branches() {
    let p = ReconcileParams::default();

    // 字段整个缺失 → 失明：本轮第 2 层不工作，明确说出来。
    let blind = reconcile(512, None, p);
    assert_eq!(blind, ReconcileVerdict::Blind { predicted: 512 });
    assert!(blind.to_string().contains("不工作"), "{blind}");
    assert!(blind.shortfall_percent().is_none());

    // 字段存在且为 0 → 真的没命中，缺口 100%，告警。
    let zero = reconcile(512, Some(0), p);
    assert_eq!(
        zero,
        ReconcileVerdict::Shortfall {
            predicted: 512,
            actual: 0,
            gap: 512
        }
    );
    assert_eq!(zero.shortfall_percent(), Some(100));

    assert_ne!(blind, zero, "None 与 Some(0) 必须落在不同变体上");
}

/// `predicted == 0`（冷启动 / 上轮镜像缺失）不判，且要能带出实际命中数——
/// 真实第 1 轮就是 predicted=0 / actual=512：此前一次同前缀调用把缓存焐热了，
/// 「没预测却命中了」不是异常，是预测这一侧信息不全。
#[test]
fn zero_prediction_is_not_judged() {
    let p = ReconcileParams::default();
    let v = reconcile(0, Some(512), p);
    assert_eq!(v, ReconcileVerdict::NoPrediction { actual: 512 });
    assert!(v.to_string().contains("不判"), "{v}");
    assert_eq!(
        reconcile(0, Some(0), p),
        ReconcileVerdict::NoPrediction { actual: 0 }
    );

    // 无预测 + 这家不报 → 先说失明：没有真实数字，谈不上对账。
    assert_eq!(
        reconcile(0, None, p),
        ReconcileVerdict::Blind { predicted: 0 }
    );
}

/// 容差与阈值都是参数，默认值不是硬编码在判读里。
#[test]
fn tolerance_and_threshold_are_parameters() {
    // 默认容差 0：预测 100 实际 101 就是「好于预期」。
    assert!(matches!(
        reconcile(100, Some(101), ReconcileParams::default()),
        ReconcileVerdict::BetterThanExpected { surplus: 1, .. }
    ));

    // 给 64 的容差：零头被吃掉，两侧都算一致。
    let loose = ReconcileParams {
        tolerance_tokens: 64,
        ..ReconcileParams::default()
    };
    assert!(matches!(
        reconcile(100, Some(101), loose),
        ReconcileVerdict::Match { .. }
    ));
    assert!(matches!(
        reconcile(100, Some(50), loose),
        ReconcileVerdict::Match { .. }
    ));

    // 阈值收紧到 10%：同一份数字就该告警了。
    let strict = ReconcileParams {
        shortfall_alert_percent: 10,
        ..ReconcileParams::default()
    };
    assert!(matches!(
        reconcile(1000, Some(880), strict),
        ReconcileVerdict::Shortfall { gap: 120, .. }
    ));
}

/// 判读不带累积状态，也不依赖调用顺序（红线 1）。
#[test]
fn layer2_is_pure() {
    let p = ReconcileParams::default();
    let cases = [
        (0u32, None),
        (0, Some(1)),
        (512, Some(512)),
        (512, None),
        (1000, Some(1)),
    ];
    for (predicted, cached) in cases {
        let first = reconcile(predicted, cached, p);
        for _ in 0..1_000 {
            assert_eq!(reconcile(predicted, cached, p), first);
        }
    }
}

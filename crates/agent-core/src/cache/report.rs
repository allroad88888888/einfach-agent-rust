//! 三层的判读结果装在一起，以及**类型上可分辨**的告警（issue 024）。
//!
//! 「三层的告警各自可分辨，不是同一个布尔」是验收原文。三层混成一个信号，
//! 报警的时候就分不出该查我们自己的序列化、查 adapter 的预测、还是查这条会话本身
//! ——而这三件事的处理方式完全不同。

use std::fmt;

use serde::{Deserialize, Serialize};

use crate::seam::Segment;

use super::drift::DriftVerdict;
use super::reconcile::ReconcileVerdict;
use super::window::WindowVerdict;

/// 哪一层报的。
#[derive(Clone, Copy, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub enum GuardLayer {
    /// 第 1 层，发请求之前，零花费。
    PreFlight,
    /// 第 2 层，响应回来对账，花一轮的钱。
    Reconcile,
    /// 第 3 层，滚动窗口，花若干轮的钱。
    Window,
}

impl GuardLayer {
    /// 层号，1 / 2 / 3。
    pub fn number(&self) -> u8 {
        match self {
            GuardLayer::PreFlight => 1,
            GuardLayer::Reconcile => 2,
            GuardLayer::Window => 3,
        }
    }
}

impl fmt::Display for GuardLayer {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let name = match self {
            GuardLayer::PreFlight => "发前比对",
            GuardLayer::Reconcile => "对账",
            GuardLayer::Window => "滚动窗口",
        };
        write!(f, "第 {} 层 {name}", self.number())
    }
}

/// 一条告警。**三个变体来自三层，各自带各自的数字**——不是同一个布尔，
/// 消费者可以只对某一层做动作（比如第 1 层直接拦下这次请求）。
///
/// 注意这里**没有**「这家没报 cached」「本轮无预测」「窗口没数据」那几种情况：
/// 它们是**失明**不是异常，混进告警会让人去修一个不存在的 bug。
/// 要展示失明状态，看 [`GuardReport`] 里对应那一层的 verdict。
#[derive(Clone, Copy, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub enum GuardAlert {
    /// 第 1 层：没打算改前缀，某一段却漂了。**我们自己的 bug，且钱还没花。**
    UnexpectedDrift { segment: Segment },
    /// 第 2 层：实际命中比预测少太多。对这家缓存语义的理解可能是错的。
    CacheShortfall { predicted: u32, actual: u32, gap: u32 },
    /// 第 3 层：连续多轮低命中。慢性失效，或者这条会话正在做没料到的事。
    ChronicMiss { streak: usize, turns: usize, hit_percent: u32 },
}

impl GuardAlert {
    /// 这条告警是哪一层报的。
    pub fn layer(&self) -> GuardLayer {
        match self {
            GuardAlert::UnexpectedDrift { .. } => GuardLayer::PreFlight,
            GuardAlert::CacheShortfall { .. } => GuardLayer::Reconcile,
            GuardAlert::ChronicMiss { .. } => GuardLayer::Window,
        }
    }
}

impl fmt::Display for GuardAlert {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "[{}] ", self.layer())?;
        match *self {
            GuardAlert::UnexpectedDrift { segment } => {
                write!(f, "{segment:?} 段漂了，本轮并没打算改前缀")
            }
            GuardAlert::CacheShortfall { predicted, actual, gap } => {
                write!(f, "预测命中 {predicted}，实际 {actual}，缺口 {gap}")
            }
            GuardAlert::ChronicMiss { streak, turns, hit_percent } => {
                write!(f, "连续 {streak} 轮低命中，最近 {turns} 轮整体命中率 {hit_percent}%")
            }
        }
    }
}

/// 一轮的三层判读结果。三个字段是**三个不同的类型**，共存、互不覆盖。
///
/// 装配时机见 [`crate::cache`] 的模块文档：`drift` 在请求发出**之前**就有了，
/// 另外两个要等响应。这里只是把它们放在一起给人看，不代表它们同时产生。
///
/// 032：`SessionEvent::TurnGuard.report` 的类型，`ts` feature 门后面导出 TS。
#[derive(Clone, Copy, PartialEq, Eq, Debug, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct GuardReport {
    /// 第 1 层，[`super::check_drift`] 的产出。
    pub drift: DriftVerdict,
    /// 第 2 层，[`super::reconcile()`] 的产出。
    pub reconcile: ReconcileVerdict,
    /// 第 3 层，[`super::check_window`] 的产出。
    pub window: WindowVerdict,
}

impl GuardReport {
    /// 本轮的告警，按层号排。没有告警就是空的——**空不等于「都正常」**，
    /// 某一层可能只是失明（这家没报 cached、窗口还没攒够）。
    ///
    /// ```
    /// use agent_core::cache::{
    ///     DriftVerdict, GuardAlert, GuardLayer, GuardReport, ReconcileVerdict, WindowVerdict,
    /// };
    /// use agent_core::Segment;
    ///
    /// let report = GuardReport {
    ///     drift: DriftVerdict::Unexpected { segment: Segment::Tools },
    ///     reconcile: ReconcileVerdict::Blind { predicted: 512 },
    ///     window: WindowVerdict::Healthy { turns: 4, hit_percent: 92, low_streak: 0 },
    /// };
    /// let alerts = report.alerts();
    /// assert_eq!(alerts, vec![GuardAlert::UnexpectedDrift { segment: Segment::Tools }]);
    /// assert_eq!(alerts[0].layer(), GuardLayer::PreFlight);
    /// ```
    pub fn alerts(&self) -> Vec<GuardAlert> {
        let mut out = Vec::new();
        if let DriftVerdict::Unexpected { segment } = self.drift {
            out.push(GuardAlert::UnexpectedDrift { segment });
        }
        if let ReconcileVerdict::Shortfall { predicted, actual, gap } = self.reconcile {
            out.push(GuardAlert::CacheShortfall { predicted, actual, gap });
        }
        if let WindowVerdict::ChronicMiss { streak, turns, hit_percent } = self.window {
            out.push(GuardAlert::ChronicMiss { streak, turns, hit_percent });
        }
        out
    }

    /// 本轮有没有任何一层告警。
    pub fn has_alert(&self) -> bool {
        !self.alerts().is_empty()
    }
}

/// 三行，每层一行，**三层都打**——只打告警的那层，人就不知道另外两层是好是失明。
impl fmt::Display for GuardReport {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        writeln!(f, "{}", self.drift)?;
        writeln!(f, "{}", self.reconcile)?;
        write!(f, "{}", self.window)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 三层同时告警时，三条各自可分辨——类型上分辨，不是靠字符串里找关键字。
    #[test]
    fn three_layers_alert_independently() {
        let report = GuardReport {
            drift: DriftVerdict::Unexpected { segment: Segment::Tools },
            reconcile: ReconcileVerdict::Shortfall { predicted: 1000, actual: 100, gap: 900 },
            window: WindowVerdict::ChronicMiss { streak: 3, turns: 10, hit_percent: 4 },
        };
        let layers: Vec<GuardLayer> = report.alerts().iter().map(GuardAlert::layer).collect();
        assert_eq!(layers, vec![GuardLayer::PreFlight, GuardLayer::Reconcile, GuardLayer::Window]);
        assert!(report.has_alert());
    }

    /// 失明不是告警：三层全失明时 `alerts()` 是空的，但 Display 要说得出「不工作」。
    #[test]
    fn blind_is_not_an_alert() {
        let report = GuardReport {
            drift: DriftVerdict::Clean,
            reconcile: ReconcileVerdict::Blind { predicted: 512 },
            window: WindowVerdict::NoData { skipped: 3 },
        };
        assert!(report.alerts().is_empty());
        assert!(!report.has_alert());
        let text = report.to_string();
        assert_eq!(text.lines().count(), 3);
        assert!(text.contains("本轮第 2 层不工作"), "{text}");
    }
}

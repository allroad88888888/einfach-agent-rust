//! 兜底第 2 层：预测 vs 真实的对账（issue 024）。
//!
//! adapter 发前算「这次应该命中 N 个 token」，响应回来跟真实的 `cached` 对一次。
//! 差太多说明我们对这家的缓存语义理解错了——而理解错这件事**不报错、不影响功能**，
//! 只是每一轮都全价。
//!
//! 这里只有一次减法和一次整数百分比比较。预测怎么算出来的（匹配语义、块粒度）
//! 全在 adapter，core 连问都问不着（红线 12）。

use std::fmt;

use serde::{Deserialize, Serialize};

/// 默认容差：**0 token**，即要求逐 token 对上。
///
/// 看着严，但换任何一个非零默认值都得回答「一个块是多大」——那是块粒度，是模型
/// 相关的知识，写进 core 就违了红线 12。而且不需要：adapter 的预测是**向下**取整
/// 的，取整的零头只会让实际比预测**多**，落进 [`ReconcileVerdict::BetterThanExpected`]
/// （信息级，不吓人）；少的那一侧由相对阈值兜着，相对阈值跟粒度无关。
///
/// 宿主真想压掉零头噪声，可以传非零容差——但那个数只有它和 adapter 说得清。
pub const DEFAULT_TOLERANCE_TOKENS: u32 = 0;

/// 默认告警阈值：缺口**超过**预测的 30%（issue 024 验收原文）。
///
/// 恰好 30% 不告警：阈值是「超过」不是「达到」，边界两侧要有确定的一边，
/// 否则同一个数字在两次读代码时会得出不同结论。
pub const DEFAULT_SHORTFALL_ALERT_PERCENT: u32 = 30;

/// 对账的两个旋钮。默认值见 [`DEFAULT_TOLERANCE_TOKENS`] 与
/// [`DEFAULT_SHORTFALL_ALERT_PERCENT`]。
#[derive(Clone, Copy, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub struct ReconcileParams {
    /// 静默带：`|实际 - 预测| <= tolerance_tokens` 一律算一致。
    pub tolerance_tokens: u32,
    /// 差于预期的告警门槛，相对 `predicted` 的百分比。缺口要**同时**超过容差
    /// 和这个比例才告警——小预测上光看比例会被放大成假警报。
    pub shortfall_alert_percent: u32,
}

impl Default for ReconcileParams {
    fn default() -> Self {
        ReconcileParams {
            tolerance_tokens: DEFAULT_TOLERANCE_TOKENS,
            shortfall_alert_percent: DEFAULT_SHORTFALL_ALERT_PERCENT,
        }
    }
}

/// 第 2 层的判读结果。**五种情况必须分开**——挤成一个「对不上」，要么是吓人
/// （好于预期被打成告警），要么是骗人（这家没报被折算成 0 命中）。
///
/// 只有 [`ReconcileVerdict::Shortfall`] 是告警。
///
/// 032：经 `GuardReport.reconcile` 可达，`ts` feature 门后面导出 TS。
#[derive(Clone, Copy, PartialEq, Eq, Debug, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub enum ReconcileVerdict {
    /// 这家的响应里 `cached` 字段整个缺失（`usage.cached == None`）。
    ///
    /// **本轮第 2 层不工作**，不是「没命中」。有一家未命中时字段就是缺的，
    /// 把 `None` 折算成 0 的话，这家的账永远对不上，而且是往「缓存全崩」的方向错。
    Blind { predicted: u32 },
    /// 本轮没有预测（`predicted == 0`：冷启动，或上一轮的镜像不在）。**不判。**
    ///
    /// 带上 `actual` 是因为它常常不是 0——此前某次同前缀的调用把缓存焐热了，
    /// 于是「没预测却命中了」。那不是异常，是预测这一侧信息不全。
    NoPrediction { actual: u32 },
    /// 对得上：差值在容差内，或缺口没到告警阈值。**静默。**
    Match { predicted: u32, actual: u32 },
    /// **好于预期**：实际命中比预测多。冷启动、换前缀、adapter 保守取整都会这样。
    ///
    /// 信息级，**不是告警**。省下来的钱被印成红字，人下次就不看这一层了。
    BetterThanExpected {
        predicted: u32,
        actual: u32,
        surplus: u32,
    },
    /// 差于预期且超过阈值：**告警**，带缺口的绝对数字。
    ///
    /// 意思是「我们对这家缓存语义的理解错了」，不是「模型出错了」——功能一切正常，
    /// 只有账单知道。
    Shortfall {
        predicted: u32,
        actual: u32,
        gap: u32,
    },
}

impl ReconcileVerdict {
    /// 缺口占预测的百分比（向下取整）。只有 [`ReconcileVerdict::Shortfall`] 有值。
    pub fn shortfall_percent(&self) -> Option<u32> {
        match self {
            ReconcileVerdict::Shortfall { predicted, gap, .. } if *predicted > 0 => Some(
                u32::try_from(u64::from(*gap) * 100 / u64::from(*predicted)).unwrap_or(u32::MAX),
            ),
            _ => None,
        }
    }
}

/// 第 2 层：拿 adapter 的预测跟真实 usage 对账。
///
/// `predicted` 是 adapter 报的 `predicted_cache`，`cached` 直接来自
/// [`crate::TokenUsage::cached`]——**`None` 和 `Some(0)` 必须原样传进来**，
/// 在调用处 `unwrap_or(0)` 就把「这家不报」和「真的没命中」揉成了一件事。
///
/// 判读顺序（`predicted == 0` 与 `cached == None` 同时成立时，报 `Blind`）：
/// 没有真实数字就没什么可对的账，这是数据源的问题，比「本轮没预测」更靠前。
///
/// **纯函数**，不读时钟不读随机（红线 1）。
///
/// ```
/// use agent_core::cache::{reconcile, ReconcileParams, ReconcileVerdict};
///
/// let p = ReconcileParams::default();
///
/// // 真实两轮的数字：冷启动不预测但命中了，以及第二轮预测与实际一致。
/// assert_eq!(reconcile(0, Some(512), p), ReconcileVerdict::NoPrediction { actual: 512 });
/// assert_eq!(
///     reconcile(512, Some(512), p),
///     ReconcileVerdict::Match { predicted: 512, actual: 512 }
/// );
///
/// // 这家没报 cached：第 2 层这轮不工作，绝不折算成 0。
/// assert_eq!(reconcile(512, None, p), ReconcileVerdict::Blind { predicted: 512 });
///
/// // 缺口超过 30% 才告警，带缺口数字。
/// assert_eq!(
///     reconcile(1000, Some(500), p),
///     ReconcileVerdict::Shortfall { predicted: 1000, actual: 500, gap: 500 }
/// );
/// ```
pub fn reconcile(predicted: u32, cached: Option<u32>, params: ReconcileParams) -> ReconcileVerdict {
    let Some(actual) = cached else {
        return ReconcileVerdict::Blind { predicted };
    };
    if predicted == 0 {
        return ReconcileVerdict::NoPrediction { actual };
    }

    if actual >= predicted {
        let surplus = actual - predicted;
        return if surplus <= params.tolerance_tokens {
            ReconcileVerdict::Match { predicted, actual }
        } else {
            ReconcileVerdict::BetterThanExpected {
                predicted,
                actual,
                surplus,
            }
        };
    }

    let gap = predicted - actual;
    // 整数比较，不换算成浮点：30% 边界上的判定必须是确定的。
    let over_ratio =
        u64::from(gap) * 100 > u64::from(params.shortfall_alert_percent) * u64::from(predicted);
    if gap > params.tolerance_tokens && over_ratio {
        ReconcileVerdict::Shortfall {
            predicted,
            actual,
            gap,
        }
    } else {
        ReconcileVerdict::Match { predicted, actual }
    }
}

/// 一句话中文，可直接进 CLI。**三种「对上了没」的情况措辞分开**，
/// 好于预期读起来不能像告警。
impl fmt::Display for ReconcileVerdict {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match *self {
            ReconcileVerdict::Blind { predicted } => write!(
                f,
                "对账：这家没报 cached 字段，本轮第 2 层不工作（预测 {predicted}，不是 0 命中）",
            ),
            ReconcileVerdict::NoPrediction { actual } => {
                write!(
                    f,
                    "对账：本轮无预测（冷启动或上轮镜像缺失），实际命中 {actual}，不判"
                )
            }
            ReconcileVerdict::Match { predicted, actual } if predicted == actual => {
                write!(f, "对账：预测 {predicted} / 实际 {actual}，一致")
            }
            ReconcileVerdict::Match { predicted, actual } => {
                write!(
                    f,
                    "对账：预测 {predicted} / 实际 {actual}，一致（差值未达告警阈值）"
                )
            }
            ReconcileVerdict::BetterThanExpected {
                predicted,
                actual,
                surplus,
            } => write!(
                f,
                "对账：预测 {predicted} / 实际 {actual}，比预期多命中 {surplus}——好于预期，不是问题",
            ),
            ReconcileVerdict::Shortfall {
                predicted,
                actual,
                gap,
            } => {
                let pct = self.shortfall_percent().unwrap_or(0);
                write!(
                    f,
                    "对账告警：预测 {predicted} / 实际 {actual}，缺口 {gap}（{pct}%）——对这家缓存语义的理解可能是错的",
                )
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 30% 是「超过」才告警：边界两侧必须落在不同变体上。
    #[test]
    fn thirty_percent_boundary_is_strict() {
        let p = ReconcileParams::default();
        // 缺口正好 30%：不告警。
        assert!(matches!(
            reconcile(1000, Some(700), p),
            ReconcileVerdict::Match { .. }
        ));
        // 多缺一个 token：告警。
        assert!(matches!(
            reconcile(1000, Some(699), p),
            ReconcileVerdict::Shortfall { gap: 301, .. }
        ));
    }

    /// 容差同时是绝对下限：小预测上，光看比例会把几个 token 的零头放大成告警。
    #[test]
    fn tolerance_is_an_absolute_floor_for_alerts() {
        let p = ReconcileParams {
            tolerance_tokens: 64,
            ..Default::default()
        };
        // 缺口 50 token = 50%，超了比例但没超容差 → 不告警。
        assert!(matches!(
            reconcile(100, Some(50), p),
            ReconcileVerdict::Match { .. }
        ));
        // 容差同样吃掉正向零头：多 64 以内算一致，不刷「好于预期」。
        assert!(matches!(
            reconcile(100, Some(164), p),
            ReconcileVerdict::Match { .. }
        ));
        assert!(matches!(
            reconcile(100, Some(165), p),
            ReconcileVerdict::BetterThanExpected { surplus: 65, .. }
        ));
    }
}

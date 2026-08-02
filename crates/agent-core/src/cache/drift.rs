//! 兜底第 1 层：请求发出**之前**的前缀比对（issue 024）。
//!
//! 输入是 adapter 已经算好的「哪一段漂了」加上 core 自己的「这轮打不打算改前缀」。
//! 这里只做一次归类——零算术、零 IO、零时钟。**为什么漂**不在这一层回答，
//! 那要看 adapter 把料摆成了什么顺序（红线 12）。
//!
//! 这是三层里唯一一层「发现了还来得及」的：判读发生在花钱之前。

use std::fmt;

use serde::{Deserialize, Serialize};

use crate::seam::Segment;

/// 本轮 core 打不打算改前缀。
///
/// **不是 `bool`**：`check_drift(Some(Segment::Tools), false)` 在调用处读不出
/// `false` 是哪一边，而传反了的后果是「事故被当成预期」静默放过——那正是这一层
/// 要拦的东西。
///
/// M1 恒为 [`PrefixIntent::Reuse`]：还没有任何一处会有意改前缀。压缩重写历史、
/// 换 skill 集、晚加的工具被并进顶层，都是后面才出现的 [`PrefixIntent::Intentional`]
/// 来源。字段现在就留出来，是因为漏了它的那一天，表现是「压缩一次报一次假警报」，
/// 然后人开始无视这一层的告警。
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default, Serialize, Deserialize)]
pub enum PrefixIntent {
    /// 沿用上一轮的前缀。**任何 drift 都是事故。**
    #[default]
    Reuse,
    /// 本轮有意改了前缀。drift 是预期内的，不算事故。
    Intentional,
}

/// 第 1 层的判读结果。
///
/// 只有 [`DriftVerdict::Unexpected`] 是告警（见 [`crate::cache::GuardReport::alerts`]），
/// 另外两个是陈述。
///
/// 032：`SessionEvent::PreflightDriftAlert` 的载荷，`ts` feature 门后面导出 TS——
/// 没有 `#[serde(tag = ..)]`，是 serde 默认的外部标签，`Clean` 这类无字段变体在
/// TS 那边落成字符串字面量 `"Clean"`，不是 `{ "Clean": null }`（ts-rs 的
/// serde-compat 会照 serde 真实序列化形状生成，不是照 Rust 语法猜）。
#[derive(Clone, Copy, PartialEq, Eq, Debug, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub enum DriftVerdict {
    /// 该复用的段逐字节没变。
    ///
    /// 本轮有意改前缀但 adapter 报 `None` 时也落这里：改了前缀不等于该复用的段
    /// 一定会变（比如只往末尾追加），没漂就是没漂。
    Clean,
    /// 漂了，但本轮本来就打算改前缀。**不是事故**，代价是这一轮全价重编码。
    Expected { segment: Segment },
    /// 漂了，而本轮压根没打算改前缀。**这是我们自己的 bug**——序列化不确定
    /// （红线 11：`HashMap` 的迭代顺序、时间戳混进了 prompt）或者哪里多拼了一个字节。
    ///
    /// 报出**是哪一段**是这一层的全部价值：只说「前缀变了」，一千条消息里找不出
    /// 是哪个字段的 key 顺序翻了。
    Unexpected { segment: Segment },
}

/// 第 1 层：归类 adapter 报上来的 drift。
///
/// `drift` 是 adapter 对着上一轮镜像算出来的「哪一段漂了」，`None` = 该复用的都没变。
/// `intent` 是 core 自己的意图，决定同一个 drift 算事故还是算预期。
///
/// **纯函数**：同一对输入永远得到同一个结果，不读时钟不读随机（红线 1）。
///
/// ```
/// use agent_core::cache::{check_drift, DriftVerdict, PrefixIntent};
/// use agent_core::Segment;
///
/// // 没打算改前缀，Tools 段却漂了 —— 抓到，且说得出是哪一段。
/// let v = check_drift(Some(Segment::Tools), PrefixIntent::Reuse);
/// assert_eq!(v, DriftVerdict::Unexpected { segment: Segment::Tools });
///
/// // 同样的 drift，本轮有意改前缀 —— 不是事故。
/// let v = check_drift(Some(Segment::Tools), PrefixIntent::Intentional);
/// assert_eq!(v, DriftVerdict::Expected { segment: Segment::Tools });
/// ```
pub fn check_drift(drift: Option<Segment>, intent: PrefixIntent) -> DriftVerdict {
    match (drift, intent) {
        (None, _) => DriftVerdict::Clean,
        (Some(segment), PrefixIntent::Intentional) => DriftVerdict::Expected { segment },
        (Some(segment), PrefixIntent::Reuse) => DriftVerdict::Unexpected { segment },
    }
}

/// 一句话中文，可直接进 CLI。
impl fmt::Display for DriftVerdict {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            DriftVerdict::Clean => write!(f, "发前比对：该复用的段逐字节没变"),
            DriftVerdict::Expected { segment } => {
                write!(f, "发前比对：{segment:?} 段变了，本轮本来就打算改前缀（预期内，这一轮全价）")
            }
            DriftVerdict::Unexpected { segment } => write!(
                f,
                "发前比对告警：{segment:?} 段漂了，但本轮没打算改前缀——先查这一段的序列化，别急着发",
            ),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 三种输入组合各自落在不同的变体上，`None` 与意图无关。
    #[test]
    fn classification_is_exhaustive() {
        assert_eq!(check_drift(None, PrefixIntent::Reuse), DriftVerdict::Clean);
        assert_eq!(check_drift(None, PrefixIntent::Intentional), DriftVerdict::Clean);
        assert_eq!(
            check_drift(Some(Segment::System), PrefixIntent::Reuse),
            DriftVerdict::Unexpected { segment: Segment::System }
        );
        assert_eq!(
            check_drift(Some(Segment::History), PrefixIntent::Intentional),
            DriftVerdict::Expected { segment: Segment::History }
        );
    }

    /// M1 的默认意图是「沿用」——默认值站错边，事故会被当成预期放过。
    #[test]
    fn default_intent_treats_drift_as_accident() {
        assert_eq!(PrefixIntent::default(), PrefixIntent::Reuse);
        assert!(matches!(
            check_drift(Some(Segment::Tools), PrefixIntent::default()),
            DriftVerdict::Unexpected { .. }
        ));
    }
}

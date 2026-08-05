//! 兜底第 3 层：滚动窗口的命中率（issue 024）。
//!
//! 前两层漏掉的慢性失效，只有攒够几轮才看得出来：每轮都全价、命中率崩、单位轮次
//! 花费异常。它同时是花费失控的**事后闸**——agent 进死循环这类没料到的模式，
//! 形态恰好长这样。
//!
//! **按轮计数，不按时间**（红线 1）：窗口是「最近 N 轮」不是「最近 N 分钟」，
//! 于是同一份历史重放两次一定得出同一个告警。

use std::fmt;

use serde::{Deserialize, Serialize};

use crate::value::session::TokenUsage;

/// 默认窗口：**最近 10 轮**有观测的轮次。
///
/// 短了（3–5）跟「连续 K 轮」这个触发条件基本重合，窗口就白设了；长了（30+）
/// 一次真实的前缀变更要拖三十轮才淡出，报出来的命中率跟当下对不上。
pub const DEFAULT_WINDOW: usize = 10;

/// 默认单轮「低命中」门槛：命中率低于 **50%**。
///
/// 暖着的对话命中率是九成以上，一次前缀变更会把某一轮打到 0——50% 分得开这两者，
/// 而且离两边都远，不会因为块取整的零头来回抖。
pub const DEFAULT_LOW_HIT_PERCENT: u32 = 50;

/// 默认告警条件：**连续 3 轮**低命中。
///
/// 单轮低命中是正常现象（换前缀、压缩、第一次见这个变体）。连续三轮说明不是
/// 一次性代价，是这条会话的前缀根本没在复用。
pub const DEFAULT_CONSECUTIVE_ALERT: usize = 3;

/// 一轮的命中观测。**构造时就把「这家没报」跟「真的没命中」分开**——
/// 揉在一起的话，不报 cached 的那家会被这一层判成「一直全价」，天天假告警。
#[derive(Clone, Copy, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub enum TurnHit {
    /// 这家的响应里 `cached` 字段整个缺失。**这一轮不进窗口**：
    /// 失明轮不能既不算好也不算坏，只能不算。
    Blind,
    /// 报了。`prompt` 是本轮 prompt 总数，`cached` 是其中命中的部分。
    Observed { prompt: u32, cached: u32 },
}

impl TurnHit {
    /// 从一次请求的 usage 取观测：`cached == None` → [`TurnHit::Blind`]，
    /// `Some(n)` → [`TurnHit::Observed`]（含 `Some(0)`，那是**真的没命中**，要进窗口）。
    pub fn from_usage(usage: &TokenUsage) -> Self {
        match usage.cached {
            None => TurnHit::Blind,
            Some(cached) => TurnHit::Observed {
                prompt: usage.prompt,
                cached,
            },
        }
    }

    /// 本轮命中率（百分比，向下取整）。失明轮或 `prompt == 0` 的退化轮返回 `None`
    /// ——0 个 prompt token 没有命中率可言，让它算 0% 会凭空造出低命中轮。
    ///
    /// `cached` 超过 `prompt` 时按 `prompt` 封顶：那是上游解析出了问题，
    /// 但这一层不该因此报出 300% 这种没法读的数。
    pub fn hit_percent(&self) -> Option<u32> {
        match *self {
            TurnHit::Blind => None,
            TurnHit::Observed { prompt: 0, .. } => None,
            TurnHit::Observed { prompt, cached } => Some(
                u32::try_from(u64::from(cached.min(prompt)) * 100 / u64::from(prompt))
                    .unwrap_or(100),
            ),
        }
    }
}

/// 窗口的三个旋钮。默认值见 [`DEFAULT_WINDOW`] / [`DEFAULT_LOW_HIT_PERCENT`] /
/// [`DEFAULT_CONSECUTIVE_ALERT`]。
#[derive(Clone, Copy, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub struct WindowParams {
    /// 窗口大小，按**有观测的轮次**计——失明轮不占位。
    pub window: usize,
    /// 单轮命中率低于这个百分比算「低命中」。
    pub low_hit_percent: u32,
    /// 连续多少轮低命中才告警。
    pub consecutive_alert: usize,
}

impl Default for WindowParams {
    fn default() -> Self {
        WindowParams {
            window: DEFAULT_WINDOW,
            low_hit_percent: DEFAULT_LOW_HIT_PERCENT,
            consecutive_alert: DEFAULT_CONSECUTIVE_ALERT,
        }
    }
}

/// 第 3 层的判读结果。只有 [`WindowVerdict::ChronicMiss`] 是告警。
///
/// 032：经 `GuardReport.window` 可达，`ts` feature 门后面导出 TS。
#[derive(Clone, Copy, PartialEq, Eq, Debug, Serialize, Deserialize)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub enum WindowVerdict {
    /// 窗口里一个有观测的轮次都没有（全失明，或还没跑过）。**第 3 层不工作**，
    /// 跟「命中率 0%」是两件事。`skipped` 是为凑满窗口扫过、又跳过的轮数。
    NoData { skipped: usize },
    /// 还没到告警条件。`hit_percent` 是窗口内按 token 加权的整体命中率，
    /// `low_streak` 是最近连续低命中的轮数（0 表示最近一轮是好的）。
    Healthy {
        turns: usize,
        hit_percent: u32,
        low_streak: usize,
    },
    /// 连续 `streak` 轮低命中，达到告警条件——慢性失效。
    ChronicMiss {
        streak: usize,
        turns: usize,
        hit_percent: u32,
    },
}

/// 第 3 层：从最近的轮次观测里判读慢性失效。
///
/// `history` 按时间**从旧到新**，可以是整条会话——只有末尾 `params.window` 个
/// **有观测的**轮次参与判读。
///
/// 告警条件是「连续 K 轮低命中」而不是「窗口平均低于阈值」：平均值滞后，且一次
/// 昂贵的全价重编码就能把平均拖下去，而那可能完全正常（压缩、换 skill）。
/// 窗口整体命中率照样报出来，但它是**给人看的数字**，不是触发条件。
///
/// 失明轮既不进窗口，也**不打断连续性**——它对这一层是不存在的。
///
/// **纯函数**，按轮计数不按时间（红线 1）。
///
/// ```
/// use agent_core::cache::{check_window, TurnHit, WindowParams, WindowVerdict};
///
/// let warm = vec![TurnHit::Observed { prompt: 1000, cached: 900 }; 10];
/// assert!(matches!(check_window(&warm, WindowParams::default()), WindowVerdict::Healthy { .. }));
///
/// let broken = vec![TurnHit::Observed { prompt: 1000, cached: 0 }; 3];
/// assert!(matches!(
///     check_window(&broken, WindowParams::default()),
///     WindowVerdict::ChronicMiss { streak: 3, .. }
/// ));
///
/// // 三轮失明不进窗口：判不出好坏，就不判。
/// let blind = vec![TurnHit::Blind; 3];
/// assert_eq!(check_window(&blind, WindowParams::default()), WindowVerdict::NoData { skipped: 3 });
/// ```
pub fn check_window(history: &[TurnHit], params: WindowParams) -> WindowVerdict {
    // 从最新往回收，收满窗口为止。收进来的顺序是「新 → 旧」，
    // 于是「最近连续低命中」就是这个序列的前缀。
    let mut recent: Vec<(u32, u32, u32)> = Vec::with_capacity(params.window);
    let mut skipped = 0usize;
    for turn in history.iter().rev() {
        if recent.len() >= params.window {
            break;
        }
        match (turn, turn.hit_percent()) {
            (TurnHit::Observed { prompt, cached }, Some(percent)) => {
                recent.push((*prompt, *cached, percent));
            }
            _ => skipped += 1,
        }
    }

    if recent.is_empty() {
        return WindowVerdict::NoData { skipped };
    }

    let turns = recent.len();
    let prompt_sum: u64 = recent.iter().map(|(p, _, _)| u64::from(*p)).sum();
    let cached_sum: u64 = recent.iter().map(|(_, c, _)| u64::from(*c)).sum();
    let hit_percent = u32::try_from(cached_sum.min(prompt_sum) * 100 / prompt_sum).unwrap_or(100);

    let low_streak = recent
        .iter()
        .take_while(|(_, _, percent)| *percent < params.low_hit_percent)
        .count();

    if params.consecutive_alert > 0 && low_streak >= params.consecutive_alert {
        WindowVerdict::ChronicMiss {
            streak: low_streak,
            turns,
            hit_percent,
        }
    } else {
        WindowVerdict::Healthy {
            turns,
            hit_percent,
            low_streak,
        }
    }
}

/// 一句话中文，可直接进 CLI。
impl fmt::Display for WindowVerdict {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match *self {
            WindowVerdict::NoData { skipped } => {
                write!(
                    f,
                    "滚动窗口：窗口内没有可用观测（跳过 {skipped} 轮），本层不工作"
                )
            }
            WindowVerdict::Healthy {
                turns,
                hit_percent,
                low_streak: 0,
            } => {
                write!(f, "滚动窗口：最近 {turns} 轮命中率 {hit_percent}%")
            }
            WindowVerdict::Healthy {
                turns,
                hit_percent,
                low_streak,
            } => write!(
                f,
                "滚动窗口：最近 {turns} 轮命中率 {hit_percent}%（已连续 {low_streak} 轮低命中，还没到告警线）",
            ),
            WindowVerdict::ChronicMiss {
                streak,
                turns,
                hit_percent,
            } => write!(
                f,
                "滚动窗口告警：已连续 {streak} 轮低命中，最近 {turns} 轮整体命中率 {hit_percent}%——前缀基本没在复用",
            ),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 失明轮不进窗口，也不打断连续性：夹在中间的 `Blind` 不该让告警落空。
    #[test]
    fn blind_turns_neither_count_nor_break_the_streak() {
        let history = vec![
            TurnHit::Observed {
                prompt: 1000,
                cached: 0,
            },
            TurnHit::Blind,
            TurnHit::Observed {
                prompt: 1000,
                cached: 0,
            },
            TurnHit::Observed {
                prompt: 1000,
                cached: 0,
            },
        ];
        assert!(matches!(
            check_window(&history, WindowParams::default()),
            WindowVerdict::ChronicMiss {
                streak: 3,
                turns: 3,
                ..
            }
        ));
    }

    /// `Some(0)`（真的没命中）必须进窗口——它跟 `None` 走的是两条路。
    #[test]
    fn explicit_zero_enters_the_window() {
        let usage = TokenUsage {
            prompt: 500,
            completion: 10,
            cached: Some(0),
        };
        assert_eq!(
            TurnHit::from_usage(&usage),
            TurnHit::Observed {
                prompt: 500,
                cached: 0
            }
        );
        assert_eq!(TurnHit::from_usage(&usage).hit_percent(), Some(0));

        let blind = TokenUsage {
            prompt: 500,
            completion: 10,
            cached: None,
        };
        assert_eq!(TurnHit::from_usage(&blind), TurnHit::Blind);
        assert_eq!(TurnHit::from_usage(&blind).hit_percent(), None);
    }
}

//! [`AutoTurnHold`]：一轮自驱动的轮次**为什么没有自己开**——
//! [`agent_runtime::AutoTurnHold`] 的可序列化姊妹类型（211）。
//!
//! 另开一个而不是给 `agent-runtime` 那个枚举挂 `Serialize`/`ts_rs::TS`：理由跟
//! [`super::OrphanFate`] 一字不差（那个 crate 至今没有 `ts` feature，也没有理由
//! 为了「跨 SSE 长什么样」背一个代码生成依赖）。字段逐一对应，[`From`] 是那条
//! 翻译线。
//!
//! `tag = "type", content = "data"`：跟 [`super::SessionEvent`] 同一个协议决定。
//! 三个变体都没有载荷，邻接标签在这种形状上退化成 `{"type":"…"}`——**仍然用邻接
//! 标签而不是裸字符串**，因为哪天某个成因要带上一个数字（比如「还差几格」），
//! 加字段不会改动已有变体的线上形状。

use serde::{Deserialize, Serialize};

use agent_runtime::AutoTurnHold as RunnerAutoTurnHold;

/// 有留言等着却没有自开的三种成因，跟 `agent_runtime::auto_turn` 的三条出路
/// 一一对应。
///
/// **载荷是事实，不是句子**：措辞由呈现层组（CLI 在
/// `agent-cli::print::events`，web 在 `packages/web/src/render/notice.ts`），
/// 跟 `OrphanFate` 同一条规矩。
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "type", content = "data")]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub enum AutoTurnHold {
    /// 自驱动预算见底。只有真实用户输入能把它加满。
    BudgetExhausted,
    /// 用户在自驱动跑到一半时喊了停。
    Cancelled,
    /// 刚从崩溃里恢复出来——**恢复不自动开轮**。
    Recovered,
}

impl From<RunnerAutoTurnHold> for AutoTurnHold {
    fn from(hold: RunnerAutoTurnHold) -> Self {
        match hold {
            RunnerAutoTurnHold::BudgetExhausted => AutoTurnHold::BudgetExhausted,
            RunnerAutoTurnHold::Cancelled => AutoTurnHold::Cancelled,
            RunnerAutoTurnHold::Recovered => AutoTurnHold::Recovered,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 三个变体逐一对应。穷举 `match` 已经在编译期保证不漏，这里钉的是**没有
    /// 映射错位**——三个变体都没有载荷，字段级的错位在编译期一点提示都没有，
    /// 把 `Cancelled` 映射成 `Recovered` 照样编译过。
    #[test]
    fn from_runner_hold_translates_variant_for_variant() {
        assert_eq!(
            AutoTurnHold::from(RunnerAutoTurnHold::BudgetExhausted),
            AutoTurnHold::BudgetExhausted
        );
        assert_eq!(
            AutoTurnHold::from(RunnerAutoTurnHold::Cancelled),
            AutoTurnHold::Cancelled
        );
        assert_eq!(
            AutoTurnHold::from(RunnerAutoTurnHold::Recovered),
            AutoTurnHold::Recovered
        );
    }

    /// 邻接标签真的过一遍 serde——TS 那边的判别联合收窄靠的就是这个 `"type"` 键。
    #[test]
    fn auto_turn_hold_serializes_round_trip() {
        let hold = AutoTurnHold::BudgetExhausted;
        let s = serde_json::to_string(&hold).unwrap();
        assert_eq!(s, r#"{"type":"budget_exhausted"}"#);
        assert_eq!(serde_json::from_str::<AutoTurnHold>(&s).unwrap(), hold);
    }
}

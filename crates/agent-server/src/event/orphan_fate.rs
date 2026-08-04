//! [`OrphanFate`]：一个没人领的后台子 agent 在轮末怎么收场——
//! [`agent_runtime::OrphanFate`] 的可序列化姊妹类型（054）。
//!
//! 不给 `agent-runtime` 那个枚举挂 `Serialize`/`ts_rs::TS`：那个 crate 至今
//! 没有 `ts` feature，也没有理由为了「跨 SSE 长什么样」背一个代码生成依赖
//! （红线 7 的精神延伸，issue 032 原话「core 不为代码生成背常驻依赖」对
//! runtime 同样成立）。这里照 [`super::UndoOutcome`] 对 `agent_core::UndoReport`
//! 的先例另开一个，字段逐一对应，[`From`] 是那条翻译线。
//!
//! `tag = "type", content = "data"`：跟 [`super::SessionEvent`] 同一个协议决定，
//! 理由见 `super` 模块文档（邻接标签对任意变体形状都成立）。

use serde::{Deserialize, Serialize};

use agent_runtime::OrphanFate as RunnerOrphanFate;

/// 轮末清算给一个后台子 agent 的三种收场，跟 `agent_runtime::orphan::reap` 的
/// 三条出路一一对应。
///
/// **载荷是事实，不是句子**：措辞由呈现层组（CLI 在
/// `agent-cli::print::events::describe_fate`，web 在
/// `packages/web/src/render/notice.ts`），跟 `AgentActivity` 两个壳各有一份
/// 呈现是同一条规矩。
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "type", content = "data")]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub enum OrphanFate {
    /// 还活着 → 连同 `descendants` 个后代一起被拆掉（`Session::despawn_child`）。
    Despawned { descendants: usize },
    /// 拆不掉（`agent_core::DespawnRefused`），它会以**活着**的状态留到下一轮。
    Kept { reason: String },
    /// 已经跑完，结果在 stash 里躺到轮末没人领，`bytes` 字节被丢弃。
    /// `is_error` 说的是**子自己**成没成。
    Discarded { bytes: usize, is_error: bool },
}

impl From<RunnerOrphanFate> for OrphanFate {
    fn from(fate: RunnerOrphanFate) -> Self {
        match fate {
            RunnerOrphanFate::Despawned { descendants } => OrphanFate::Despawned { descendants },
            RunnerOrphanFate::Kept { reason } => OrphanFate::Kept { reason },
            RunnerOrphanFate::Discarded { bytes, is_error } => {
                OrphanFate::Discarded { bytes, is_error }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 三个变体逐一对应，穷举 `match` 已经在编译期保证不漏——这里额外钉一个
    /// 运行期样本，防止哪天有人把某个变体的字段悄悄改错映射（跟
    /// `super::tests::from_runner_event_maps_text_delta` 同一条理由）。
    #[test]
    fn from_runner_fate_translates_field_for_field() {
        assert_eq!(
            OrphanFate::from(RunnerOrphanFate::Despawned { descendants: 2 }),
            OrphanFate::Despawned { descendants: 2 }
        );
        assert_eq!(
            OrphanFate::from(RunnerOrphanFate::Kept { reason: "StillRead".to_string() }),
            OrphanFate::Kept { reason: "StillRead".to_string() }
        );
        assert_eq!(
            OrphanFate::from(RunnerOrphanFate::Discarded { bytes: 15, is_error: true }),
            OrphanFate::Discarded { bytes: 15, is_error: true }
        );
    }

    /// 邻接标签真的过一遍 serde（不是只看 derive 存在）——TS 那边的判别联合
    /// 收窄靠的就是这个 `"type"` 键。
    #[test]
    fn orphan_fate_serializes_round_trip() {
        let fate = OrphanFate::Discarded { bytes: 15, is_error: false };
        let s = serde_json::to_string(&fate).unwrap();
        assert_eq!(s, r#"{"type":"discarded","data":{"bytes":15,"is_error":false}}"#);
        assert_eq!(serde_json::from_str::<OrphanFate>(&s).unwrap(), fate);
    }
}

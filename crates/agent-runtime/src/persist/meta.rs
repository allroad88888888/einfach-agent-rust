//! [`PersistedMeta`]：`EntryMeta`（agent-core）的落盘形态。
//!
//! `agent_core::EntryMeta` 刻意不 derive `Deserialize`——`label: &'static str` 借不出
//! `'static`（类型文档的原话）。落盘这一层因此需要一份**能反序列化**的姊妹类型：
//! `label` 换成 `String`，`epoch` 换成裸 `u64`（`Epoch` 本身可以 serde，但 `Jsonl<K,V,M>`
//! 的 trait bound 要求 `M: Clone`，用裸 `u64` 少一层 newtype 没有坏处）。
//!
//! 011 的实做记录已经点名这道账：「落盘 schema 归 011，它那一侧的 `label` 是
//! `String`」——这份类型就是那句话的落地。

use serde::{Deserialize, Serialize};

use agent_core::{Epoch, EntryMeta, known_label};

/// `agent_core::EntryMeta` 的可落盘姊妹类型。字段一一对应，只有 `label` 换了类型。
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct PersistedMeta {
    pub turn_id: u64,
    pub epoch: u64,
    pub label: String,
    pub barrier: bool,
}

impl From<&EntryMeta> for PersistedMeta {
    fn from(meta: &EntryMeta) -> Self {
        PersistedMeta {
            turn_id: meta.turn_id,
            epoch: meta.epoch.0,
            label: meta.label.to_string(),
            barrier: meta.barrier,
        }
    }
}

/// 载入路径的翻译失败：这一行落盘的 `label` 不在这一版代码认识的封闭集合里
/// （`agent_core::known_label`）。**不是** IO 层面的损坏（`Jsonl` 已经在自己那一层
/// 处理过尾部半行/中部损坏），这是语义层面的「读得出字节，但认不出这个标签」——
/// 大概率是用更新的代码写的会话文件被更旧的二进制打开。诚实地拒绝，不编一个假标签。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UnknownLabel(pub String);

impl std::fmt::Display for UnknownLabel {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "会话文件里的日志标签 \"{}\" 这一版代码不认识（可能是更新版本写的文件）", self.0)
    }
}

impl std::error::Error for UnknownLabel {}

impl TryFrom<PersistedMeta> for EntryMeta {
    type Error = UnknownLabel;

    fn try_from(meta: PersistedMeta) -> Result<Self, Self::Error> {
        let label = known_label(&meta.label).ok_or_else(|| UnknownLabel(meta.label.clone()))?;
        Ok(EntryMeta { turn_id: meta.turn_id, epoch: Epoch(meta.epoch), label, barrier: meta.barrier })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_known_label_round_trips_through_the_persisted_form() {
        let meta = EntryMeta { turn_id: 3, epoch: Epoch(7), label: "tool_result", barrier: true };
        let persisted = PersistedMeta::from(&meta);
        assert_eq!(persisted, PersistedMeta { turn_id: 3, epoch: 7, label: "tool_result".to_string(), barrier: true });

        let back = EntryMeta::try_from(persisted).unwrap();
        assert_eq!(back, meta);
    }

    #[test]
    fn an_unrecognized_label_is_rejected_not_guessed() {
        let persisted = PersistedMeta { turn_id: 1, epoch: 0, label: "some_future_label".to_string(), barrier: false };
        let err = EntryMeta::try_from(persisted).unwrap_err();
        assert_eq!(err, UnknownLabel("some_future_label".to_string()));
    }
}

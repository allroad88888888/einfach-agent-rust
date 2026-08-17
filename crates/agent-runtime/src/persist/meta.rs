//! [`PersistedMeta`]：`EntryMeta`（agent-core）的落盘形态。
//!
//! `agent_core::EntryMeta` 刻意不 derive `Deserialize`——`label: &'static str` 借不出
//! `'static`（类型文档的原话）。落盘这一层因此需要一份**能反序列化**的姊妹类型：
//! `label` 换成 `String`，`epoch` 换成裸 `u64`（`Epoch` 本身可以 serde，但 `Jsonl<K,V,M>`
//! 的 trait bound 要求 `M: Clone`，用裸 `u64` 少一层 newtype 没有坏处）。
//!
//! 011 的实做记录已经点名这道账：「落盘 schema 归 011，它那一侧的 `label` 是
//! `String`」——这份类型就是那句话的落地。
//!
//! ## 199 之前写的会话文件怎么读回来
//!
//! 那一版的字段是 `barrier: bool`，这一版是三态的
//! [`Undoability`]。映射**逐字确定**：`barrier: true → Blocked`、
//! `barrier: false → StateOnly`。老会话本来就没有还原钩子（`Hooked` 这一档是 199
//! 才有的），所以这个映射对它们是**真的**，不是将就（199 §九）。
//!
//! 迁移做在 [`RawMeta`] 上而不是给 `undoability` 加个 `#[serde(default)]`：默认值
//! 会把老文件里 `barrier: true` 的那一条读成 `StateOnly`，也就是**一条真实的不可逆
//! 操作从此不再挡 undo**——不报错、不 panic，只是某一天用户撤销时副作用悄悄留在了
//! 外面。这正是红线导言点名的那类 bug，所以老字段必须真的被读进来。
//!
//! 这一份类型同时供两个后端用（`jsonl` 与 `idb`——两边都是
//! `SessionStore<AtomKey, AgentValue, PersistedMeta>`，`M` 全程泛型透传），所以迁移
//! 写在这里一处，两条持久化路一起生效。

use serde::{Deserialize, Serialize};

use agent_core::{EntryMeta, Epoch, Undoability, known_label};

/// `agent_core::EntryMeta` 的可落盘姊妹类型。字段一一对应，只有 `label` 换了类型。
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(from = "RawMeta")]
pub struct PersistedMeta {
    pub turn_id: u64,
    pub epoch: u64,
    pub label: String,
    pub undoability: Undoability,
}

/// 反序列化的中转形状：**两个版本的字段都收**，由 [`From`] 决定信哪个。
///
/// `undoability` 有值就用它（199 之后写的文件）；没有就看老的 `barrier` 位。
/// 两个都没有的行只可能是被截断/损坏的（`Jsonl` 那一层已经处理过尾部半行），
/// 退回 `StateOnly` 是这里能做的最保守的猜——它不会凭空**取消**一道屏障，
/// 因为老文件里真有屏障的那一行一定带着 `barrier: true`。
#[derive(Deserialize)]
struct RawMeta {
    turn_id: u64,
    epoch: u64,
    label: String,
    #[serde(default)]
    undoability: Option<Undoability>,
    #[serde(default)]
    barrier: Option<bool>,
}

impl From<RawMeta> for PersistedMeta {
    fn from(raw: RawMeta) -> Self {
        let migrated = match raw.barrier {
            Some(true) => Undoability::Blocked,
            _ => Undoability::StateOnly,
        };
        let undoability = raw.undoability.unwrap_or(migrated);
        PersistedMeta {
            turn_id: raw.turn_id,
            epoch: raw.epoch,
            label: raw.label,
            undoability,
        }
    }
}

impl From<&EntryMeta> for PersistedMeta {
    fn from(meta: &EntryMeta) -> Self {
        PersistedMeta {
            turn_id: meta.turn_id,
            epoch: meta.epoch.0,
            label: meta.label.to_string(),
            undoability: meta.undoability,
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
        write!(
            f,
            "会话文件里的日志标签 \"{}\" 这一版代码不认识（可能是更新版本写的文件）",
            self.0
        )
    }
}

impl std::error::Error for UnknownLabel {}

impl TryFrom<PersistedMeta> for EntryMeta {
    type Error = UnknownLabel;

    fn try_from(meta: PersistedMeta) -> Result<Self, Self::Error> {
        let label = known_label(&meta.label).ok_or_else(|| UnknownLabel(meta.label.clone()))?;
        Ok(EntryMeta {
            turn_id: meta.turn_id,
            epoch: Epoch(meta.epoch),
            label,
            undoability: meta.undoability,
        })
    }
}

#[cfg(test)]
#[path = "meta_tests.rs"]
mod tests;

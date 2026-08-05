//! 快照长什么样：全部 primitive 的「逻辑键 → 值」清单，可落盘。这个文件对 store
//! 一无所知 —— 不 import `AtomId`，键是泛型 `K`。
//!
//! 「怎么从 store 采集、怎么灌回去」在同目录的 [`capture`](super::capture)（它认
//! `AtomId`）。这一刀和 009 的 `log` / `record` 是同一刀，理由也一样：红线 4 在本 crate
//! 的形状是**可落盘的那一侧根本没有 `AtomId` 这个符号**，`scripts/check-invariants.sh`
//! 因此在结构上永不可能被触发，而不是靠人记得别写。
//!
//! ## 只存 primitive
//!
//! 「完整状态 = 所有 primitive atom 的值」（`docs/STATE-MODEL.md`），derived 全部可重算
//! —— 这正是红线 1（derived 的 read fn 必须是纯函数）买来的东西。把 derived 也存进来
//! 不是多存一份保险，是给 schema 演进埋一个一致性负担：旧快照里那个算出来的值，和新版
//! 构图函数算出来的对不上时，它长得和真状态一模一样，会被当真。

use serde::{Deserialize, Serialize};

/// 一份快照：**全部 primitive** 的 `(逻辑键, 值)`。
///
/// # 为什么是 `Vec` 而不是 map
///
/// 1. 键的语义由上层选（红线 4：落盘用逻辑键，不用进程内自增的 `AtomId`）。store 层
///    连 `K: Ord` / `K: Hash` 都不该要求 —— 要求了就是在替上层决定键怎么比较。
/// 2. 顺序 = 上层喂进来的顺序，[`capture`](super::capture) 不重排。落盘字节要不要
///    逐字节确定，由上层排序决定（红线 11 是同一个理由：`HashMap` 的迭代顺序在 Rust
///    里是随机化的，而 `AtomFamily` 内部正是 `HashMap`）。
/// 3. 重复的键不是这里的事。快照里同一个键出现两次是上层的 bug；去重要 `Hash`/`Ord`，
///    见第 1 条。[`restore`](super::restore) 的行为是后写覆盖先写。
///
/// # schema 演进白拿
///
/// 恢复时三种键差异各有各的答案，都不需要迁移脚本：
///
/// | 情况 | 谁负责 |
/// |------|--------|
/// | 快照里有、当前图里也有 | [`restore`](super::restore) 灌回 |
/// | 快照里**没有**（新增的槽位） | 构图函数建它时的默认值就是答案，`restore` 不碰 |
/// | 快照里**多出来**（删掉的槽位） | `restore` 的 `on_unknown` 回调报给上层，不静默丢 |
#[derive(Clone, PartialEq, Debug, Serialize, Deserialize)]
pub struct Snapshot<K, V> {
    pub values: Vec<(K, V)>,
}

/// 采集与灌回在同目录的 `capture`（那个文件认 `AtomId`，这个文件不认），这里再导出
/// 一次：issue 010 点名的是 `snapshot.rs` 这一个文件，把路径保持可用比让调用方去记
/// 「实现被红线 4 逼成了两个文件」便宜。
pub use super::capture::{capture, restore};

#[cfg(test)]
mod tests {
    use super::*;

    type Snap = Snapshot<String, i64>;

    fn snap(pairs: &[(&str, i64)]) -> Snap {
        Snapshot {
            values: pairs.iter().map(|(k, v)| ((*k).to_string(), *v)).collect(),
        }
    }

    #[test]
    fn serde_roundtrip_keeps_the_logical_keys_and_their_order() {
        // 键是 String（逻辑键），不是 AtomId —— 红线 4。顺序原样往返：落盘件的字节
        // 是否逐字节确定，由喂进来的顺序决定，序列化这一层不重排。
        let s = snap(&[("agent/root/messages", 3), ("agent/a1/tokens", 12)]);
        let json = serde_json::to_string(&s).unwrap();
        let back: Snap = serde_json::from_str(&json).unwrap();
        assert_eq!(back, s);
        assert!(json.find("messages").unwrap() < json.find("tokens").unwrap());
    }

    #[test]
    fn an_empty_snapshot_roundtrips() {
        // 「一个 primitive 都还没写过」是合法的会话起点，不是错误。
        let s: Snap = Snapshot { values: Vec::new() };
        let back: Snap = serde_json::from_str(&serde_json::to_string(&s).unwrap()).unwrap();
        assert_eq!(back, s);
        assert!(back.values.is_empty());
    }

    #[test]
    fn the_same_key_may_appear_twice_and_survives_the_roundtrip_unchanged() {
        // 去重不是这一层的事（要 Hash/Ord）。往返之后两条都还在，顺序不变 ——
        // restore 的「后写覆盖先写」因此是可预测的。
        let s = snap(&[("k", 1), ("k", 2)]);
        let back: Snap = serde_json::from_str(&serde_json::to_string(&s).unwrap()).unwrap();
        assert_eq!(
            back.values,
            vec![("k".to_string(), 1), ("k".to_string(), 2)]
        );
    }
}

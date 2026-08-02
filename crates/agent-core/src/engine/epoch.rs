//! 在飞 effect 的世代标记（红线 6）。
//!
//! 一次 provider 调用或 tool 执行发出去之后，世界可能已经变了——用户按了取消、
//! 按了 undo。结果回来时如果直接回写，写进的是一个**已经被回滚掉的世界**：
//! 一个「幽灵结果」，偶发、依赖时序、难复现。
//!
//! 解法只有三步，缺一不可（STATE-MODEL §「在飞的 effect」）：
//!
//! 1. effect 发出时带上当时的 epoch
//! 2. 取消 / undo 时 bump epoch
//! 3. 结果回写前比对，不等就丢弃
//!
//! 第 3 步在 [`crate::engine::step`] 的入口（本 issue 就做掉，见那里的「epoch 闸」）；
//! 第 2 步的**时机**是转移表的事（取消在 016，undo 在 017），这里只给 [`Epoch::next`]
//! 这个原语。

use serde::{Deserialize, Serialize};

/// 世代号。**只增不减**——所以「不等于当前」就等价于「过期」，比对用 `!=` 而不用
/// `<`，少一个方向搞反的机会。
///
/// 是 `u64` 不是时间戳：红线 1 要求重放能得出同样的结果，时间戳做不到。
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug, Serialize, Deserialize)]
pub struct Epoch(pub u64);

impl Epoch {
    /// 一个会话开局的世代。
    pub const START: Epoch = Epoch(0);

    /// 下一代。**取消和 undo 调它**，调完之前所有在飞 effect 的结果全部作废。
    ///
    /// 不做 `checked_add`：u64 溢出要 1.8e19 次取消，比进程寿命长得多，加一层
    /// `Option` 只会让每个调用点多一个 `unwrap`。
    #[must_use]
    pub fn next(self) -> Epoch {
        Epoch(self.0 + 1)
    }
}

impl Default for Epoch {
    fn default() -> Self {
        Epoch::START
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip() {
        let e = Epoch(7);
        let s = serde_json::to_string(&e).unwrap();
        assert_eq!(serde_json::from_str::<Epoch>(&s).unwrap(), e);
    }

    /// 只增不减，且 `next` 不改原值（`Copy` 语义）——闸门靠这两条才能用 `!=` 判过期。
    #[test]
    fn next_is_monotonic_and_pure() {
        let e = Epoch::START;
        assert_eq!(e.next(), Epoch(1));
        assert_eq!(e, Epoch::START);
        assert!(e.next() > e);
        assert_eq!(Epoch::default(), Epoch::START);
    }
}

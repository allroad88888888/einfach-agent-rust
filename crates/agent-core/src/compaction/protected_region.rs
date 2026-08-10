//! 「最近 N 轮」这条保护区的线画在历史的哪一格（096 §四第 3 条）。
//!
//! 第 2 档拿它当「哪些工具结果够不着」的上界，第 3 档拿它当摘要的 `upto`——
//! **同一条线**，所以它住在两者之外。两处各画一条的那一天，会出现「摘要盖住了
//! 一段第 2 档还认为在保护区里」的错位，而错位不报错。
//!
//! # 「轮」怎么数（102 定死，别自己发明）
//!
//! **一条 [`Role::User`] 消息开启一轮。**「最近 N 轮」＝从倒数第 N 条 `User`
//! 消息（含）到历史末尾。历史里 `User` 消息不足 N 条 → 整个历史都在保护区。
//!
//! 全程只看 `Role`，**不看时间戳、不看 `turn_id`**（红线 1）：纯结构的定义，
//! 同一份历史重放两次一定得出同一条线。

use imbl::Vector;

use crate::value::message::{Message, Role};

/// 保护区之外那段历史的**独占**上界：`history[..n]` 都不受保护，`history[n..]`
/// 是最近 `protect_recent_turns` 轮，一个字节都不许动。
///
/// 两个端点：`protect_recent_turns == 0` → 返回历史长度（没有保护区）；
/// `User` 消息不足 `protect_recent_turns` 条 → 返回 `0`（整个历史都在保护区，
/// 第 2 档无可清、第 3 档无可摘）。
pub(crate) fn protected_region_start(
    history: &Vector<Message>,
    protect_recent_turns: usize,
) -> usize {
    if protect_recent_turns == 0 {
        return history.len();
    }

    // 每条 `Role::User` 消息在历史里的下标，保持原有顺序（最老在前）。
    let user_turn_starts: Vec<usize> = history
        .iter()
        .enumerate()
        .filter_map(|(idx, message)| (message.role == Role::User).then_some(idx))
        .collect();

    // `checked_sub` 天然处理「`User` 不足 N 条」→ `None` → 边界 0。
    match user_turn_starts.len().checked_sub(protect_recent_turns) {
        Some(offset) => user_turn_starts[offset],
        None => 0,
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use crate::ids::MessageId;
    use crate::value::message::ContentBlock;

    use super::*;

    fn msg(id: u64, role: Role) -> Message {
        Message {
            id: MessageId(id),
            role,
            blocks: vec![ContentBlock::Text(Arc::from("x"))],
        }
    }

    /// 四轮，每轮一条 `User` + 一条 `Assistant`：下标 0/2/4/6 是四条 `User`。
    fn four_turns() -> Vector<Message> {
        Vector::from(
            (0..8)
                .map(|i| {
                    let role = if i % 2 == 0 { Role::User } else { Role::Assistant };
                    msg(i as u64, role)
                })
                .collect::<Vec<_>>(),
        )
    }

    /// 线画在倒数第 N 条 `User` 消息上（含它自己）。
    #[test]
    fn the_line_sits_on_the_nth_user_message_from_the_end() {
        let history = four_turns();
        assert_eq!(protected_region_start(&history, 1), 6);
        assert_eq!(protected_region_start(&history, 2), 4);
        assert_eq!(protected_region_start(&history, 3), 2);
        assert_eq!(protected_region_start(&history, 4), 0);
    }

    /// `User` 不足 N 条 → 整个历史都在保护区（边界 0），不是 panic、也不是全清。
    #[test]
    fn too_few_user_turns_protects_everything() {
        assert_eq!(protected_region_start(&four_turns(), 5), 0);
        assert_eq!(protected_region_start(&four_turns(), 99), 0);
        assert_eq!(protected_region_start(&Vector::new(), 3), 0);
    }

    /// `0` 轮保护 = 没有保护区，线画在历史末尾（不是「全保护」）。
    #[test]
    fn zero_protected_turns_means_no_protected_region() {
        assert_eq!(protected_region_start(&four_turns(), 0), 8);
        assert_eq!(protected_region_start(&Vector::new(), 0), 0);
    }

    /// 只看 `Role`：一条 `User` 消息在哪个下标就画在哪，跟消息 id 无关。
    #[test]
    fn only_the_role_decides_where_the_line_goes() {
        let history = Vector::from(vec![
            msg(90, Role::Assistant),
            msg(7, Role::Assistant),
            msg(3, Role::User),
            msg(1, Role::Assistant),
        ]);
        assert_eq!(protected_region_start(&history, 1), 2);
    }
}

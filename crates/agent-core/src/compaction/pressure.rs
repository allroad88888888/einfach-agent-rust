//! 窗口压力够不够开火：自动阶梯里**唯一一个数值判据**（096 第一/三问）。
//!
//! 第 2 档（[`clear_policy`](super::clear_policy)）和阶梯本身
//! （[`ladder`](super::ladder)）都要问这一句，所以它住在两者之外——两处各写一遍
//! 的那一天，「触发线是多少」就有了两个可能对不上的答案，而它们对不上的症状是
//! **每轮全价重编码**（096 §三），测试全绿，只在账单上浮出来。
//!
//! **纯函数**（红线 1）：零 IO、零时钟、零随机。**只做算术**（红线 12）：比的是
//! 百分比，不看 provider、不看能力位——决策 17 已经把「DeepSeek 上该压得更狠」
//! 这条路堵死了。

/// 上一轮**实测**的 prompt token 占窗口的百分比**超过** `trigger_percent` 了吗。
///
/// 三种「问不出来」一律答 `false`，不 `unwrap`、不 panic：
///
/// - `last_prompt_tokens` 为 `None`：这一轮没有观测（首轮，或这家 provider 不报）。
/// - `context_window` 为 `None`：未知/不设限（`value/session.rs` 的字段注释点名
///   了「不能直接 `unwrap`」）。
/// - `context_window` 为 `Some(0)`：零窗口会除零。接口只写了 `None` 不触发，
///   这一支是「不许 `unwrap`」那条精神的延伸。
///
/// 恰好等于触发线**不**开火——边界要有确定的一边。
pub(crate) fn over_trigger_line(
    last_prompt_tokens: Option<u32>,
    context_window: Option<u32>,
    trigger_percent: u32,
) -> bool {
    let (Some(prompt), Some(window)) = (last_prompt_tokens, context_window) else {
        return false;
    };
    if window == 0 {
        return false;
    }
    // u64：`u32::MAX * 100` 溢出 u32，两个 u32 入参先提升再乘。交叉相乘
    // （`a * 100 > c * b`）跟 floor 除法**不等价**，边界会漂，弃用。
    let percent = (u64::from(prompt) * 100) / u64::from(window);
    percent > u64::from(trigger_percent)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 缺失或无意义的输入一律不开火，不 panic。
    #[test]
    fn missing_or_degenerate_inputs_never_fire() {
        assert!(!over_trigger_line(Some(u32::MAX), None, 85));
        assert!(!over_trigger_line(None, Some(1000), 85));
        assert!(!over_trigger_line(Some(1), Some(0), 85));
    }

    /// 严格「超过」：恰好等于触发线是不开火那一边。
    #[test]
    fn the_line_itself_is_on_the_quiet_side() {
        assert!(!over_trigger_line(Some(85), Some(100), 85));
        assert!(over_trigger_line(Some(86), Some(100), 85));
    }

    /// 整数除法向下取整：85.9% 算 85，不开火。
    #[test]
    fn the_percentage_floors() {
        assert!(!over_trigger_line(Some(859), Some(1000), 85));
        assert!(over_trigger_line(Some(860), Some(1000), 85));
    }

    /// 大数不溢出：`u32::MAX * 100` 装不进 u32，这里必须先提升到 u64。
    #[test]
    fn a_huge_window_does_not_overflow() {
        assert!(over_trigger_line(Some(u32::MAX), Some(u32::MAX), 85));
        assert!(!over_trigger_line(Some(1), Some(u32::MAX), 85));
    }
}

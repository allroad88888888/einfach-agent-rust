//! 连接期退避：指数 + jitter，**只用在「还没拿到响应」的失败上**。
//!
//! 流式请求一律不重试——已经吐出去的增量收不回来，重试等于重复输出
//! （docs/issues/022-first-provider.md）。连接建立失败（DNS、拒连、握手，或
//! 迟迟等不到响应头）还没产出任何东西给用户看，退避重试是安全的。

use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

/// 退避节奏。三家都不返回限流头（PROVIDERS.md §一），节奏只能自己定。
#[derive(Clone, Copy, Debug)]
pub struct Backoff {
    pub base: Duration,
    /// 总尝试次数上限（含第一次），不是「重试次数」。
    pub max_attempts: u32,
}

impl Default for Backoff {
    fn default() -> Self {
        Backoff { base: Duration::from_millis(200), max_attempts: 3 }
    }
}

impl Backoff {
    /// 第 `attempt` 次失败后该等多久（`attempt` 从 1 开始计数）。指数增长封顶
    /// 在 2^4 倍，外加 0..=一半的抖动——多个客户端同时退避时不至于又同时撞车。
    pub fn delay(&self, attempt: u32) -> Duration {
        let factor = 1u32 << attempt.saturating_sub(1).min(4);
        let exp = self.base.saturating_mul(factor);
        exp + Duration::from_millis(jitter_ms(u64::try_from(exp.as_millis()).unwrap_or(u64::MAX) / 2))
    }
}

/// 粗糙但够用的抖动源：系统时钟纳秒位的低位。这里不需要密码学质量的随机数，
/// 引入 `rand` 依赖换一个可预测性上无所谓的数字不划算。
fn jitter_ms(max: u64) -> u64 {
    use std::time::{SystemTime, UNIX_EPOCH};
    if max == 0 {
        return 0;
    }
    let nanos =
        SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.subsec_nanos()).unwrap_or(0);
    u64::from(nanos) % (max + 1)
}

/// 睡够 `dur`，但每 50ms 检查一次取消标志，标志置位提前返回——连接期退避
/// 也不该吃掉一次 Ctrl-C。
pub fn sleep_cancelable(dur: Duration, cancel: &AtomicBool) {
    const TICK: Duration = Duration::from_millis(50);
    let mut remaining = dur;
    while remaining > Duration::ZERO {
        if cancel.load(Ordering::Relaxed) {
            return;
        }
        let step = remaining.min(TICK);
        std::thread::sleep(step);
        remaining -= step;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn delay_grows_and_stays_bounded() {
        let b = Backoff { base: Duration::from_millis(100), max_attempts: 5 };
        // 下界：至少是没抖动时的指数值；上界：抖动最多加一半。
        for attempt in 1..=5 {
            let factor = 1u32 << (attempt - 1).min(4);
            let floor = b.base * factor;
            let d = b.delay(attempt);
            assert!(d >= floor, "attempt {attempt}: {d:?} < floor {floor:?}");
            assert!(d <= floor + floor / 2 + Duration::from_millis(1), "attempt {attempt}: {d:?} 超出预期上界");
        }
    }

    #[test]
    fn delay_caps_growth_beyond_attempt_five() {
        let b = Backoff { base: Duration::from_millis(10), max_attempts: 10 };
        let d5 = b.delay(5);
        let d9 = b.delay(9);
        // 2^4 封顶后，attempt 5 和 attempt 9 的指数部分该相等（都乘 16）。
        assert!(d9 >= Duration::from_millis(160));
        assert!(d5 >= Duration::from_millis(160));
        assert!(d9 < Duration::from_millis(160 + 80 + 1));
    }

    #[test]
    fn sleep_cancelable_returns_early_when_flag_is_set() {
        let cancel = AtomicBool::new(true);
        let start = std::time::Instant::now();
        sleep_cancelable(Duration::from_secs(5), &cancel);
        assert!(start.elapsed() < Duration::from_millis(200), "该立刻返回，实际 {:?}", start.elapsed());
    }

    #[test]
    fn sleep_cancelable_waits_full_duration_when_not_cancelled() {
        let cancel = AtomicBool::new(false);
        let start = std::time::Instant::now();
        sleep_cancelable(Duration::from_millis(120), &cancel);
        assert!(start.elapsed() >= Duration::from_millis(110));
    }
}

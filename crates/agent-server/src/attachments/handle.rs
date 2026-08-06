use std::fmt;
use std::sync::atomic::{AtomicU64, Ordering};

static NEXT_HANDLE: AtomicU64 = AtomicU64::new(1);
const MAX_HANDLE_NUMBER: u64 = u64::MAX - 1;

/// 不透明的图片引用。它不是授权凭据：每次读取都还要校验 session 所有权。
#[derive(Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct ImageHandle(String);

impl ImageHandle {
    pub(crate) fn allocate() -> Option<Self> {
        // 不依赖随机数；进程内的全局单调序列避免不同 vault 重用同一把手。
        Self::allocate_from(&NEXT_HANDLE)
    }

    /// Parse the exact handle shape exposed to models and HTTP clients.
    ///
    /// Syntax is not authorization: callers must still use [`AttachmentVault::lease`]
    /// with the owning session before the handle can resolve to bytes.
    pub fn parse(value: &str) -> Option<Self> {
        parse_number(value).map(|_| Self(value.to_owned()))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// 恢复历史后把进程内分配器推进到已有句柄之后，避免重启后重用同一个
    /// `img_*` 并把旧历史误指向新字节。
    pub(crate) fn reserve_next(&self) {
        self.reserve_next_in(&NEXT_HANDLE);
    }

    fn allocate_from(counter: &AtomicU64) -> Option<Self> {
        let number = counter
            .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |number| {
                (1..=MAX_HANDLE_NUMBER)
                    .contains(&number)
                    .then(|| number + 1)
            })
            .ok()?;
        Some(Self(format!("img_{number}")))
    }

    fn reserve_next_in(&self, counter: &AtomicU64) {
        if let Some(number) = parse_number(&self.0) {
            counter.fetch_max(number + 1, Ordering::Relaxed);
        }
    }
}

fn parse_number(value: &str) -> Option<u64> {
    let digits = value.strip_prefix("img_")?;
    if digits.is_empty()
        || digits.starts_with('0')
        || !digits.bytes().all(|byte| byte.is_ascii_digit())
    {
        return None;
    }
    let number = digits.parse::<u64>().ok()?;
    (number <= MAX_HANDLE_NUMBER).then_some(number)
}

impl fmt::Debug for ImageHandle {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_tuple("ImageHandle").field(&self.0).finish()
    }
}

impl fmt::Display for ImageHandle {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_accepts_only_canonical_allocatable_numbers() {
        assert_eq!(ImageHandle::parse("img_42").unwrap().as_str(), "img_42");
        for invalid in [
            "img_0",
            "img_00",
            "img_042",
            "img_+42",
            "img_18446744073709551615",
            "img_18446744073709551616",
        ] {
            assert!(ImageHandle::parse(invalid).is_none(), "accepted {invalid}");
        }
    }

    #[test]
    fn allocation_stops_at_the_sentinel_instead_of_wrapping() {
        let counter = AtomicU64::new(MAX_HANDLE_NUMBER);
        let last = ImageHandle::allocate_from(&counter).unwrap();

        assert_eq!(last.as_str(), format!("img_{MAX_HANDLE_NUMBER}"));
        assert!(ImageHandle::allocate_from(&counter).is_none());
        assert_eq!(counter.load(Ordering::Relaxed), u64::MAX);
    }

    #[test]
    fn reserving_the_largest_handle_exhausts_without_wrapping() {
        let counter = AtomicU64::new(1);
        let persisted = ImageHandle::parse(&format!("img_{MAX_HANDLE_NUMBER}")).unwrap();

        persisted.reserve_next_in(&counter);

        assert_eq!(counter.load(Ordering::Relaxed), u64::MAX);
        assert!(ImageHandle::allocate_from(&counter).is_none());
    }
}

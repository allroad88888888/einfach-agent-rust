use std::fmt;
use std::sync::atomic::{AtomicU64, Ordering};

static NEXT_HANDLE: AtomicU64 = AtomicU64::new(1);

/// 不透明的图片引用。它不是授权凭据：每次读取都还要校验 session 所有权。
#[derive(Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct ImageHandle(String);

impl ImageHandle {
    pub(crate) fn allocate() -> Self {
        // 不依赖随机数；进程内的全局单调序列避免不同 vault 重用同一把手。
        Self(format!(
            "img_{}",
            NEXT_HANDLE.fetch_add(1, Ordering::Relaxed)
        ))
    }

    /// Parse the exact handle shape exposed to models and HTTP clients.
    ///
    /// Syntax is not authorization: callers must still use [`AttachmentVault::lease`]
    /// with the owning session before the handle can resolve to bytes.
    pub fn parse(value: &str) -> Option<Self> {
        let digits = value.strip_prefix("img_")?;
        (!digits.is_empty() && digits.bytes().all(|byte| byte.is_ascii_digit()))
            .then(|| Self(value.to_owned()))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
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

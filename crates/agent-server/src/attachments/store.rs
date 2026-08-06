use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use crate::SessionId;

use super::error::{LeaseError, RegisterError};
use super::handle::ImageHandle;
use super::index::Inner;
use super::record::ImageData;
use super::validation::validate;

/// 附件仓的硬配额和存活时长。注册会先完整检查，再一次性记账和写入。
#[derive(Clone, Debug)]
pub struct AttachmentVaultConfig {
    pub max_image_bytes: usize,
    pub max_session_images: usize,
    pub max_session_bytes: usize,
    pub max_global_images: usize,
    pub max_global_bytes: usize,
    pub ttl: Duration,
}

impl Default for AttachmentVaultConfig {
    fn default() -> Self {
        Self {
            max_image_bytes: 10 * 1024 * 1024,
            max_session_images: 16,
            max_session_bytes: 32 * 1024 * 1024,
            max_global_images: 256,
            max_global_bytes: 256 * 1024 * 1024,
            ttl: Duration::from_secs(15 * 60),
        }
    }
}

/// 一次注册的借用输入。字节会在 `register` 成功时复制进 vault。
pub struct ImageRegistration<'a> {
    pub mime: &'a str,
    pub name: Option<&'a str>,
    pub bytes: &'a [u8],
}

/// 线程安全的、进程内的 session-scoped 图片仓。
#[derive(Clone)]
pub struct AttachmentVault {
    config: AttachmentVaultConfig,
    inner: Arc<Mutex<Inner>>,
}

/// 一个活动读取租约。无需且不能 `Clone`；释放后才允许已延期的过期回收完成。
pub struct ImageLease {
    handle: ImageHandle,
    image: Arc<ImageData>,
    vault: Arc<Mutex<Inner>>,
}

impl ImageLease {
    pub fn handle(&self) -> &ImageHandle {
        &self.handle
    }

    pub fn mime(&self) -> &str {
        self.image.mime()
    }

    pub fn name(&self) -> Option<&str> {
        self.image.name()
    }

    pub fn bytes(&self) -> &[u8] {
        self.image.bytes()
    }

    pub fn byte_len(&self) -> usize {
        self.image.byte_len()
    }
}

impl Drop for ImageLease {
    fn drop(&mut self) {
        let mut inner = self.vault.lock().expect("attachment vault mutex poisoned");
        inner.release(&self.handle);
    }
}

impl AttachmentVault {
    pub fn new(config: AttachmentVaultConfig) -> Self {
        Self {
            config,
            inner: Arc::new(Mutex::new(Inner::default())),
        }
    }

    pub fn register(
        &self,
        owner: &SessionId,
        image: ImageRegistration<'_>,
        now: Instant,
    ) -> Result<ImageHandle, RegisterError> {
        validate(&image)?;
        let bytes = image.bytes.len();
        if bytes > self.config.max_image_bytes {
            return Err(RegisterError::ImageByteLimit {
                limit_bytes: self.config.max_image_bytes,
            });
        }

        let mut inner = self.inner.lock().expect("attachment vault mutex poisoned");
        inner.check_quota(owner, bytes, &self.config)?;
        let handle = ImageHandle::allocate();
        inner.insert(
            handle.clone(),
            owner.clone(),
            Arc::new(ImageData::new(image.mime, image.name, image.bytes)),
            now + self.config.ttl,
        );
        Ok(handle)
    }

    /// 在指定时刻读取图片。`sweep(now)` 之后过期图片不再可借；活动租约会延后回收。
    pub fn lease(
        &self,
        owner: &SessionId,
        handle: &ImageHandle,
        now: Instant,
    ) -> Result<ImageLease, LeaseError> {
        let mut inner = self.inner.lock().expect("attachment vault mutex poisoned");
        let image = inner.lease_at(owner, handle, now)?;
        Ok(ImageLease {
            handle: handle.clone(),
            image,
            vault: Arc::clone(&self.inner),
        })
    }

    /// 回收到期图片。正在被读取的图片标记为待回收，直到其 lease 释放。
    pub fn sweep(&self, now: Instant) -> usize {
        self.inner
            .lock()
            .expect("attachment vault mutex poisoned")
            .sweep(now)
    }

    /// 关闭 session 后不再允许新的读取；已拿到的 lease 仍持有其安全的只读快照。
    pub fn close_session(&self, owner: &SessionId) -> usize {
        self.inner
            .lock()
            .expect("attachment vault mutex poisoned")
            .unavailable_session(owner)
    }

    /// 显式容量驱逐入口。对其他 session 的把手一律返回 `Unknown`。
    pub fn evict(&self, owner: &SessionId, handle: &ImageHandle) -> Result<(), LeaseError> {
        self.inner
            .lock()
            .expect("attachment vault mutex poisoned")
            .evict(owner, handle)
    }
}

//! 会话作用域的内存图片附件仓。
//!
//! 这里只保存 provider-neutral 的图片字节和元数据；HTTP、prompt 组装和 provider
//! 上传由后续接缝负责。`ImageHandle` 只是引用，读取仍必须带上所属 `SessionId`。

mod error;
mod handle;
mod index;
mod mime;
mod record;
mod store;
mod validation;

pub use error::{LeaseError, RegisterError};
pub use handle::ImageHandle;
pub use store::{AttachmentVault, AttachmentVaultConfig, ImageLease, ImageRegistration};

#[cfg(test)]
mod lifecycle_tests;
#[cfg(test)]
mod tests;

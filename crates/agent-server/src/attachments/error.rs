use std::fmt;

/// 注册图片时的可恢复失败。错误中永远不带图片字节、文件名或路径。
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RegisterError {
    InvalidMime,
    InvalidName,
    EmptyImage,
    SessionClosed,
    ImageByteLimit { limit_bytes: usize },
    SessionImageLimit { limit: usize },
    SessionByteLimit { limit_bytes: usize },
    GlobalImageLimit { limit: usize },
    GlobalByteLimit { limit_bytes: usize },
}

impl fmt::Display for RegisterError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidMime => f.write_str("invalid image MIME type"),
            Self::InvalidName => f.write_str("invalid image name"),
            Self::EmptyImage => f.write_str("image bytes must not be empty"),
            Self::SessionClosed => f.write_str("attachment session is closed"),
            Self::ImageByteLimit { .. } => f.write_str("image byte quota exceeded"),
            Self::SessionImageLimit { .. } => f.write_str("session image quota exceeded"),
            Self::SessionByteLimit { .. } => f.write_str("session byte quota exceeded"),
            Self::GlobalImageLimit { .. } => f.write_str("global image quota exceeded"),
            Self::GlobalByteLimit { .. } => f.write_str("global byte quota exceeded"),
        }
    }
}

impl std::error::Error for RegisterError {}

/// 查找失败。跨 session 的 handle 也必须表现为 `Unknown`，避免枚举其他 session。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LeaseError {
    Unknown,
    Unavailable,
}

impl fmt::Display for LeaseError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Unknown => f.write_str("unknown image handle"),
            Self::Unavailable => f.write_str("image handle is unavailable"),
        }
    }
}

impl std::error::Error for LeaseError {}

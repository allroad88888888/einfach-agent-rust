//! [`SessionStoreError`]：`Jsonl` 唯一往 `on_error` 里塞的东西。
//!
//! **绝不带 K/V 内容**——状态里可能有用户对话（`docs/issues/011-session-store.md`
//! 的硬约束）。每个变体只带「哪一行、什么类别」，`Io` 那个也只转发
//! `io::ErrorKind` 的描述，不转发底层错误的 `Display`（有些 IO 错误的 `Display`
//! 会把调用参数带出来，宁可信息少一点也不冒这个险）。

/// 见模块文档：分类 + 行号，不带内容。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SessionStoreError {
    /// 打开或写入文件失败。
    Io { detail: String },
    /// 中部损坏：这一行（以及它之后的一切）解析不出合法记录，整份 load 被拒绝
    /// ——不能只加载半份状态（issue 011 的硬约束）。
    CorruptLine { line: usize },
    /// 尾部半行：写到一半断电/杀进程的诚实语义，容忍——从这一行截断，
    /// 之前的内容正常加载。
    TruncatedTail { line: usize },
}

impl std::fmt::Display for SessionStoreError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SessionStoreError::Io { detail } => write!(f, "会话文件 IO 失败：{detail}"),
            SessionStoreError::CorruptLine { line } => {
                write!(f, "会话文件第 {line} 行损坏（非法记录），拒绝加载")
            }
            SessionStoreError::TruncatedTail { line } => {
                write!(f, "会话文件第 {line} 行是不完整的尾行，已忽略并从该行截断")
            }
        }
    }
}

impl std::error::Error for SessionStoreError {}

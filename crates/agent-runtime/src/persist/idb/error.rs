//! [`IdbStoreError`]：`IdbStore` 唯一往 `on_error` 里塞的东西——跟
//! `crate::jsonl::SessionStoreError` 同一条红线：不带 K/V 内容（那里面可能是用户
//! 对话），每个变体只带「哪一类、第几条」。
//!
//! 只有两个变体，比 `SessionStoreError` 少一个「尾部半行」：KV 端口没有这个概念——
//! `put` 要么整个 key 落定要么没落定，不存在「写到一半」的中间态（真正的
//! IndexedDB 事务提供这个保证，[`super::memory_kv::MemoryKv`] 的
//! `Mutex<BTreeMap>` 同理）。所以这里也就没有 `Jsonl::TruncatedTail` 那种
//! 「容忍、从这里截断」的第三态——journal 里每条记录要么完整解析，要么整份拒绝。

use super::kv::KvError;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum IdbStoreError {
    /// KV 操作本身失败（打不开数据库、事务被拒绝、配额超限……）。
    Kv(KvError),
    /// 一条 journal 记录解析不出合法结构——中部损坏，整份拒绝加载（不加载半份状态，
    /// 跟 `Jsonl::CorruptLine` 同一条硬约束）。`index` 是这条记录在
    /// [`KvStore::scan_prefix`](super::kv::KvStore::scan_prefix) 返回顺序里的下标，
    /// 不是 journal key 本身——不带内容，只带「第几条」。
    CorruptRecord { index: usize },
}

impl std::fmt::Display for IdbStoreError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            IdbStoreError::Kv(e) => write!(f, "IndexedDB 会话存储 IO 失败：{e}"),
            IdbStoreError::CorruptRecord { index } => {
                write!(f, "会话 journal 第 {index} 条记录损坏（非法记录），拒绝加载")
            }
        }
    }
}

impl std::error::Error for IdbStoreError {}

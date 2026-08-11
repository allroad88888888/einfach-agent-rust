//! [`KvStore`]：把「怎么存」从「怎么重放」里摘出来的异步端口（issue 114a）。
//!
//! 只给三个操作——get / put / 按前缀扫。这三个就够把 `SessionStore`
//! （`agent_store::SessionStore`）需要的五种写事件（Entry/Snapshot/Cursor/
//! DropOldest/DropAfter）表达成一份 append-only 的「journal」：每次写都是给一个新
//! key `put` 一份序列化的 [`super::record::Record`]，`scan_prefix` 把它们按 key 的
//! 字节序原样吐回来（IndexedDB 的游标天然按 key 排序，[`super::memory_kv::MemoryKv`]
//! 用 `BTreeMap` 保证同一件事）——回放（[`super::replay`]）只是把这些记录按顺序
//! 重新喂给一份 [`agent_store::persist::SessionLog`]，跟 `crate::Jsonl` 重放文件行
//! 是同一个算法，只是「一行」换成了「一个 key」。
//!
//! `get` 目前没有被 [`super::replay`] 用到——回放只需要 `put` + `scan_prefix`。
//! 留着它是因为一个「KV 端口」名副其实就该有单键读，[`super::memory_kv::MemoryKv`]
//! 和 [`super::web_kv`] 两边实现它的成本都接近零，往后（比如免解码只查“存不存在”）
//! 用得上；不是现在就该删掉的死代码。
//!
//! 不要求 `Send`：真正的浏览器实现（`super::web_kv`）握着 `web_sys` 的 `JsValue`，
//! 那类东西在 wasm 单线程模型下本来就不是 `Send`——端口这一层要留出空间给它，不能
//! 现在就用一个 native 独有的约束把路堵死。native 侧真正需要 `Send`（工作线程）的
//! 地方（[`super::store::IdbStore::spawn`]）自己在那里加这个 bound，不该甩给端口。

/// 见模块文档：分类信息，不带 key/value 内容——那两样都可能是用户对话
/// （跟 `crate::jsonl::SessionStoreError` 同一条红线：绝不打印 K/V）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KvError {
    pub detail: String,
}

impl std::fmt::Display for KvError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "KV 存储操作失败：{}", self.detail)
    }
}

impl std::error::Error for KvError {}

/// 异步 KV 端口：get / put / 按前缀扫，仅此三个方法。
///
/// `#[allow(async_fn_in_trait)]`：默认 lint 建议把 `async fn` 换成显式
/// `-> impl Future<..> + Send`，但这里**故意不加 `Send`**——真正的浏览器实现
/// （`super::web_kv`）握着 `web_sys::JsValue`，wasm 单线程模型下那类东西本来就
/// 不是 `Send`，加了这个 bound 就等于堵死 web_kv 去实现这个 trait。
#[allow(async_fn_in_trait)]
pub trait KvStore {
    /// 单个 key 目前的值，`None` = 没有这个 key。
    async fn get(&self, key: &[u8]) -> Result<Option<Vec<u8>>, KvError>;

    /// 写一个 key，覆盖式——重复 `put` 同一个 key 是「换成最新值」，不是追加。
    async fn put(&self, key: &[u8], value: &[u8]) -> Result<(), KvError>;

    /// 按前缀扫描，返回的 `(key, value)` 必须按 key 的字节序排列——[`super::replay`]
    /// 依赖这个顺序等于写入顺序（journal key 是零填充的十进制计数器，字节序 == 数值
    /// 序，见 `record.rs` 模块文档）。这是这个端口对调用方唯一的排序承诺。
    async fn scan_prefix(&self, prefix: &[u8]) -> Result<Vec<(Vec<u8>, Vec<u8>)>, KvError>;
}

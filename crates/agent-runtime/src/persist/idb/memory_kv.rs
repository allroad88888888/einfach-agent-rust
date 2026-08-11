//! [`MemoryKv`]：[`KvStore`] 的假实现，纯内存 `BTreeMap`，进程退出即丢——不需要
//! 浏览器就能把 [`super::store::IdbStore`] 整条回放/游标/压实链路测完，这正是 114a
//! 的关键设计要求（见 `docs/issues/114-wasm-host.md` 的「拆法」一节）。
//!
//! 用 `BTreeMap` 不是 `HashMap`：[`KvStore::scan_prefix`] 承诺按 key 字节序返回
//! （见 `record.rs` 模块文档），`BTreeMap::range` 天然满足；`web_sys::IdbDatabase`
//! 那边（[`super::web_kv`]）靠游标满足同一个契约——两个实现各自用自己平台最自然的
//! 手段兑现同一条排序承诺，不是巧合，是这个端口存在的意义。
//!
//! `Clone` 是浅克隆（`Arc` 计数 +1，共享同一份底层 `BTreeMap`）——特意这样设计：
//! 「同一个数据库被重新连接一次」在真实的 `web_sys::IdbDatabase` 那边靠的是同一个
//! 数据库名字，不是同一个 Rust 值；`MemoryKv::clone()` 是 native 测试里模拟这件事
//! 最直接的手段（见 `store.rs` 的重启回归测试：drop 掉第一个 `IdbStore` 之后，用
//! 同一个 `MemoryKv` 克隆出的句柄开第二个，断言历史没丢、`next_index` 没有把旧
//! journal 覆盖掉）。

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

use super::kv::{KvError, KvStore};

#[derive(Clone)]
pub struct MemoryKv {
    map: Arc<Mutex<BTreeMap<Vec<u8>, Vec<u8>>>>,
}

impl MemoryKv {
    pub fn new() -> Self {
        MemoryKv {
            map: Arc::new(Mutex::new(BTreeMap::new())),
        }
    }
}

impl Default for MemoryKv {
    fn default() -> Self {
        Self::new()
    }
}

/// 锁中毒（某次持锁时 panic）按「这个后端从今往后再也读不到／写不进任何东西」处理
/// ——跟 `agent_store::persist::Memory` 同一条取舍：这是测试用的假后端，唯一可能
/// 失败的地方就是自己的锁，不该比真正的后端更容易把上层带崩。
fn poisoned<T>(_: std::sync::PoisonError<T>) -> KvError {
    KvError {
        detail: "MemoryKv 的锁中毒了（某次持锁时 panic 过）".to_string(),
    }
}

impl KvStore for MemoryKv {
    async fn get(&self, key: &[u8]) -> Result<Option<Vec<u8>>, KvError> {
        let guard = self.map.lock().map_err(poisoned)?;
        Ok(guard.get(key).cloned())
    }

    async fn put(&self, key: &[u8], value: &[u8]) -> Result<(), KvError> {
        let mut guard = self.map.lock().map_err(poisoned)?;
        guard.insert(key.to_vec(), value.to_vec());
        Ok(())
    }

    async fn scan_prefix(&self, prefix: &[u8]) -> Result<Vec<(Vec<u8>, Vec<u8>)>, KvError> {
        let guard = self.map.lock().map_err(poisoned)?;
        Ok(guard
            .range(prefix.to_vec()..)
            .take_while(|(k, _)| k.starts_with(prefix))
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect())
    }
}

// native only：测试用 `super::blocking::run_to_completion` 驱动 async 方法，那个
// 模块本身只在非 wasm32 编译（见 `blocking.rs` 模块文档）。
#[cfg(all(test, not(target_arch = "wasm32")))]
mod tests {
    use super::*;
    use crate::persist::idb::blocking::run_to_completion;

    #[test]
    fn get_on_a_missing_key_is_none() {
        let kv = MemoryKv::new();
        assert_eq!(run_to_completion(kv.get(b"nope")).unwrap(), None);
    }

    #[test]
    fn put_then_get_round_trips() {
        let kv = MemoryKv::new();
        run_to_completion(kv.put(b"a", b"1")).unwrap();
        assert_eq!(run_to_completion(kv.get(b"a")).unwrap(), Some(b"1".to_vec()));
    }

    #[test]
    fn put_is_overwriting_not_appending() {
        let kv = MemoryKv::new();
        run_to_completion(kv.put(b"a", b"1")).unwrap();
        run_to_completion(kv.put(b"a", b"2")).unwrap();
        assert_eq!(run_to_completion(kv.get(b"a")).unwrap(), Some(b"2".to_vec()));
    }

    #[test]
    fn scan_prefix_only_returns_matching_keys_in_byte_order() {
        let kv = MemoryKv::new();
        for (k, v) in [("a/2", "two"), ("a/1", "one"), ("b/1", "other"), ("a/10", "ten")] {
            run_to_completion(kv.put(k.as_bytes(), v.as_bytes())).unwrap();
        }
        let rows = run_to_completion(kv.scan_prefix(b"a/")).unwrap();
        let keys: Vec<String> = rows
            .iter()
            .map(|(k, _)| String::from_utf8(k.clone()).unwrap())
            .collect();
        // 字节序，不是"直觉"里的数值序——"a/1" < "a/10" < "a/2"，这正是 record.rs
        // 用零填充而不是裸十进制字符串编码 index 的原因。
        assert_eq!(keys, vec!["a/1", "a/10", "a/2"]);
    }

    #[test]
    fn scan_prefix_on_an_empty_store_is_empty() {
        let kv = MemoryKv::new();
        assert!(run_to_completion(kv.scan_prefix(b"anything/")).unwrap().is_empty());
    }
}

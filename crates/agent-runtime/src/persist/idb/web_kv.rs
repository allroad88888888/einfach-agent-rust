//! [`IdbDatabaseKv`]：[`KvStore`] 真正的浏览器实现，`web_sys::IdbDatabase` 上薄薄
//! 一层——**只在 `wasm32` 编译**，也**只能在浏览器里验**（native 没有 IndexedDB，
//! 这个仓库的 `cargo test --workspace` 覆盖不到这个文件；wasm32 target 的
//! `cargo check` 能验证它跟 web-sys 的 API 对得上，但验证不了运行时行为——真机
//! 验收是 114 issue 本身的事，不是 114a 的范围）。
//!
//! ## 开库不是这里的事
//!
//! 构造函数只接一个**已经打开**的 `IdbDatabase` + 一个 object store 名字。
//! `indexedDB.open(name, version)`、`onupgradeneeded` 建 object store 这些是宿主
//! 装配的事（114c）——一个持久化后端不该替调用方决定数据库名字、版本号、什么时候
//! 要建库，那些是应用级的决策，端口只管「给我一个能用的 store，我用它实现三个
//! KV 操作」。
//!
//! ## key/value 都当 UTF-8 字符串存，不当二进制
//!
//! [`super::record::journal_key`] 产出的永远是合法 UTF-8（十进制数字 + `/`），
//! [`super::record::Record`] 序列化出来的永远是合法 UTF-8（`serde_json::to_vec`
//! 就是 JSON 文本）——这个模块的**唯一**调用方（[`super::replay`]/[`super::worker`]，
//! 都在 `idb` 模块内）从来不会喂真正的二进制数据进来。于是可以把 IndexedDB 的
//! key/value 都存成 JS 字符串：不用处理 `ArrayBuffer`/`Uint8Array` 键类型、不用
//! `IDBKeyRange` 的二进制比较语义，字符串前缀范围（`IdbKeyRange::bound(prefix,
//! prefix + '\u{ffff}')`）直接对应 [`super::record::journal_prefix`] 要的字节序
//! 前缀扫——这正是「薄到看一眼就知道对不对」的具体做法：牺牲通用性（这个实现假设
//! 所有 key/value 都是 UTF-8），换来完全不需要处理 IndexedDB 二进制键这一整类
//! 复杂度。如果 `KvStore` 未来有别的调用方需要塞真正的二进制 value，这个假设要
//! 重新评估，但那不是 114a 的范围。

use js_sys::{Array, Promise};
use wasm_bindgen::closure::Closure;
use wasm_bindgen::{JsCast, JsValue};
use web_sys::{IdbDatabase, IdbKeyRange, IdbRequest, IdbTransactionMode};

use super::kv::{KvError, KvStore};

pub struct IdbDatabaseKv {
    db: IdbDatabase,
    store_name: String,
}

impl IdbDatabaseKv {
    pub fn new(db: IdbDatabase, store_name: impl Into<String>) -> Self {
        IdbDatabaseKv {
            db,
            store_name: store_name.into(),
        }
    }

    fn transaction(&self, mode: IdbTransactionMode) -> Result<web_sys::IdbTransaction, KvError> {
        self.db
            .transaction_with_str_and_mode(&self.store_name, mode)
            .map_err(js_err)
    }

    fn object_store(&self, tx: &web_sys::IdbTransaction) -> Result<web_sys::IdbObjectStore, KvError> {
        tx.object_store(&self.store_name).map_err(js_err)
    }
}

fn js_err(e: JsValue) -> KvError {
    // 只转 IndexedDB 抛出的 DOMException/Error 的类别描述，不转任何调用参数——见
    // 模块文档同一条红线（`crate::jsonl::error` 那份注释原样适用：这里可能间接
    // 带着序列化后的 K/V，但 `js_err` 只吃事件/异常对象本身,不吃 key/value）。
    KvError {
        detail: format!("{e:?}"),
    }
}

fn to_js_string(bytes: &[u8]) -> Result<JsValue, KvError> {
    let s = std::str::from_utf8(bytes).map_err(|_| KvError {
        detail: "key/value 不是合法 UTF-8——违反了 web_kv 模块文档记的那条假设".to_string(),
    })?;
    Ok(JsValue::from_str(s))
}

fn from_js_string(value: &JsValue) -> Result<Vec<u8>, KvError> {
    value
        .as_string()
        .map(String::into_bytes)
        .ok_or_else(|| KvError {
            detail: "IndexedDB 返回的值不是字符串——违反了 web_kv 模块文档记的那条假设".to_string(),
        })
}

/// 把一个 `IdbRequest` 的完成事件桥接成一个 `Future`——`IDBRequest` 本身不是
/// `Promise`，标准做法是手写一个只会 resolve/reject 一次的 `Promise`，用
/// `onsuccess`/`onerror` 各挂一个只调用一次的闭包。
async fn request_result(req: &IdbRequest) -> Result<JsValue, KvError> {
    let req_ok = req.clone();
    let promise = Promise::new(&mut |resolve, reject| {
        let ok_req = req_ok.clone();
        let onsuccess = Closure::once(move |_evt: web_sys::Event| {
            let _ = resolve.call1(&JsValue::NULL, &ok_req.result().unwrap_or(JsValue::NULL));
        });
        req.set_onsuccess(Some(onsuccess.as_ref().unchecked_ref()));
        onsuccess.forget();

        let onerror = Closure::once(move |evt: web_sys::Event| {
            let _ = reject.call1(&JsValue::NULL, &evt);
        });
        req.set_onerror(Some(onerror.as_ref().unchecked_ref()));
        onerror.forget();
    });
    wasm_bindgen_futures::JsFuture::from(promise)
        .await
        .map_err(js_err)
}

/// 前缀扫描要用的 key range：`[prefix, prefix + 高位哨兵字符]`——字符串前缀范围
/// 的标准写法，`'\u{ffff}'` 大于任何一个真实 journal key 里会出现的字符
/// （`record.rs` 的编码只用十进制数字和 `/`）。
fn prefix_range(prefix: &[u8]) -> Result<IdbKeyRange, KvError> {
    let lower = to_js_string(prefix)?;
    let mut upper_s = String::from_utf8(prefix.to_vec()).map_err(|_| KvError {
        detail: "prefix 不是合法 UTF-8".to_string(),
    })?;
    upper_s.push('\u{ffff}');
    let upper = JsValue::from_str(&upper_s);
    IdbKeyRange::bound(&lower, &upper).map_err(js_err)
}

impl KvStore for IdbDatabaseKv {
    async fn get(&self, key: &[u8]) -> Result<Option<Vec<u8>>, KvError> {
        let tx = self.transaction(IdbTransactionMode::Readonly)?;
        let store = self.object_store(&tx)?;
        let key_js = to_js_string(key)?;
        let req = store.get(&key_js).map_err(js_err)?;
        let result = request_result(&req).await?;
        if result.is_undefined() || result.is_null() {
            Ok(None)
        } else {
            Ok(Some(from_js_string(&result)?))
        }
    }

    async fn put(&self, key: &[u8], value: &[u8]) -> Result<(), KvError> {
        let tx = self.transaction(IdbTransactionMode::Readwrite)?;
        let store = self.object_store(&tx)?;
        let key_js = to_js_string(key)?;
        let value_js = to_js_string(value)?;
        let req = store.put_with_key(&value_js, &key_js).map_err(js_err)?;
        request_result(&req).await?;
        Ok(())
    }

    async fn scan_prefix(&self, prefix: &[u8]) -> Result<Vec<(Vec<u8>, Vec<u8>)>, KvError> {
        let range = prefix_range(prefix)?;
        let tx = self.transaction(IdbTransactionMode::Readonly)?;
        let store = self.object_store(&tx)?;

        // `getAllKeys`/`getAll` 各一次请求就能拿到范围内的全部行，不需要手写游标
        // 循环——两次调用的返回顺序都保证跟 key 的排序一致（IndexedDB 规范），
        // 按下标 zip 就是对的 `(key, value)` 配对。
        let keys_req = store.get_all_keys_with_key(&range).map_err(js_err)?;
        let keys_val = request_result(&keys_req).await?;
        let values_req = store.get_all_with_key(&range).map_err(js_err)?;
        let values_val = request_result(&values_req).await?;

        let keys = Array::from(&keys_val);
        let values = Array::from(&values_val);
        let mut rows = Vec::with_capacity(keys.length() as usize);
        for i in 0..keys.length() {
            rows.push((from_js_string(&keys.get(i))?, from_js_string(&values.get(i))?));
        }
        Ok(rows)
    }
}

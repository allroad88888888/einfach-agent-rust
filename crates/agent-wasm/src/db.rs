//! 开 IndexedDB：**一个会话一个数据库**，库里一个叫 `journal` 的 object store。
//!
//! `agent_runtime::persist::idb` 的 `IdbDatabaseKv` 只接一个**已经打开**的
//! `IdbDatabase`——它的模块文档写明了理由：「一个持久化后端不该替调用方决定
//! 数据库名字、版本号、什么时候要建库，那些是应用级的决策」。这个文件就是那个
//! 应用级决策的落点（114a 说的「宿主装配的事（114c）」）。
//!
//! # 为什么一个会话一个库，而不是一个库里一个会话一张表
//!
//! IndexedDB 的 object store **只能在 `onupgradeneeded` 里建**，也就是只能靠
//! 提升数据库版本号来建。「一个库多张表」于是意味着每开一个新会话都要把整个
//! 数据库升一版——版本号会随会话数无限增长，而且同一个库的两个标签页会互相
//! 撞 `versionchange`。一个会话一个库把这件事整个消掉：版本号恒为 1，新会话
//! 就是新库。
//!
//! 库名是 `agent-session-<id>`。`<id>` 已经过 [`crate::session_id`] 的白名单
//! （`[A-Za-z0-9_-]`、≤128），所以拼进库名不需要任何转义。

use wasm_bindgen::closure::Closure;
use wasm_bindgen::{JsCast, JsValue};
use web_sys::{IdbDatabase, IdbOpenDbRequest, IdbRequest};

/// journal 落在哪张 object store 里。
pub(crate) const OBJECT_STORE: &str = "journal";

/// 打开（必要时创建）这个会话的数据库。
pub(crate) async fn open(session_id: &str) -> Result<IdbDatabase, String> {
    let factory = web_sys::window()
        .ok_or_else(|| "没有 window：这个宿主不是页面主线程".to_string())?
        .indexed_db()
        .map_err(describe)?
        .ok_or_else(|| "这个浏览器没有 IndexedDB（隐私模式？）".to_string())?;

    let request: IdbOpenDbRequest = factory
        .open_with_u32(&format!("agent-session-{session_id}"), 1)
        .map_err(describe)?;

    // 建库/升版本：只在库第一次被打开时触发。`Closure::once` 正好对上——这个
    // 回调一辈子最多被调一次。
    let on_upgrade = Closure::once(move |event: web_sys::Event| {
        create_store_if_missing(&event);
    });
    request.set_onupgradeneeded(Some(on_upgrade.as_ref().unchecked_ref()));

    let result = await_request(request.as_ref()).await;
    // 闭包活到这一刻为止：`onupgradeneeded` 只可能在 `success` 之前触发，
    // 到这里它已经不可能再被调用了，可以安全析构。
    drop(on_upgrade);

    result?
        .dyn_into::<IdbDatabase>()
        .map_err(|_| "indexedDB.open 的结果不是 IDBDatabase".to_string())
}

/// `onupgradeneeded` 回调体：库里没有 `journal` 就建一个。
///
/// 拿不到 event.target / result 时静默返回——那种情况下随后的 `open` 会以
/// 「没有这张 object store」的形式失败，错误报在那里比在这里更有信息量。
fn create_store_if_missing(event: &web_sys::Event) {
    let Some(target) = event.target() else { return };
    let Ok(request) = target.dyn_into::<IdbRequest>() else {
        return;
    };
    let Ok(result) = request.result() else { return };
    let Ok(db) = result.dyn_into::<IdbDatabase>() else {
        return;
    };
    if !db.object_store_names().contains(OBJECT_STORE) {
        let _ = db.create_object_store(OBJECT_STORE);
    }
}

/// 把一个 `IDBRequest` 的完成事件桥成 `Future`。跟
/// `agent_runtime::persist::idb::web_kv` 里那个同名手法是同一件事——`IDBRequest`
/// 不是 `Promise`，标准做法是手写一个只 resolve/reject 一次的 `Promise`。
///
/// 没有复用那一份：它是 `idb` 模块的私有实现细节，而这个文件在另一个 crate 里。
/// 为一个八行的桥接把私有函数公开出去，等于把「怎么桥」变成跨 crate 契约。
async fn await_request(request: &IdbRequest) -> Result<JsValue, String> {
    let request = request.clone();
    let promise = js_sys::Promise::new(&mut |resolve, reject| {
        let ok_request = request.clone();
        let on_success = Closure::once(move |_event: web_sys::Event| {
            let _ = resolve.call1(&JsValue::NULL, &ok_request.result().unwrap_or(JsValue::NULL));
        });
        request.set_onsuccess(Some(on_success.as_ref().unchecked_ref()));
        on_success.forget();

        let on_error = Closure::once(move |event: web_sys::Event| {
            let _ = reject.call1(&JsValue::NULL, &event);
        });
        request.set_onerror(Some(on_error.as_ref().unchecked_ref()));
        on_error.forget();
    });
    wasm_bindgen_futures::JsFuture::from(promise)
        .await
        .map_err(describe)
}

/// `JsValue` 没有统一的 `Display`。只转异常对象本身，**不转任何调用参数**
/// （`web_kv::js_err` 同一条红线：错误消息里不许出现 key/value 内容）。
fn describe(error: JsValue) -> String {
    if let Some(err) = error.dyn_ref::<js_sys::Error>() {
        return err.message().into();
    }
    format!("{error:?}")
}

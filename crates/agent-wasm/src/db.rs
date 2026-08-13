//! 开 / 删这个会话的 IndexedDB 库：**一个会话一个数据库**，库里两张 object store
//! ——`journal`（会话日志）与 `images`（图片字节）。
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
//! 撞 `versionchange`。一个会话一个库把这件事整个消掉：版本号只随 schema 走
//! （[`VERSION`]，128 之前是恒为 1），跟会话数无关，新会话就是新库。
//!
//! 库名是 `agent-session-<id>`（[`database_name`]，**只有那一处拼**）。`<id>` 已经过
//! [`crate::session_id`] 的白名单（`[A-Za-z0-9_-]`、≤128），所以拼进库名不需要任何转义。
//!
//! # 两张 store，两个所有者（128；分工来自 119 §四）
//!
//! | store | 谁建 | 谁读写 |
//! |---|---|---|
//! | `journal` | 本文件 | `agent-runtime` 的 `IdbDatabaseKv`（114a） |
//! | `images` | 本文件 | **页面自己的 JS**，Rust 一个字节都不碰 |
//!
//! 建表只能在 `onupgradeneeded` 里做，而 `open` 在 Rust 手上——所以 **schema 归
//! Rust，数据归页面**。图片**没有**走 `IdbDatabaseKv`：`web_kv.rs` 的模块文档把
//! 「key/value 都是 UTF-8 字符串」写成了那个实现的前提，而图片是真二进制。两张
//! 互不相干的 store 各按自己的形状存，`KvStore` 因此一个字节都不用动——`images`
//! 里直接放 `Blob`（IndexedDB 的结构化克隆原生支持），**不 base64**（+33% 体积，
//! 还把二进制混进字符串编码）。
//!
//! `images` 的主键走 in-line：建表时给了 `keyPath: "id"`（[`IMAGE_KEY_PATH`]），
//! 页面 `put(record)` 不用另传 key。记录里除 `id` 之外还有什么**由页面定**
//! （建议 `{ id, blob, mime, bytes, addedAt }`，归 129），本文件不认识那些字段。
//!
//! # 页面必须遵守的三条（违反的症状都不是当场报错）
//!
//! 1. 页面自己 `indexedDB.open('agent-session-<id>')` 时**不要带版本号**。不带 =
//!    「按当前版本打开，不触发升级」，版本号这件事于是完全由 Rust 一方拥有，
//!    两边不会互相升版本打架。
//! 2. **`openSession(id)` 必须先调过**，页面才能碰这个库。反过来的话，页面会先
//!    建出一个版本 1、一张 store 都没有的空库，随后 Rust 的 `open(…, 2)` 去升它
//!    ——升得动，但页面那一次读拿到的是「没有 `images` 这张 store」。症状是
//!    「第一次用好好的，某次刷新之后读不到了」，很难查，所以这是顺序约束不是建议。
//! 3. 调 `deleteSession(id)` 之前，页面要先把自己那条连接 `db.close()`——见
//!    [`delete`]：删库撞上开着的连接是**挂住**，不是报错。
//!
//! # 版本 1 → 2
//!
//! 版本 1 的库（M13 真机验收留下的那些）下次打开会走一次 `onupgradeneeded`。
//! [`create_missing_stores`] 只建缺的那张，不碰已有的 `journal`——升级不动任何一条
//! 已有数据。这条是 128 的验收主证据，而且**只有拿一个真的版本 1 的库才验得到**：
//! 新建的库直接建在版本 2 上，走的根本不是升级路径。

use wasm_bindgen::closure::Closure;
use wasm_bindgen::{JsCast, JsValue};
use web_sys::{IdbDatabase, IdbFactory, IdbObjectStoreParameters, IdbOpenDbRequest, IdbRequest};

/// journal 落在哪张 object store 里。
pub(crate) const OBJECT_STORE: &str = "journal";

/// 图片落在哪张 object store 里。**Rust 侧只建它，不读它、不写它**（见模块文档）。
const IMAGE_STORE: &str = "images";

/// `images` 的 in-line 主键字段名。页面那份记录里必须有它。
const IMAGE_KEY_PATH: &str = "id";

/// schema 版本：1 = 只有 `journal`（M13 及之前留下的库都在这一版）；
/// 2 = 多一张 `images`（128）。**只随 schema 变**，不随会话数变。
const VERSION: u32 = 2;

/// 库名。开和删必须拼出同一个名字，所以只有这一处拼。
fn database_name(session_id: &str) -> String {
    format!("agent-session-{session_id}")
}

/// 打开（必要时创建 / 升级）这个会话的数据库。
pub(crate) async fn open(session_id: &str) -> Result<IdbDatabase, String> {
    let request: IdbOpenDbRequest = factory()?
        .open_with_u32(&database_name(session_id), VERSION)
        .map_err(describe)?;

    // 建库/升版本：只在版本号对不上时触发。`Closure::once` 正好对上——这个
    // 回调一辈子最多被调一次。
    let on_upgrade = Closure::once(move |event: web_sys::Event| {
        create_missing_stores(&event);
    });
    request.set_onupgradeneeded(Some(on_upgrade.as_ref().unchecked_ref()));

    let result = await_open_request(&request, BLOCKED_BY_OPEN).await;
    // 闭包活到这一刻为止：`onupgradeneeded` 只可能在 `success` 之前触发，
    // 到这里它已经不可能再被调用了，可以安全析构。
    drop(on_upgrade);

    let database = result?
        .dyn_into::<IdbDatabase>()
        .map_err(|_| "indexedDB.open 的结果不是 IDBDatabase".to_string())?;
    close_on_versionchange(&database);
    Ok(database)
}

/// 删掉这个会话的**整个库**：journal 与图片一起没（119 §五-3 选「同一个库」换来
/// 的正是这个——不需要记着删两个地方）。
///
/// # `deleteDatabase` 撞上开着的连接是「挂住」，不是「报错」
///
/// 三种连接，三种处置：
///
/// | 谁开着 | 怎么办 |
/// |---|---|
/// | Rust 自己那条（`assemble::Live` 手上） | 调用方先把 `live` 放掉（`AgentHost::delete_session`）；就算没放掉，[`close_on_versionchange`] 也会让它自己关 |
/// | **页面自己那条** | 关不了，它在 JS 那边——页面必须先 `db.close()`（模块文档第 3 条） |
/// | 别的标签页 | 本版代码开的连接自己会关；老代码开的关不掉 |
///
/// 后两种就是 `onblocked`，这里**一律 reject**：页面上那个删除按钮宁可报一句
/// 「有别的连接开着」，也不能永远转圈且没有任何错误信息。
///
/// ⚠️ reject **不等于「没删」**：被 blocked 的删除请求在浏览器里仍然挂着，等最后
/// 一条连接关掉（比如那个标签页被关了）它照样会把库删掉。所以这条错误的准确含义是
/// 「现在没删成，去关掉别的连接再来」，不是「什么都没发生」。
pub(crate) async fn delete(session_id: &str) -> Result<(), String> {
    let request = factory()?
        .delete_database(&database_name(session_id))
        .map_err(describe)?;
    await_open_request(&request, BLOCKED_BY_DELETE).await?;
    Ok(())
}

const BLOCKED_BY_DELETE: &str = "删不掉：这个会话的库还有别的连接开着（页面自己那条要先 db.close()，别的标签页要先关掉）。\
     注意删除请求还挂在浏览器里，等最后一条连接关掉它仍会生效。";

const BLOCKED_BY_OPEN: &str =
    "打不开：这个库正被别的标签页用旧版本占着，升不上去。关掉其它标签页再试。";

/// `window.indexedDB`。开和删都要它。
fn factory() -> Result<IdbFactory, String> {
    web_sys::window()
        .ok_or_else(|| "没有 window：这个宿主不是页面主线程".to_string())?
        .indexed_db()
        .map_err(describe)?
        .ok_or_else(|| "这个浏览器没有 IndexedDB（隐私模式？）".to_string())
}

/// `onupgradeneeded` 回调体：**缺哪张建哪张**，已有的一张都不碰——版本 1 的库
/// 走到这里时 `journal` 已经在了，只会多出一张 `images`，里面的数据一条不动。
///
/// 拿不到 event.target / result 时静默返回——那种情况下随后的 `open` 会以
/// 「没有这张 object store」的形式失败，错误报在那里比在这里更有信息量。
fn create_missing_stores(event: &web_sys::Event) {
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
    if !db.object_store_names().contains(IMAGE_STORE) {
        // in-line key：记录自己带着 `id`，页面不用在 `put` 时另传一个 key。
        let params = IdbObjectStoreParameters::new();
        params.set_key_path_opt_str(Some(IMAGE_KEY_PATH));
        let _ = db.create_object_store_with_optional_parameters(IMAGE_STORE, &params);
    }
}

/// 给刚开出来的连接挂上 IndexedDB 那条标准自保：**收到 `versionchange` 就自己
/// `close()`**。
///
/// 没有它，[`delete`] 会被 Rust 自己这条连接挂住，而 Rust 这边并没有「该关了」
/// 的时机可用：`IdbDatabase` 的所有权在 `assemble::Live` 里，`Live` 被 drop 时
/// 只是把 JS 那侧的引用放掉，连接要等 GC 真的回收那个对象才关——GC 什么时候来
/// 没有任何保证。挂上这个回调之后「关」变成连接自己的事，删库那边就只剩下页面
/// 自己那条连接需要操心。
///
/// 代价说清楚：连接一关，它背后那个会话此后写不进去了（错误走 `store_error`
/// 出口）。这正是删库时该有的行为——删的就是它。
///
/// 闭包**不捕获** `db`（要用时从 `event.target()` 现取），所以 `forget()` 泄掉的
/// 只是闭包本身那几十字节，不会反过来把连接对象钉住、让它永远 GC 不掉。
fn close_on_versionchange(db: &IdbDatabase) {
    let on_version_change = Closure::wrap(Box::new(|event: web_sys::Event| {
        let Some(target) = event.target() else { return };
        if let Ok(db) = target.dyn_into::<IdbDatabase>() {
            db.close();
        }
    }) as Box<dyn FnMut(web_sys::Event)>);
    db.set_onversionchange(Some(on_version_change.as_ref().unchecked_ref()));
    on_version_change.forget();
}

/// 把一个 `IDBOpenDBRequest` 的完成事件桥成 `Future`。跟
/// `agent_runtime::persist::idb::web_kv` 里那个同名手法是同一件事——`IDBRequest`
/// 不是 `Promise`，标准做法是手写一个只 resolve/reject 一次的 `Promise`。
///
/// 没有复用那一份：它是 `idb` 模块的私有实现细节，而这个文件在另一个 crate 里。
/// 为一个八行的桥接把私有函数公开出去，等于把「怎么桥」变成跨 crate 契约。
///
/// **比那一份多一个 `onblocked`**：`open`（升版本时）和 `deleteDatabase` 都可能
/// 被别的连接挡住，而挡住的表现是「这三个回调一个都不来」。不接它就是一个永远
/// 不 settle 的 Promise。
async fn await_open_request(
    request: &IdbOpenDbRequest,
    blocked: &'static str,
) -> Result<JsValue, String> {
    let request = request.clone();
    let promise = js_sys::Promise::new(&mut |resolve, reject| {
        let ok_request = request.clone();
        let on_success = Closure::once(move |_event: web_sys::Event| {
            let _ = resolve.call1(
                &JsValue::NULL,
                &ok_request.result().unwrap_or(JsValue::NULL),
            );
        });
        request.set_onsuccess(Some(on_success.as_ref().unchecked_ref()));
        on_success.forget();

        let error_reject = reject.clone();
        let on_error = Closure::once(move |event: web_sys::Event| {
            let _ = error_reject.call1(&JsValue::NULL, &event);
        });
        request.set_onerror(Some(on_error.as_ref().unchecked_ref()));
        on_error.forget();

        let on_blocked = Closure::once(move |_event: web_sys::Event| {
            let _ = reject.call1(&JsValue::NULL, &js_sys::Error::new(blocked));
        });
        request.set_onblocked(Some(on_blocked.as_ref().unchecked_ref()));
        on_blocked.forget();
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

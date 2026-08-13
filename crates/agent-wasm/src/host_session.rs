//! `AgentHost` 上**碰 `live` 的那一面**：开会话、说一句话、取消、查身份/历史、
//! 删会话。跟 [`crate::host`] 的分家线就是这一句——那边的入口一个都不碰 `live`。
//!
//! # 借用纪律（这个 crate 唯一真正微妙的地方）
//!
//! `run_turn` 要 `&mut Session` + `&mut RunnerCtx`，而它是一条很长的 await 链。
//! 于是 [`AgentHost::send`] 会**在整轮对话期间**持有 `live` 的可变借用。取消必须在
//! 这期间生效，所以取消标志（`Arc<AtomicBool>`，`RunnerCtx::cancel_flag()` 给的那
//! 一份）单独放在另一个 `RefCell` 里：`cancel()` 只碰它，碰不到 `live`。
//!
//! 由此得到三条对页面的约定，**违反前两条会 panic 而不是静默出错**（这是好事，
//! 静默的重入会变成一个查不出来的状态错乱）：
//!
//! 1. 上一轮 `send()` 的 Promise 没 settle 之前，不要再调 `send()`/`open_session()`
//!    ——`session_id()`/`history_json()` 也一样，它们要的共享借用同样借不到。
//! 2. **任何回调里**都不要回头调上面那几个。事件回调只读、只画，本来就不该干活；
//!    工具回调（[`AgentHost::on_tool_call`]）**天然要干活**，但它能干的是「不经过
//!    这个 `AgentHost` 的活」——完整清单与理由在 `on_tool_call` 的文档注释里，
//!    那是第一个用它的人一定会读的地方。
//! 3. [`AgentHost::delete_session`] 也碰 `live`，但它用 `try_borrow_mut` 把「撞上在
//!    飞的一轮」变成一次 **reject 而不是 panic**——那是个破坏性操作，页面上那个按钮
//!    随时可能被按到，收一句「这一轮还在飞」比整个 wasm 实例 panic 掉强。**这个手法
//!    只对从借用外面打进来的调用成立**，对回调里的重入不成立：见 `on_tool_call`。

use std::rc::Rc;
use std::sync::atomic::Ordering;

use agent_core::Session;
use agent_runtime::RunnerCtx;
use wasm_bindgen::prelude::*;

use crate::callback;
use crate::host::{AgentHost, js_error};
use crate::{assemble, db, history, session_id, turn, undo};

#[wasm_bindgen]
impl AgentHost {
    /// 当前打开的会话 id。没开就是 `undefined`。
    #[wasm_bindgen(js_name = sessionId)]
    pub fn session_id(&self) -> Option<String> {
        self.inner
            .live
            .borrow()
            .as_ref()
            .map(|live| live.id.clone())
    }

    /// 开一个会话（同一个 id 再开一次 = 从 IndexedDB 把它接回来；换个 id =
    /// 切会话）。Promise 结果是**重放出来的历史** JSON，页面据此重画。
    #[wasm_bindgen(js_name = openSession)]
    pub fn open_session(&self, id: String) -> js_sys::Promise {
        let inner = Rc::clone(&self.inner);
        wasm_bindgen_futures::future_to_promise(async move {
            let id = session_id::validate(&id).map_err(js_error)?.to_string();
            let live = assemble::open(
                id,
                &inner.config,
                callback::event_sink(Rc::clone(&inner.on_event)),
                callback::store_error_sink(Rc::clone(&inner.on_event)),
            )
            .await
            .map_err(js_error)?;
            *inner.cancel.borrow_mut() = Some(live.ctx.cancel_flag());
            let replayed = history::to_json(&live.session);
            *inner.live.borrow_mut() = Some(live);
            Ok(JsValue::from_str(&replayed))
        })
    }

    /// 删掉一个会话：**journal 与图片一起没**。
    ///
    /// 删的是整个库（`agent-session-<id>`），所以图片不需要单独清——这正是 119
    /// §五-3 选「同一个库」换来的东西。schema 与连接管理的细节在 [`crate::db`]。
    ///
    /// 页面必须知道的三条：
    ///
    /// 1. **删当前打开的这个会话是允许的**，代价是它当场被关掉：`sessionId()`
    ///    变回 `undefined`，`send()` 会开始报「还没打开会话」，接下来开哪个由页面
    ///    决定。选「关掉」而不是「拒绝」的理由：这个宿主没有别的关会话的入口，
    ///    拒绝就等于「你正在看的这个会话永远删不掉」。
    /// 2. **页面自己那条 IndexedDB 连接要先 `db.close()`**（[`crate::db`] 模块文档
    ///    第 3 条）。没关的话这次调用 **reject**，不是挂住；错误的含义是「现在没
    ///    删成」，不是「什么都没发生」——见 [`crate::db::delete`]。
    /// 3. 这一轮对话还在飞的时候调它会 reject（`live` 正被 `send()` 借着）。
    ///    先 `cancel()`，等 `send()` 的 Promise settle 再删。
    ///
    /// 成功时 Promise 结果是 `undefined`。
    #[wasm_bindgen(js_name = deleteSession)]
    pub fn delete_session(&self, id: String) -> js_sys::Promise {
        let inner = Rc::clone(&self.inner);
        wasm_bindgen_futures::future_to_promise(async move {
            let id = session_id::validate(&id).map_err(js_error)?.to_string();
            // 先放掉 Rust 这边可能持有的那条连接，再去删。顺序反了就是自己把自己
            // 挡住（`onblocked`）。借用在这个块里开始、在这个块里结束——**不跨
            // `await`**，那正是模块文档那条借用纪律说的事。
            {
                let mut guard = inner.live.try_borrow_mut().map_err(|_| {
                    js_error("这一轮还在飞：先 cancel()，等 send() 的 Promise settle 再删")
                })?;
                if guard.as_ref().is_some_and(|live| live.id == id) {
                    *guard = None;
                    *inner.cancel.borrow_mut() = None;
                }
            }
            db::delete(&id).await.map_err(js_error)?;
            Ok(JsValue::UNDEFINED)
        })
    }

    /// 说一句话，跑到这一轮结束。Promise 结果是一份 JSON：
    /// `{"status":"…","cancelledTurn":"…"|null}`；流式增量走事件回调，不走这个
    /// 返回值。`cancelledTurn` 只在这一轮被取消时非空，说的是「被丢弃的半轮
    /// 到底丢没丢干净」（撞上不可逆屏障时会留下，用户该知道）。
    ///
    /// 下面那句 `borrow_mut()` 就是模块文档整节纪律的源头：它活到这个 async 块
    /// 结束为止，**跨过了中间每一个 await 点**——包括工具回调那次。
    pub fn send(&self, text: String) -> js_sys::Promise {
        let inner = Rc::clone(&self.inner);
        wasm_bindgen_futures::future_to_promise(async move {
            let mut guard = inner.live.borrow_mut();
            let live = guard
                .as_mut()
                .ok_or_else(|| js_error("还没打开会话：先调 openSession(id)"))?;
            // `Err` 是 M12 的 transient-source 出口。**124 之后它不再是不可达的**
            // ——工具表里有了 `web:source/` 前缀的工具（见 `crate::tools`），
            // 这条从没在浏览器里跑过的路第一次真的会亮。真出现就是一条给页面的
            // 错误，不是一个假的终态——`js_error` 会让那个 Promise reject。
            // （这里原本写着「这个宿主的工具表里结构上不可达」，那是 114 时的
            // 事实，124 推翻了它。）
            let outcome = turn::run(&mut live.session, &mut live.ctx, &text)
                .await
                .map_err(|failure| js_error(&format!("{failure:?}")))?;
            let payload = serde_json::json!({
                "status": format!("{:?}", outcome.status),
                "cancelledTurn": outcome.cancelled_turn.map(|report| format!("{report:?}")),
            });
            Ok(JsValue::from_str(&payload.to_string()))
        })
    }

    /// 取消正在飞的这一轮。**不碰 `live`**，所以可以在 `send()` 的 Promise 还
    /// 没 settle 时调（那正是它唯一有意义的时机），工具回调里调它也安全。
    ///
    /// 123：**工具执行期间也算数**。翻完标志顺手叫醒那一轮——这一轮此刻可能正停在
    /// 页面回调那个 Promise 上，单线程里没有第二个人会替它发现标志变了，不叫就得等
    /// 到那个 Promise 自己 settle（永不 settle 的话就是永远）。醒来之后这一轮走取消
    /// 轮丢弃，被丢下的那次回调照样跑完，但它的结果再也回不到状态里
    /// （[`crate::interrupt`] 模块文档第三节）。
    pub fn cancel(&self) {
        if let Some(flag) = self.inner.cancel.borrow().as_ref() {
            flag.store(true, Ordering::Relaxed);
            crate::interrupt::wake();
        }
    }

    /// 撤一整轮（196）。返回一份 JSON：`{"kind":"Applied","entries":n,"turnId":n}` /
    /// `{"kind":"Blocked",…,"barrier":{…}}` / `{"kind":"Nothing"}`，形状与理由见
    /// [`crate::undo`]。
    ///
    /// **同步方法**：`undo_turn` 不 await 任何东西，`borrow_mut()` 不跨 await 点，
    /// 所以不像 [`Self::send`] 那样要背模块文档那节纪律。
    #[wasm_bindgen(js_name = undoTurn)]
    pub fn undo_turn(&self) -> Result<String, JsValue> {
        self.with_live(undo::undo)
    }

    /// 越过第一条不可逆屏障再撤（196）。只在页面**拿到 `Blocked` 并让用户确认之后**
    /// 才该调——越过屏障意味着那次不可逆副作用不会被回滚，这是用户的决定不是默认档。
    #[wasm_bindgen(js_name = undoTurnForce)]
    pub fn undo_turn_force(&self) -> Result<String, JsValue> {
        self.with_live(undo::undo_force)
    }

    /// 反演一次撤销（196）。结果只会是 `Applied`/`Nothing`——redo 没有屏障。
    #[wasm_bindgen(js_name = redoTurn)]
    pub fn redo_turn(&self) -> Result<String, JsValue> {
        self.with_live(undo::redo)
    }

    /// 三个撤销命令共用的一句：取出活会话、把 `&mut Session` + `&mut RunnerCtx`
    /// 递进去。没打开会话时给一条跟 [`Self::send`] 一字不差的错误。
    fn with_live(&self, f: fn(&mut Session, &mut RunnerCtx) -> String) -> Result<String, JsValue> {
        let mut guard = self.inner.live.borrow_mut();
        let live = guard
            .as_mut()
            .ok_or_else(|| js_error("还没打开会话：先调 openSession(id)"))?;
        Ok(f(&mut live.session, &mut live.ctx))
    }

    /// 当前会话的历史，形状同 `openSession` 的结果。
    #[wasm_bindgen(js_name = historyJson)]
    pub fn history_json(&self) -> String {
        match self.inner.live.borrow().as_ref() {
            Some(live) => history::to_json(&live.session),
            None => "[]".to_string(),
        }
    }
}

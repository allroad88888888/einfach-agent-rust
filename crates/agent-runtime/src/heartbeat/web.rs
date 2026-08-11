//! 心跳的 **wasm32 实现**：`setInterval` + `clearInterval`。契约与「为什么泵需要
//! 心跳」见 [`super`] 的模块文档，这里只说这一份实现自己的取舍。
//!
//! # 为什么不是线程
//!
//! `wasm32-unknown-unknown`（默认剖面，无 `+atomics`/`SharedArrayBuffer`）上
//! `std::thread::spawn` **编得过、一调就 trap**（113 实做记录里已实测）。浏览器
//! 里「隔一段时间叫我一次」本来就是事件循环的原生设施，所以这份实现比 native
//! 那份还薄：一个 `setInterval` 回调，回调体里做的事跟 native 后台线程循环体里
//! 那三行完全一样——取出登记着的 waker、叫醒它。
//!
//! # 为什么用反射取 `setInterval` 而不是 `web_sys::Window`
//!
//! 跟 `agent_transport::js_timer` 同一个理由：同一份代码要能在主线程与 Worker
//! 里都拿到定时器，两者的共同点只有「全局作用域上有这个函数」——`window` 在
//! Worker 里根本不存在。反射取全局属性因此比 `web_sys::window().unwrap()` 更
//! 诚实，也省掉一个只为这一处而开的 web-sys feature。
//!
//! # `Rc<RefCell<_>>` 而不是 `Arc<Mutex<_>>`
//!
//! wasm 默认单线程：waker 槽位的写入方（泵每次 poll 调 `register`）与读取方
//! （定时器回调）跑在同一条线程上，不可能真的并发。用 `Mutex` 只会在这条路径
//! 上多一次原子操作，还会让「这里有并发」这个错误印象留在代码里。
//!
//! # 拿不到定时器怎么办：退化成「没有心跳」，不是 panic
//!
//! `setInterval` 取不到（非常规宿主）时这份实现不装定时器、也不报错。后果是泵
//! 只在真有 IO 消息时才醒——截止线扫描与取消轮询会迟钝，但对话本身照跑。在一个
//! 连定时器都没有的宿主里 panic 掉整个页面，比这个后果更坏。

use std::cell::RefCell;
use std::rc::Rc;
use std::task::Waker;
use std::time::Duration;

use wasm_bindgen::closure::Closure;
use wasm_bindgen::{JsCast, JsValue};

/// 一条心跳。**随创建者一起活、一起死**：`Drop` 里 `clearInterval`。
pub(crate) struct Heartbeat {
    /// 当前该叫醒谁。定时器回调与 `register` 都碰它，见模块文档。
    waker: Rc<RefCell<Option<Waker>>>,
    /// `None` = 这个宿主没给我们定时器（见模块文档最后一节）。**只被 `Drop`
    /// 读**——握着它就是「心跳随创建者一起死」这条契约本身。
    _timer: Option<Interval>,
}

impl Heartbeat {
    pub(crate) fn start(interval: Duration) -> Self {
        let waker: Rc<RefCell<Option<Waker>>> = Rc::new(RefCell::new(None));
        let tick = Rc::clone(&waker);
        let closure = Closure::<dyn FnMut()>::new(move || {
            // 先把 waker 取出来（结束这次借用）再叫，不要在持有 `RefCell` 借用
            // 的情况下调外部代码——`wake()` 可能同步 poll 回泵，泵会 `register`
            // 一次，那就是一次 `already borrowed` panic。native 那份用「不拿着
            // 锁调外部代码」表达的是同一条纪律。
            let waker = tick.borrow().clone();
            if let Some(waker) = waker {
                waker.wake();
            }
        });
        let timer = Interval::start(closure, interval);
        Heartbeat {
            waker,
            _timer: timer,
        }
    }

    /// 登记「下一次心跳叫醒我」。每次 poll 都调一次：执行器换了 waker 也不会
    /// 漏掉唤醒。
    pub(crate) fn register(&self, waker: &Waker) {
        let mut slot = self.waker.borrow_mut();
        if !slot.as_ref().is_some_and(|known| known.will_wake(waker)) {
            *slot = Some(waker.clone());
        }
    }
}

/// 一个已经装好的 `setInterval`，**握着它的回调闭包**——闭包一旦被丢掉，JS 那边
/// 再触发就是调用一块已经释放的内存。所以两者必须同生共死，`Drop` 里先
/// `clearInterval` 再让闭包随结构体一起析构。
struct Interval {
    id: f64,
    _closure: Closure<dyn FnMut()>,
}

impl Interval {
    /// **吃掉**闭包的所有权：装成功了它随 `Interval` 活着，装不成就在这个函数
    /// 结束时随参数一起析构（那时 JS 那边并没有拿到它的引用，析构是安全的）。
    fn start(closure: Closure<dyn FnMut()>, interval: Duration) -> Option<Self> {
        let set_interval = global_function("setInterval")?;
        let id = set_interval
            .call2(
                &js_sys::global(),
                closure.as_ref().unchecked_ref::<js_sys::Function>(),
                &JsValue::from_f64(interval.as_millis() as f64),
            )
            .ok()?
            .as_f64()?;
        Some(Interval {
            id,
            _closure: closure,
        })
    }
}

impl Drop for Interval {
    fn drop(&mut self) {
        if let Some(clear_interval) = global_function("clearInterval") {
            let _ = clear_interval.call1(&js_sys::global(), &JsValue::from_f64(self.id));
        }
    }
}

/// 全局作用域上的一个函数（`setInterval`/`clearInterval`）。见模块文档
/// 「为什么用反射」。
fn global_function(name: &str) -> Option<js_sys::Function> {
    js_sys::Reflect::get(&js_sys::global(), &JsValue::from_str(name))
        .ok()?
        .dyn_into::<js_sys::Function>()
        .ok()
}

//! 缓存兜底三层的**判读**（issue 024）。
//!
//! 前缀缓存失效不报错、不影响任何功能，只是每一轮都全价——最贵的一家 120 倍。
//! 一个「一切正常」的 bug 跑一个月，代价是四位数。这个模块存在的全部理由是
//! **当场知道**，而不是月底看账单才知道；它同时是红线 11 唯一的运行期实检手段。
//!
//! # 判断归 adapter，比对归 core（红线 12）
//!
//! 「这次该命中多少 token」要看匹配语义和块粒度，「哪一段漂了」要知道料被摆成了
//! 什么顺序——两件都是模型相关的判断，adapter 做。**这里只有减法、整数百分比和
//! 一个按轮计数的窗口**，零个模型相关判断，也拿不到做那种判断的料。
//!
//! # 三层，按「多快发现」排，不是按「多准」排
//!
//! | 层 | 什么时候 | 花多少钱才发现 | 入口 | 抓什么 |
//! |---|---|---|---|---|
//! | 1 | 请求发出**之前** | 0 | [`check_drift`] | 我们自己的序列化 bug |
//! | 2 | 响应回来 | 一轮 | [`reconcile()`] | 我们对这家缓存语义理解错了 |
//! | 3 | 攒够几轮 | 若干轮 | [`check_window`] | 前两层都漏掉的慢性失效 |
//!
//! 三层的产出是**三个不同的类型**，共存在一个 [`GuardReport`] 里；告警是
//! [`GuardAlert`] 的三个变体。不是同一个布尔——混成一个信号，报警时就分不出
//! 该查哪一头，而这三头的处理方式完全不同。
//!
//! # 没有「一次算完三层」的函数
//!
//! 故意的。第 1 层的输入在**请求发出之前**就齐了，给一个 `evaluate(...)` 就等于
//! 把它挪到花完钱之后跑，那这一层白做。宿主的顺序是：
//!
//! ```text
//! encode → check_drift → （发请求）→ 响应 → reconcile + check_window → GuardReport
//! ```
//!
//! ```
//! use agent_core::cache;
//! use agent_core::TokenUsage;
//!
//! // 发出去之前：adapter 报 drift，core 归类。本轮没打算改前缀。
//! let drift = cache::check_drift(None, cache::PrefixIntent::Reuse);
//! assert_eq!(drift, cache::DriftVerdict::Clean);
//!
//! // 响应回来：对账 + 滚动窗口。
//! let usage = TokenUsage { prompt: 2432, completion: 88, cached: Some(512) };
//! let reconcile = cache::reconcile(512, usage.cached, cache::ReconcileParams::default());
//! let mut history = vec![cache::TurnHit::Observed { prompt: 2000, cached: 1900 }];
//! history.push(cache::TurnHit::from_usage(&usage));
//! let window = cache::check_window(&history, cache::WindowParams::default());
//!
//! let report = cache::GuardReport { drift, reconcile, window };
//! assert!(!report.has_alert());
//! ```
//!
//! # 纯函数（红线 1）
//!
//! 全部判读函数不读时钟、不取随机、不做 IO，也没有可变状态：窗口是**参数**不是
//! 累加器。第 3 层的「最近 N 轮」按**轮次**计数不按时间，于是同一份历史重放两次
//! 一定得出同一个告警——重放对得上，是崩溃恢复和审计回放的前提。
//!
//! # `None` 与 `Some(0)`：失明不是 0
//!
//! 有一家未命中时 `cached` 字段整个缺失，另两家给显式的 `0`
//! （[`crate::TokenUsage::cached`] 的注释）。前者是「这家不报」，后者是
//! 「真的没命中」。**混成一回事，第 2 层就永远对不上账，而且是往「缓存全崩」
//! 的方向错。** 所以失明轮在第 2 层是 [`ReconcileVerdict::Blind`]，在第 3 层
//! 干脆不进窗口——既不算好也不算坏，只能不算。
//!
//! # 已知局限
//!
//! - **第 2 层只跟上一次比**（ROADMAP §四）：只留一个前缀镜像，就判不出
//!   「跟三轮前那个分支相比」。留最近 N 个镜像更准，等真遇到再加。
//! - **第 3 层会在小 prompt 上误报**：prompt 很小的时候有的家干脆不缓存，
//!   连着三轮都很小就会告警。那几轮本来就便宜，是噪声不是错——真被烦到了再加
//!   「最小 prompt 门槛」，但那个门槛的取值只有 adapter 说得清，不该由 core 定。

pub mod drift;
pub mod reconcile;
pub mod report;
pub mod window;

pub use drift::{DriftVerdict, PrefixIntent, check_drift};
pub use reconcile::{
    DEFAULT_SHORTFALL_ALERT_PERCENT, DEFAULT_TOLERANCE_TOKENS, ReconcileParams, ReconcileVerdict,
    reconcile,
};
pub use report::{GuardAlert, GuardLayer, GuardReport};
pub use window::{
    DEFAULT_CONSECUTIVE_ALERT, DEFAULT_LOW_HIT_PERCENT, DEFAULT_WINDOW, TurnHit, WindowParams,
    WindowVerdict, check_window,
};

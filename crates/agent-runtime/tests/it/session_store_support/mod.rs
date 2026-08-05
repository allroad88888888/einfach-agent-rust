//! `Jsonl` 集成测试共用的三件小事：临时文件路径、收集 `on_error` 的回调、以及一个
//! 满足 `Jsonl<K, V, M>` 全部 trait bound（`Clone + Serialize + DeserializeOwned +
//! Send + 'static`）的最小值类型——跟 `agent-cli/tests/support/mod.rs::temp_dir` 同一个
//! 「每个用例一个独立临时路径，不清理，OS/CI 自行回收」的取舍。
#![allow(dead_code)]

use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use agent_store::AtomValue;
use serde::{Deserialize, Serialize};

use agent_runtime::SessionStoreError;

/// 每个用例一个独立的 `.jsonl` 路径（文件不预先创建——`Jsonl::new` 自己按需建）。
pub fn temp_path(name: &str) -> PathBuf {
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let n = COUNTER.fetch_add(1, Ordering::Relaxed);
    std::env::temp_dir().join(format!(
        "agent-runtime-it-session-store-{name}-{}-{n}.jsonl",
        std::process::id()
    ))
}

pub type Errors = Arc<Mutex<Vec<SessionStoreError>>>;

/// 收集所有 `on_error` 调用，供测试断言「至少报过一次」/「报的是哪一类」。
pub fn collecting_on_error() -> (Errors, impl Fn(SessionStoreError) + Send + Sync + 'static) {
    let errors: Errors = Arc::new(Mutex::new(Vec::new()));
    let sink = errors.clone();
    (errors, move |e: SessionStoreError| {
        sink.lock().unwrap().push(e)
    })
}

/// 最小值类型：`i64` 包一层新类型——`AtomValue` 是外部 trait，`tests/` 下每个文件都是
/// 独立 crate，孤儿规则不让直接 `impl AtomValue for i64`。派生 `Serialize`/
/// `Deserialize` 是 `Jsonl` 落盘要求的，其余测试文件（`Memory`）不需要但派生不影响它们。
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct Val(pub i64);

impl AtomValue for Val {
    fn null() -> Self {
        Val(0)
    }
}

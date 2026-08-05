//! 独立测试 agent（issue 027 黑盒验收）的共享测试基础设施门面。
//!
//! 每个 `indep_*.rs` 都是独立编译的集成测试二进制，各自 `mod indep_support;`
//! 引用这里；单个测试文件用不到的 `pub` 项在那个二进制里天然构成
//! “未使用”，因此整个子树统一 `allow(dead_code)`——这是共享测试夹具在
//! “每个二进制各自只用一部分” 场景下的标准写法，不代表真的有死代码。
#![allow(dead_code)]

pub mod fake_server;
pub mod process;
pub mod scratch;
pub mod sse;

pub use fake_server::{FakeServer, Script};
pub use process::CliProcess;
pub use scratch::Scratch;

use std::time::Duration;

/// 子进程 + 编译产物级别的超时宽容度（任务书原话：单测 <30s 级）。
pub const DEFAULT_TIMEOUT: Duration = Duration::from_secs(10);

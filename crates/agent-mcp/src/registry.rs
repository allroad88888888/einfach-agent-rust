//! `McpRegistry`：进程内、store 外的表，server id → 活的 `McpClient`（红线 3）。
//! `atom`/快照里只有 server 的配置与逻辑标识（id/命令行/可用性）——全部可序列化；
//! **这张表本身不 derive `Serialize`，也不会被塞进任何 atom**：`agent-core`
//! 压根不依赖 `agent-mcp`（见本 crate 的 `Cargo.toml` 依赖方向），类型层面就够不
//! 着 `McpClient`/`McpRegistry`——不是「没写进去」而是「写不进去」。结构性证明
//! 见 `tests/registry_not_in_snapshot_042.rs`。
//!
//! **崩溃恢复**：从配置重连，不从快照复活句柄——句柄本来就是进程局部的
//! （docs/MCP.md §「活句柄住 store 外」）。
//!
//! # 两层锁：要串行的是「同一个 server」，不是「所有 server」（issue 070）
//!
//! 表一把锁、每个 client 各一把锁。[`McpRegistry::with_client`] 先用表锁**只做一次
//! 查表 + 克隆 `Arc`** 就放手，阻塞式 JSON-RPC 往返在**那个 client 自己的锁**上跑。
//! 于是调 server `a` 不再挡住并发调 server `b`——M8 的 `spawn(background)` 让多个子
//! agent 真并发跑，它们各自调 MCP 本来会在表锁上排队排到超时（最长 30s）。
//!
//! **每个 client 那把锁不能省**：`McpClient` 内部是一条 stdio 管道，应答靠 `id` 匹配
//! （见 `client` 模块文档「应答匹配」），同一个 server 的并发往返交错就乱。所以降的
//! 是粒度，不是串行本身。两条性质各有一条独测钉着：
//! `tests/registry_concurrency_070.rs`。
//!
//! **这里曾经写着一句没兑现的承诺**：「持锁跨一整个往返是暂时的，043 的异步执行路会
//! 把『发请求』和『等响应』拆开」。043 发了，但它做的是**另一件事**——把整次阻塞往返
//! 挪到背景线程（`agent-runtime/src/mcp_call.rs`），协议层照旧是「写一行、等一行」，
//! 表锁一个字没动。粒度问题因此活到了 070，靠上面这两层锁修，不靠等某个未来的重构。
//!
//! 锁中毒（`f` 里 panic）的爆炸半径也跟着收窄了：毒的是那一个 server 的锁，别的 server
//! 照常用；从前毒的是整张表。

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use crate::client::McpClient;

/// 一个 client 的共享句柄：`Arc` 让它能被摘出表之后继续用（在飞的调用不会被
/// `remove`/覆盖式 `insert` 从脚下抽走），`Mutex` 是「同一个 server 串行」那把锁。
pub type ClientHandle = Arc<Mutex<McpClient>>;

/// server id → 活的 `McpClient`。表锁只在查表/改表期间持住，**从不跨一次 JSON-RPC
/// 往返**（模块文档「两层锁」）。
#[derive(Default)]
pub struct McpRegistry {
    clients: Mutex<HashMap<String, ClientHandle>>,
}

impl McpRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    /// 登记一个已经握手成功的 client。同一个 server id 重复插入会覆盖旧的——旧句柄
    /// 的最后一个持有者 drop 它时，`StdioTransport` 的 `Drop` 级联杀掉子进程，不会留
    /// 孤儿。有在飞调用时「最后一个持有者」是那个调用者，于是子进程活到那次往返结束
    /// 才死，而不是被从脚下抽走。
    pub fn insert(&self, server_id: impl Into<String>, client: McpClient) {
        let handle = Arc::new(Mutex::new(client));
        self.clients
            .lock()
            .unwrap()
            .insert(server_id.into(), handle);
    }

    /// 摘掉一个 server（比如要重连）。表当场忘掉它；返回的是**共享句柄**而不是独占
    /// 所有权——可能正有一次调用在这个 client 上在飞，调用方拿到的这份跟它是同一个。
    /// 丢掉返回值即可，子进程在最后一个持有者手里落地时被杀掉收尸。
    pub fn remove(&self, server_id: &str) -> Option<ClientHandle> {
        self.clients.lock().unwrap().remove(server_id)
    }

    pub fn contains(&self, server_id: &str) -> bool {
        self.clients.lock().unwrap().contains_key(server_id)
    }

    /// 当前登记着的 server id——诊断/`/mcp` 状态命令（045）用。
    pub fn server_ids(&self) -> Vec<String> {
        self.clients.lock().unwrap().keys().cloned().collect()
    }

    /// 对某个 server 的 client 做一次操作。**表锁只用来查一次表**，`f`（多半是一次
    /// 阻塞往返）跑在这个 client 自己的锁上——所以别的 server 不受影响，同一个
    /// server 的调用方排队（模块文档「两层锁」）。server id 不存在 → `None`（宿主
    /// 自己决定这是不是要紧——`Unavailable` 的 server 走这条路，docs/MCP.md
    /// §「host 能力差异」）。
    ///
    /// `f` 里**不要**再对同一个 server 调 `with_client`：两把锁都不可重入，会自死锁。
    pub fn with_client<T>(
        &self,
        server_id: &str,
        f: impl FnOnce(&mut McpClient) -> T,
    ) -> Option<T> {
        let handle = {
            let clients = self.clients.lock().unwrap();
            Arc::clone(clients.get(server_id)?)
        }; // ← 表锁在这里就还回去了，往返不在它里面跑。
        let mut client = handle.lock().unwrap();
        Some(f(&mut client))
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::*;
    use crate::client::connect_fake_server;

    fn fake_client() -> McpClient {
        let script = r#"read l1
printf '%s\n' '{"jsonrpc":"2.0","id":1,"result":{"protocolVersion":"2025-06-18","capabilities":{}}}'
read l2
"#;
        connect_fake_server(script, Duration::from_secs(5)).unwrap()
    }

    #[test]
    fn fresh_registry_has_no_server() {
        let registry = McpRegistry::new();
        assert!(!registry.contains("srv"));
        assert!(registry.server_ids().is_empty());
    }

    #[test]
    fn insert_then_contains_and_server_ids() {
        let registry = McpRegistry::new();
        registry.insert("srv", fake_client());
        assert!(registry.contains("srv"));
        assert_eq!(registry.server_ids(), vec!["srv".to_string()]);
    }

    #[test]
    fn remove_hands_the_handle_over_and_registry_forgets_it() {
        let registry = McpRegistry::new();
        registry.insert("srv", fake_client());
        let handle = registry.remove("srv").expect("刚插进去的该摘得下来");
        assert!(!registry.contains("srv"));
        // 摘下来的句柄仍然是活的（在飞的调用靠这条命续着，见 `remove` 文档）。
        assert_eq!(handle.lock().unwrap().protocol_version, "2025-06-18");
    }

    /// 覆盖式 `insert` 之后，表里给出的是新句柄——旧的那份只剩已经摘出去的持有者
    /// 攥着，谁都不再攥时子进程才落地。
    #[test]
    fn reinsert_replaces_the_handle_the_table_hands_out() {
        let registry = McpRegistry::new();
        registry.insert("srv", fake_client());
        let old = registry.remove("srv").expect("先摘一份旧的");
        registry.insert("srv", fake_client());
        let new = registry.remove("srv").expect("新的也该在表里");
        assert!(!Arc::ptr_eq(&old, &new), "重复 insert 该换掉表里那份句柄");
    }

    #[test]
    fn remove_of_missing_id_is_none() {
        let registry = McpRegistry::new();
        assert!(registry.remove("nope").is_none());
    }

    #[test]
    fn with_client_runs_closure_on_the_stored_client() {
        let registry = McpRegistry::new();
        registry.insert("srv", fake_client());
        let version = registry.with_client("srv", |c| c.protocol_version.clone());
        assert_eq!(version, Some("2025-06-18".to_string()));
    }

    #[test]
    fn with_client_on_missing_id_is_none() {
        let registry = McpRegistry::new();
        assert!(registry.with_client("nope", |_| ()).is_none());
    }
}

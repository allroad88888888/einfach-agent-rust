//! [`SessionRegistry`]：`SessionId → SessionHandle`（issue 030）。M3 是单副本
//! 内存表——`ARCHITECTURE.md` §「多副本时的粘性路由」画的 `trait SessionRegistry`
//! （`fn owner(&self, id) -> Option<PodAddr>`）是 M4 后 `RedisRegistry` 落地时才
//! 长出来的接缝，这里先把单机版的语义做对：`open`/`get`/`close`，外加崩溃时
//! 「不静默移除」的 dead 标记。
//!
//! # 崩溃隔离怎么在这里体现
//!
//! `open` 成功之后，registry 对每个 session 额外记一个 `died: Arc<Mutex<
//! Option<String>>>`（`crate::actor::spawn` 造的，actor 线程 panic 时自己写
//! 进去）。`get` 每次都现读这个单元格——不是「注册表在后台轮询发现死亡」，
//! 而是「死亡的证据一直在那，谁问就现给谁看」：panic 那一刻事件流已经收到
//! 终态广播（[`crate::event::SessionEvent::SessionDied`]），`get`/`close` 只是
//! 把同一个死因也报给还没在听事件流、但直接查 registry 的调用方。

mod spec;

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::thread;

use crate::actor::{self, OpenError};
use crate::command::Command;
use crate::handle::SessionHandle;

pub use spec::{OpenSpec, ToolTableSpec};

/// 一个 session 的标识。廉价可 `Clone`（`Arc<str>` 包底），当 `HashMap` 键用
/// （`Eq + Hash + Ord`——`Ord` 不是查找必需，但排序输出对诊断/测试断言方便，
/// 成本为零就一起派生了）。
#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct SessionId(Arc<str>);

impl SessionId {
    pub fn new(id: impl Into<Arc<str>>) -> Self {
        SessionId(id.into())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Display for SessionId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl From<&str> for SessionId {
    fn from(s: &str) -> Self {
        SessionId(Arc::from(s))
    }
}

impl From<String> for SessionId {
    fn from(s: String) -> Self {
        SessionId(Arc::from(s.as_str()))
    }
}

/// `get` 的回答：活着（给你一份可以直接用的句柄），或者死了（给你死因，不是
/// `None`——`None` 留给「这个 id 压根没 open 过」）。
pub enum SessionQuery {
    Alive(SessionHandle),
    Dead { reason: String },
}

/// `close` 的失败：这个 id 没 open 过，或者 open 过但 actor 已经先一步崩了
/// （`WasDead` 不是「关闭失败」——线程已经不在了，`close` 依然把它从表里摘掉，
/// 只是要如实告诉调用方「你以为在关一个活的，其实它已经死了」）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CloseError {
    NotFound,
    WasDead { reason: String },
}

impl std::fmt::Display for CloseError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            CloseError::NotFound => write!(f, "这个 session id 没有 open 过"),
            CloseError::WasDead { reason } => {
                write!(f, "actor 已经先一步崩了（{reason}），现在才摘表")
            }
        }
    }
}

impl std::error::Error for CloseError {}

struct Entry {
    handle: SessionHandle,
    join: thread::JoinHandle<()>,
    died: Arc<Mutex<Option<String>>>,
}

/// 表里一个 id 对应的槽位。**`Opening` 是这张表的并发闸**——见 [`SessionRegistry::
/// open`] 文档「为什么需要一个中间态」：光有「检查 + 之后再插入」两步，中间隔着
/// `actor::spawn`（起线程、等握手，不快）,两个并发 `open` 都能在检查那一步看到
/// 「表里没有」，都往下走，最后一个 `insert` 覆盖前一个——前一个 actor 线程从此
/// 没人 `join`、没人 `close`，两份还各自可能在写同一个持久化文件。`Opening` 把
/// 「我正在起这个 id」这件事本身写进表里，让第二个并发 `open` 在检查那一步就能
/// 看到并拒绝，不必等到 `insert` 那一刻才发现自己白起了一个线程。
enum Slot {
    /// 已经有人在 `open` 这个 id 了，`actor::spawn` 还没返回。
    Opening,
    Ready(Entry),
}

/// `SessionId → SessionHandle` 的单机内存表。
#[derive(Default)]
pub struct SessionRegistry {
    sessions: Mutex<HashMap<SessionId, Slot>>,
}

impl SessionRegistry {
    pub fn new() -> Self {
        SessionRegistry {
            sessions: Mutex::new(HashMap::new()),
        }
    }

    /// 开一个 session：起专属 actor 线程，登记进表，返回句柄。同一个 id 重复
    /// `open`（且现有那份还活着，或者正在被别的调用 `open` 着）会被拒绝——两个
    /// actor 线程共享同一份持久化文件路径会互相踩（`agent_store::Jsonl` 没有
    /// 跨进程/跨线程的写锁协商），「同一个 id 至多一个活着（或正在起）的
    /// actor」是这张表要维持的不变量。已死的旧 entry 不挡道：`open` 会先摘掉
    /// 它再起新的（重开一个曾经崩溃的 session 是合理操作，不该被一条陈旧的
    /// 死亡记录卡住）。
    ///
    /// # 为什么需要一个中间态（[`Slot::Opening`]）
    ///
    /// `actor::spawn` 起线程、等启动握手，不是一次锁内的轻量操作——不能在持锁
    /// 期间调它（会把整张表锁死到这一个 session 起好为止，别的 session 的
    /// `open`/`get`/`close` 全部干等）。于是「检查这个 id 是否已被占用」和
    /// 「把新 entry 记进表」天然是两次分开的持锁操作，中间那段没锁的间隙就是
    /// 两个并发 `open(id)` 都能通过检查、都真的起了一个线程、最后一个
    /// `insert` 悄悄覆盖前一个——前一个线程从此没人 `join`。`Slot::Opening`
    /// 就是为了让第一次检查那一刻就把「这个 id 有主了」写进表，堵死这个窗口
    /// （独测 `tests/concurrent_open_of_the_same_id_only_one_wins.rs` 钉住，
    /// 临时改回旧逻辑验证过它确实会抓到——8 个并发线程会全部 `Ok`）。
    pub fn open(&self, spec: OpenSpec) -> Result<SessionHandle, OpenError> {
        let id = spec.id.clone();
        {
            let mut sessions = self.sessions.lock().unwrap();
            match sessions.get(&id) {
                Some(Slot::Opening) => {
                    return Err(OpenError(format!(
                        "session \"{id}\" 正在被另一次 open() 调用起，等它完成或者失败再试"
                    )));
                }
                Some(Slot::Ready(existing)) if existing.died.lock().unwrap().is_none() => {
                    return Err(OpenError(format!(
                        "session \"{id}\" 已经 open 着，先 close 或者等它自己死了再重开"
                    )));
                }
                Some(Slot::Ready(_)) | None => {} // 没有，或者有但已经死了——占位，继续往下起。
            }
            sessions.insert(id.clone(), Slot::Opening);
        }

        let spawn_result = actor::spawn(spec);
        let mut sessions = self.sessions.lock().unwrap();
        match spawn_result {
            Ok(spawned) => {
                let handle = spawned.handle.clone();
                sessions.insert(
                    id,
                    Slot::Ready(Entry {
                        handle: spawned.handle,
                        join: spawned.join,
                        died: spawned.died,
                    }),
                );
                Ok(handle)
            }
            Err(e) => {
                sessions.remove(&id); // 起失败：把占位摘掉，不留一个永远 Opening 的死坑挡着以后的重试。
                Err(e)
            }
        }
    }

    /// 表里当前登记过的全部 id——`Opening`（正在起）也算在内，只有从没
    /// `open` 过、或者已经被 [`Self::close`] 摘表的才不在这份列表里。**不区分
    /// 死活**：宿主优雅关闭时（`crate::http::SessionsHandle::close_all`，035）
    /// 要做的是「把还挂在表里的都关一遍」，已经死了的 entry 调 `close` 只是
    /// 拿到 [`CloseError::WasDead`] 而不是新错误，不值得在这里先过滤一遍。
    pub fn ids(&self) -> Vec<SessionId> {
        self.sessions.lock().unwrap().keys().cloned().collect()
    }

    /// 查一个 session 现在的状态。`None` = 这个 id 从没 `open` 过，或者正在
    /// `open` 的路上还没起好——对调用方来说是同一种「问不出个所以然」，不值得
    /// 为「正在起」单开一个 `SessionQuery` 变体（这个窗口通常是毫秒级）。
    pub fn get(&self, id: &SessionId) -> Option<SessionQuery> {
        let sessions = self.sessions.lock().unwrap();
        match sessions.get(id)? {
            Slot::Opening => None,
            Slot::Ready(entry) => Some(match entry.died.lock().unwrap().clone() {
                Some(reason) => SessionQuery::Dead { reason },
                None => SessionQuery::Alive(entry.handle.clone()),
            }),
        }
    }

    /// 优雅关闭：从表里摘掉、发 `Shutdown`（排在队列末尾，不打断在飞的轮次
    /// ——想立刻打断先发 `Cancel`）、`join` 线程。**摘表在 `join` 之前**——
    /// `JoinHandle::join` 要求拿到所有权（`fn join(self)`），拿不到「先留在
    /// 表里给别人查、`join` 完再摘」两全的写法（除非把 `JoinHandle` 也包一层
    /// `Option` 让它能被 `take`，M3 没有必要为一条尚未出现在验收里的并发查询
    /// 场景加这层复杂度）。代价是：`close` 正在 `join` 的这一小段时间里，
    /// 另一个线程 `get` 同一个 id 会看到 `None`（就像它从没 `open` 过），不是
    /// `Alive`/`Dead`——`close` 的调用方自己知道「我在关它」，这不是新信息；
    /// 需要这段窗口语义更精确时再回头处理。
    pub fn close(&self, id: &SessionId) -> Result<(), CloseError> {
        let entry = {
            let mut sessions = self.sessions.lock().unwrap();
            match sessions.remove(id) {
                Some(Slot::Ready(entry)) => entry,
                // `Opening`：某个并发的 `open(id)` 还在起线程，这里没有一个
                // 我们能 `join` 的句柄——摘掉这个占位对那次 `open` 无害（它
                // 起完之后会无条件覆盖式 `insert` 一个 `Ready`，不看这里还剩
                // 什么），对 `close` 的调用方就是「没找到可关的东西」。
                Some(Slot::Opening) | None => return Err(CloseError::NotFound),
            }
        };
        // 已经死了的话 Shutdown 送不到（`mpsc::Receiver` 早没了）——`send`
        // 的 `Err` 在这里就是预期结果，不是需要上报的新错误，`died` 单元格
        // 才是死因的正牌来源。
        let _ = entry.handle.send(Command::Shutdown);
        let _ = entry.join.join(); // 见 `crate::actor` 模块文档：panic 已被 catch_unwind 接住，这里不会看到 Err，防御性地仍不 unwrap。
        match entry.died.lock().unwrap().clone() {
            Some(reason) => Err(CloseError::WasDead { reason }),
            None => Ok(()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn session_id_displays_as_its_string() {
        let id = SessionId::from("s-1");
        assert_eq!(id.to_string(), "s-1");
        assert_eq!(id.as_str(), "s-1");
    }

    #[test]
    fn get_on_a_never_opened_id_is_none() {
        let registry = SessionRegistry::new();
        assert!(registry.get(&SessionId::from("nope")).is_none());
    }

    #[test]
    fn close_on_a_never_opened_id_is_not_found() {
        let registry = SessionRegistry::new();
        assert_eq!(
            registry.close(&SessionId::from("nope")),
            Err(CloseError::NotFound)
        );
    }

    #[test]
    fn ids_is_empty_on_a_fresh_registry() {
        let registry = SessionRegistry::new();
        assert!(registry.ids().is_empty());
    }
}

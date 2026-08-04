//! [`RunnerCtx`] 持有的远端工具等待槽。
//!
//! 这个状态只能由 actor 线程修改；它把 Web 宿主的回传绑定到已经派出的精确工具
//! 调用，避免 HTTP 客户端伪造 epoch 或写入任意 `ToolsPending` 槽。
//!
//! 每个槽还带一条**截止线**（060）：正常路径靠宿主 `POST /tool_result` 收敛，
//! 但「前端崩了 / 网关挂了 / 客户端压根没实现这个工具」这三种情况下没有任何
//! 东西会回来，槽不收敛、会话永久停在 `ToolsPending`。到点怎么处理不在这个
//! 文件（见 [`crate::deadline`]），这里只负责**记住每个槽什么时候算过期**。
//!
//! # 072：这张表是「这次调用还要不要执行」的**唯一权威**
//!
//! 「这帧 `tool_executing` 是不是补发的」根本不是正确的判据——派了活、宿主还没
//! 执行就换了个客户端，那帧确实是补发的、活却真的还欠着。唯一权威的判据是
//! 「这次调用现在是否还在这张表里等着」。所以这张表**导出成一份只读投影**
//! （[`RemoteToolWaiting`] → `GET /sessions/{id}/pending_tools`），宿主收到帧先
//! 向它求证，命中才执行；每次连上再拉一次把欠的活补掉。
//!
//! 于是**下面四个变更点各要通知一次投影，一个都不能漏**：登记（派活）、回传
//! （正常收场）、截止线（060 判失败）、取消/undo/redo（斩断）。漏了后两者，宿主
//! 会去执行一个**已经收场**的调用——回传照旧被安全拒绝，副作用照旧发生，正是这
//! 份投影要堵的病换个入口复发。
//!
//! 通知必须发生在**槽变化的那一刻**，不能等到宿主的命令边界：登记就在
//! `run_turn` 内部（`crate::dispatch` 的远端第五路），**下一行**就把
//! `tool_executing` 广播出去了；投影晚一步，客户端就有一个「收到帧 → 去问 →
//! 说没有」的窗口，那是漏活。

use std::time::Instant;

use agent_core::{AgentId, Epoch, ToolCallId, ToolCallRequest};

use crate::ctx::RunnerCtx;

/// 一条还欠着的远端调用，**投影形状**（072）：宿主执行前唯一要问的两件事——
/// 「这次调用还欠着吗」「欠的是什么」。
///
/// **刻意不含 `epoch` 和 `deadline`**：epoch 是服务端保管的凭据（客户端伪造不了
/// 也不该看见，`agent-server` 的 `routes/tool_result.rs` 模块文档写死了这条），
/// 截止线是 060 的内部账，宿主拿它做不了任何正确的决定。
#[derive(Clone, Debug, PartialEq)]
pub struct RemoteToolWaiting {
    pub agent: AgentId,
    pub call_id: ToolCallId,
    pub request: ToolCallRequest,
}

/// 已派给远端宿主、尚未获确认的调用。
pub(crate) struct PendingRemoteTool {
    pub(crate) agent: AgentId,
    pub(crate) call_id: ToolCallId,
    pub(crate) epoch: Epoch,
    pub(crate) request: ToolCallRequest,
    /// 060：登记那一刻按 [`RunnerCtx::with_remote_tool_timeout`] 的预算算好的
    /// **绝对时刻**。存绝对时刻而不是存 `Duration` + 起始点：判过期是热路径上
    /// 反复做的事（宿主每次空闲等命令都要问一次「最早的一条什么时候到点」），
    /// 一个 `Instant` 比较就够，不必每次重算。
    pub(crate) deadline: Instant,
}

#[derive(Default)]
pub(crate) struct PendingRemoteTools(Vec<PendingRemoteTool>);

impl RunnerCtx {
    /// 登记一个仅能由远端宿主回传的调用。重复 id 违反 provider 协议；保留较早
    /// 的登记，确保任何回传至多收敛一个原始工具槽。
    ///
    /// 截止线在这里算（`now + ctx.remote_tool_timeout`），不由调用点传：
    /// dispatch 只知道「这次调用要交给宿主」，「等多久算等不到了」是会话级配置。
    pub(crate) fn register_remote_tool(
        &mut self,
        agent: AgentId,
        call_id: ToolCallId,
        epoch: Epoch,
        request: ToolCallRequest,
    ) {
        if self
            .pending_remote_tools
            .0
            .iter()
            .any(|pending| pending.agent == agent && pending.call_id == call_id)
        {
            return;
        }
        let deadline = Instant::now() + self.remote_tool_timeout;
        self.pending_remote_tools.0.push(PendingRemoteTool { agent, call_id, epoch, request, deadline });
        self.publish_pending_remote_tools();
    }

    /// 只取走确实仍在等待的精确调用；重复、跨 agent 或已撤销的回传都会失败。
    pub(crate) fn take_remote_tool(
        &mut self,
        agent: &AgentId,
        call_id: &ToolCallId,
    ) -> Option<PendingRemoteTool> {
        let index = self
            .pending_remote_tools
            .0
            .iter()
            .position(|pending| &pending.agent == agent && &pending.call_id == call_id)?;
        let taken = self.pending_remote_tools.0.remove(index);
        self.publish_pending_remote_tools();
        Some(taken)
    }

    /// 取走 `now` 这一刻**已经过期**的全部等待槽（060）。取走即消费：过期槽从
    /// 此不再认任何回传，迟到的结果走 `take_remote_tool` 找不到那条既有拒绝路。
    pub(crate) fn take_expired_remote_tools(&mut self, now: Instant) -> Vec<PendingRemoteTool> {
        let mut expired = Vec::new();
        let mut index = 0;
        while index < self.pending_remote_tools.0.len() {
            if self.pending_remote_tools.0[index].deadline > now {
                index += 1;
                continue;
            }
            expired.push(self.pending_remote_tools.0.remove(index));
        }
        // 072 第二个必接的变更点：截止线取走的槽**已经按失败收尾**，投影得在这一刻
        // 就收缩——漏了它，宿主刷新后会去执行一个早已收场的调用（本 issue 的病换个
        // 入口复发，见模块文档）。这一刻没有过期槽就不通知：`take_expired_remote_tools`
        // 每次空闲超时都会被调一次，无变化时不该白扰动投影。
        if !expired.is_empty() {
            self.publish_pending_remote_tools();
        }
        expired
    }

    /// 最早的一条截止线，没有等待槽就是 `None`（060）。
    ///
    /// 宿主用它决定空闲等命令时该阻塞多久：`None` → 照旧无限期阻塞（没有远端
    /// 等待的会话一分钱开销都不多付）；`Some(t)` → 至多等到 `t`，到点调
    /// [`crate::sweep_remote_tool_deadlines`]。
    pub fn next_remote_deadline(&self) -> Option<Instant> {
        self.pending_remote_tools.0.iter().map(|pending| pending.deadline).min()
    }

    /// 还有几个远端调用在等回传。宿主/测试用来判断「这次派发到底有没有进等待
    /// 槽」——060 验收第一条（未声明的 `web:` 名字**不进槽**）就断言它是 0。
    pub fn pending_remote_tool_count(&self) -> usize {
        self.pending_remote_tools.0.len()
    }

    /// 取消、撤回或会话终止后切断未完成远端调用，防止迟到回传写入新 epoch。
    pub fn discard_remote_tools(&mut self) {
        // 072 第三个必接的变更点。空表时直接返回：`/undo` `/redo` `/cancel` 每次都
        // 会调它一次，绝大多数会话根本没有远端等待，不该为此每条命令都扰动一次投影。
        if self.pending_remote_tools.0.is_empty() {
            return;
        }
        self.pending_remote_tools.0.clear();
        self.publish_pending_remote_tools();
    }

    /// 此刻还欠着回传的远端调用（072 的投影本体）。数据源是等待槽表**本身**，
    /// 不是第二份账——「不新增第二真值源」（INTEGRATION.md §七）。
    ///
    /// 公开且纯读：宿主（`agent-server` 的 `GET /sessions/{id}/pending_tools`）和
    /// 测试都拿它，跟 [`RunnerCtx::pending_remote_tool_count`] 同一类口子。
    pub fn pending_remote_tools(&self) -> Vec<RemoteToolWaiting> {
        self.pending_remote_tools
            .0
            .iter()
            .map(|pending| RemoteToolWaiting {
                agent: pending.agent.clone(),
                call_id: pending.call_id.clone(),
                request: pending.request.clone(),
            })
            .collect()
    }

    /// 设一条待办投影变化回调（072）。照 [`RunnerCtx::with_tree_events`] 同款：
    /// 独立字段、**替换**不是追加、没设就是 no-op（CLI 就是这个默认值——它只有
    /// 一个客户端，没有第二个实例要对账）。
    ///
    /// 回调收到的是**变化之后**的整份快照，不是增量：一份小 `Vec` 的整体重写比
    /// 「加了一条 / 减了一条」两种消息好——接收方只要照单覆盖，不必自己维护一份
    /// 会跟服务端漂移的镜像（048 的树快照同一条判据）。
    pub fn with_pending_remote_tools(mut self, on_change: Box<dyn FnMut(Vec<RemoteToolWaiting>)>) -> Self {
        self.on_pending_remote_tools = Some(on_change);
        self
    }

    /// 把变化之后的整份投影发给宿主设的回调。没设就完全不算那份快照——跟
    /// [`RunnerCtx::tree_events_enabled`] 挡在调用点前面是同一个取舍。
    fn publish_pending_remote_tools(&mut self) {
        if self.on_pending_remote_tools.is_none() {
            return;
        }
        let snapshot = self.pending_remote_tools();
        if let Some(on_change) = self.on_pending_remote_tools.as_mut() {
            on_change(snapshot);
        }
    }
}

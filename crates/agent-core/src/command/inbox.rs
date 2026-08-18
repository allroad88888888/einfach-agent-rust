//! 收件箱：**往别人那儿放一句话，和把放进来的话喂给自己**（205，决策 35 §二）。
//!
//! 这个文件只做这一件事的三条命令 + 一个读口。「一条投递什么时候被送达」的两档
//! 定义与编解码在 [`value::inbox`](crate::value::inbox)，「谁排空、什么时候排空」
//! 是运行时的事（issue 206），这里只提供那两个定点要调的东西。
//!
//! ## 为什么投递是写、不是读
//!
//! `Slot::Inbox` 站 `Private`（`graph::visibility`）：**发得进去 ≠ 读得出来**。
//! A 能往 B 的收件箱放话，但读不到 B 的收件箱——包括自己投的那条被没被消费。
//! 这不是限制得不够，是刻意的：要确认对方收到没有，等他回一条，跟人一样。
//!
//! ## 跨 agent 写，跟 spawn 同一条路
//!
//! [`Session::deliver`] 写的是**目标**的槽位，而 `Txn` 绑在一个 agent 上——所以它
//! 走 `Txn::set_key`（`txn.rs` 那条为 spawn/despawn 开的口），并且
//! `commit_as(from, …)`：**这条 entry 记在发送方头上**。于是 `/undo` 撤掉发送方
//! 那一轮，投出去的话跟着消失——整棵树共用一条日志的好处在这里又兑现一次，
//! 不需要任何分布式事务。
//!
//! ## 可逆性
//!
//! 三条命令全是 `Undoability::StateOnly`（`Txn` 的默认档）：纯状态，没碰外部世界，
//! 回滚状态就够了，不需要还原钩子（决策 34 那套里最干净的一格）。

use std::sync::Arc;

use crate::graph::{AtomKey, Slot};
use crate::ids::AgentId;
use crate::value::inbox::{self, Deliver, InboxItem};
use crate::value::message::{ContentBlock, Role};

use super::session::Session;

/// 一次投递被拒的理由。
///
/// 全部是**显式变体**，不是 `Option`/`bool`：投不进去的原因决定调用方下一步怎么办
/// （换个人发、还是先把话收着、还是告诉模型它写错了），糊成一个 `false` 之后只能靠猜。
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum DeliverDenied {
    /// 正文是空的（或只有空白）。空话进对方历史只会占一格 token 什么也没说。
    EmptyText,
    /// 发给自己。要给自己留话有 `Slot::Notes`（issue 209），不是这条路。
    ToYourself { agent: AgentId },
    /// 这个 id 不在本会话这棵树上。
    NotInSession { target: AgentId },
    /// 在树上，但已经不活着（没 spawn 出来过、被撤销了、或者已经 despawn）。
    TargetNotLive { target: AgentId },
    /// 发送方自己不活着——一个死掉的 agent 不该还能说话。
    SenderNotLive { from: AgentId },
    /// [`Deliver::NextTurn`] 的目标只能是 root。
    ///
    /// **不是保守，是子 agent 不跨 turn**（ORCHESTRATION §二/§四.4）：孤儿在 turn
    /// 收尾被 `despawn_child` 拆掉，所以投给别人等于投进一个下一轮不存在的收件箱。
    /// 显式拒而不是投完静默丢——后者是这条路上唯一一个会悄悄吞掉消息的入口。
    NextTurnMustTargetRoot { target: AgentId, root: AgentId },
}

/// 排空时那条话进对方历史的样子里，标记来源的那一段。
///
/// **这是 core 唯一一处替另一个 agent 往 prompt 里写字的地方**，所以它必须逐字节
/// 确定（红线 11）：`from` 是路径 id（`root/a1`），没有时间戳、没有序号、没有随机
/// id。改这个格式等于改所有历史会话下一轮的前缀，别顺手改。
fn rendered(item: &InboxItem) -> String {
    format!("[来自 {}] {}", item.from.as_str(), item.text)
}

impl Session {
    /// 这个 agent 收件箱里**还没被消费**的全部条目，按投递顺序。
    ///
    /// 宿主侧读（跟 `messages_of` 同一类），不是跨 agent 读——`Inbox` 对**别的
    /// agent** 是 `Private`，而宿主不是 agent。运行时靠它在 turn 收尾判断「还有
    /// 没有没被读到的话」（issue 206 §4）。
    ///
    /// 非创建：探一个不存在的 id 答空表，不在 family 里留 atom。
    pub fn inbox_of(&self, agent: &AgentId) -> Vec<InboxItem> {
        self.peek(&AtomKey::Agent(agent.clone(), Slot::Inbox))
            .map(|v| inbox::from_value(&v))
            .unwrap_or_default()
    }

    /// 往 `target` 的收件箱尾部放一句话。
    ///
    /// **只追加，不唤醒任何人**——「收信人已经跑完了要不要把它拉回来」是运行时的
    /// 编排问题（issue 206 §3），core 不驱动任何东西（红线 7）。
    ///
    /// `when` 的两档见 [`Deliver`]；`NextTurn` 只能投给 root，理由见
    /// [`DeliverDenied::NextTurnMustTargetRoot`]。
    pub fn deliver(
        &mut self,
        from: &AgentId,
        target: &AgentId,
        text: Arc<str>,
        when: Deliver,
    ) -> Result<(), DeliverDenied> {
        if text.trim().is_empty() {
            return Err(DeliverDenied::EmptyText);
        }
        if from == target {
            return Err(DeliverDenied::ToYourself {
                agent: target.clone(),
            });
        }
        if !self.in_session(target) {
            return Err(DeliverDenied::NotInSession {
                target: target.clone(),
            });
        }
        if !self.is_live(target) {
            return Err(DeliverDenied::TargetNotLive {
                target: target.clone(),
            });
        }
        if !self.is_live(from) {
            return Err(DeliverDenied::SenderNotLive { from: from.clone() });
        }
        let root = self.agent().clone();
        if when == Deliver::NextTurn && target != &root {
            return Err(DeliverDenied::NextTurnMustTargetRoot {
                target: target.clone(),
                root,
            });
        }

        let mut items = self.inbox_of(target);
        items.push(InboxItem {
            from: from.clone(),
            text,
            when,
        });
        let key = AtomKey::Agent(target.clone(), Slot::Inbox);
        let next = inbox::to_value(&items);
        // `commit_as(from, …)`：这条 entry 记在**发送方**头上，于是撤掉发送方那一轮
        // 时投出去的话跟着消失（模块文档「跨 agent 写」）。
        self.commit_as(from, "deliver", |txn| txn.set_key(key, next));
        Ok(())
    }

    /// 把 `agent` 收件箱里 [`Deliver::Now`] 的条目按序喂进它的 `Messages`。
    ///
    /// **[`Deliver::NextTurn`] 的条目原地不动**——两档共用一个槽位，各认各的定点
    /// （模块文档）。调用点在收信人**组装 provider 请求之前**（issue 206 §2）：
    /// 放在别处就会让一条在飞请求的回复落在被投递的那条**后面**，历史里长出一段
    /// 「答非所问」，而那不报错。
    ///
    /// 返回搬了几条，让调用方决定要不要因此重新驱动这个 agent——**core 自己不驱动
    /// 任何东西**（红线 7）。没有待收的就什么都不做，一条 entry 都不落。
    pub fn drain_now(&mut self, agent: &AgentId) -> usize {
        self.drain(&agent.clone(), Deliver::Now, "drain_now")
    }

    /// 把 root 收件箱里 [`Deliver::NextTurn`] 的条目按序喂进 root 的 `Messages`。
    ///
    /// 调用点在 `begin_turn` **之后**、本轮第一次组装请求之前（issue 206 §2）：
    /// 放在 `begin_turn` 之前，那条留言会挂在**上一轮**的尾巴上，undo 掉上一轮就把
    /// 一条还没被读过的话一起吞了。
    pub fn drain_next_turn(&mut self) -> usize {
        let root = self.agent().clone();
        self.drain(&root, Deliver::NextTurn, "drain_next_turn")
    }

    /// 两个定点共用的后半段：挑出这一档、喂进历史、把剩下的写回去。
    ///
    /// 合成一处是刻意的——两份拷贝就是两处可以判错的地方，而判错的形式是「一档把
    /// 另一档的条目也搬走了」，那等于把一条该等下一轮的话当场灌了进去。
    fn drain(&mut self, agent: &AgentId, when: Deliver, label: &'static str) -> usize {
        let (taken, kept): (Vec<_>, Vec<_>) = self
            .inbox_of(agent)
            .into_iter()
            .partition(|item| item.when == when);
        if taken.is_empty() {
            return 0;
        }
        let key = AtomKey::Agent(agent.clone(), Slot::Inbox);
        let rest = inbox::to_value(&kept);
        self.commit_as(agent, label, |txn| {
            for item in &taken {
                txn.push_message(Role::User, vec![ContentBlock::Text(Arc::from(rendered(item)))]);
            }
            txn.set_key(key, rest);
        });
        taken.len()
    }
}

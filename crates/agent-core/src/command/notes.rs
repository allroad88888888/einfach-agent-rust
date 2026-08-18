//! 草稿纸：**一个 agent 自己记的东西**（209，决策 35 §三）。
//!
//! 这个文件只做这一件事的一条命令 + 一个读口。值的形状与编解码在
//! [`value::notes`](crate::value::notes)，工具面（模型怎么调它）在
//! `agent-runtime`。
//!
//! ## 为什么是新槽位，不是给现有槽位开写口
//!
//! 用户要的是「改本 agent 状态」。现有槽位一格一格数过去没有一格是模型的：
//! `MaxTurns` 是部署方的、`ToolsAllowed` 是父给的、`SendPlan`/`Summaries` 是
//! adapter 的、`Status` 是父要读的。**给它们开写口 = 让被约束者改自己的约束。**
//!
//! 新槽位不碰任何现有不变量，而且白拿全套机制：`/undo` 连带撤销、崩溃恢复自动
//! 带回、审计看得到每一次改。这是本仓架构直接掉出来的，不是新造的机制——
//! 这个文件里没有一行代码认识「撤销」或「恢复」。
//!
//! ## 两道上限，两种拒法
//!
//! 这张表会以 tool_result 的形式进 prompt，所以无上限 = 给模型一把慢慢烧钱的枪。
//! 两道闸各有各的形状：
//!
//! - **条目数**（[`MAX_NOTES`]）：撞顶**显式拒**（[`NoteDenied::TooManyNotes`]），
//!   不静默丢。丢一条的症状是模型以为记下了、下一轮查不到，然后再记一遍。
//! - **单条长度**（[`NOTE_VALUE_CAP`]）：core 这一层也**拒**。截断要不要做、
//!   怎么如实告诉模型，是工具层的判断（它有话跟模型说，core 没有）——core 只
//!   保证「一条超长的 note 进不了状态」，因为这条闸真正拦的是宿主自己写错，
//!   不是模型。
//!
//! key 一律拒不截断：截断 key 会**改掉它的身份**——模型下一轮拿原来的名字去查，
//! 查不到，而它记的时候明明成功了。
//!
//! ## 可逆性
//!
//! `Undoability::StateOnly`（`Txn` 的默认档）：纯状态，没碰外部世界，回滚状态就
//! 够了，不需要还原钩子——决策 34 那套里最干净的一格。

use std::sync::Arc;

use crate::graph::{AtomKey, Slot};
use crate::ids::AgentId;
use crate::value::notes::{self, Notes};

use super::session::Session;

/// 一张草稿纸最多几条。
///
/// 数字本身不神圣，**有上限**才是。32 条是「模型真的会用来记事」和「整张表还能
/// 一眼看完」之间的一个位置：撞顶时模型收到的是一句「满了，先删几条」，
/// 而不是一份悄悄少了几行的表。
pub const MAX_NOTES: usize = 32;

/// 单条正文最多几个字节。
///
/// 按**字节**不按字符：进 prompt 的计价单位是 token，而字节是这一层能确定的最
/// 接近的量。1KB 一条 × 32 条 = 32KB 上限，跟 004 的工具结果上限同一个数量级。
pub const NOTE_VALUE_CAP: usize = 1024;

/// key 最多几个字节。key 每一条都会跟正文一起进 prompt，而它该是个标签不是段落。
pub const NOTE_KEY_CAP: usize = 64;

/// 写一条 note 被拒的理由。
///
/// 全部是**显式变体**，不是 `Option`/`bool`：拒绝的原因决定模型下一步怎么办
/// （换个短点的 key、先删几条、还是它压根不该写），糊成一个 `false` 之后只能靠猜。
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum NoteDenied {
    /// key 是空的（或只有空白）。
    EmptyKey,
    /// key 太长。**拒不截断**：截断会改掉它的身份（见模块文档）。
    KeyTooLong { bytes: usize, max: usize },
    /// 正文太长。
    ValueTooLong { bytes: usize, max: usize },
    /// 条目数撞顶。`live` 是撞顶时表里已有的条数。
    ///
    /// **只在新增时才可能**——覆盖一个已有的 key 不增加条数，撞顶之后照样改得动
    /// 已经记下的东西（不然模型会陷在「满了 → 想改一条腾地方 → 也被拒」里）。
    TooManyNotes { live: usize, max: usize },
    /// 这个 id 不在本会话这棵树上。
    NotInSession { agent: AgentId },
    /// 在树上，但已经不活着。一个死掉的 agent 不该还能往状态里写东西。
    NotLive { agent: AgentId },
}

impl Session {
    /// 这个 agent 的整张草稿纸，**按 key 升序**（容器自带，见
    /// [`value::notes`](crate::value::notes)）。
    ///
    /// 宿主侧读（跟 `messages_of` 同一类），不是跨 agent 读——`Notes` 对**别的
    /// agent** 是 `Private`，而宿主不是 agent。
    ///
    /// 非创建：探一个不存在的 id 答空表，不在 family 里留 atom。
    pub fn notes_of(&self, agent: &AgentId) -> Notes {
        self.peek(&AtomKey::Agent(agent.clone(), Slot::Notes))
            .map(|v| notes::from_value(&v))
            .unwrap_or_default()
    }

    /// 写一条 note。`value` 为 `None` = **删掉这条**。
    ///
    /// 同一个 key 写第二次是覆盖，不是追加——它是一张表，不是流水账。
    ///
    /// 删一条不存在的 key 是**成功且什么都不做**（不落 entry）：模型删两次不该
    /// 收到一句错误，那只会让它以为出了别的问题。同理，写一个跟现值一模一样的
    /// 值也不落 entry——`record_set` 本来就不给没变的值落 `Change`（009 的
    /// 「幽灵步不落条目」），这里只是不额外制造一条空 batch。
    pub fn set_note(
        &mut self,
        agent: &AgentId,
        key: Arc<str>,
        value: Option<Arc<str>>,
    ) -> Result<(), NoteDenied> {
        if key.trim().is_empty() {
            return Err(NoteDenied::EmptyKey);
        }
        if key.len() > NOTE_KEY_CAP {
            return Err(NoteDenied::KeyTooLong {
                bytes: key.len(),
                max: NOTE_KEY_CAP,
            });
        }
        if let Some(text) = &value
            && text.len() > NOTE_VALUE_CAP
        {
            return Err(NoteDenied::ValueTooLong {
                bytes: text.len(),
                max: NOTE_VALUE_CAP,
            });
        }
        if !self.in_session(agent) {
            return Err(NoteDenied::NotInSession {
                agent: agent.clone(),
            });
        }
        if !self.is_live(agent) {
            return Err(NoteDenied::NotLive {
                agent: agent.clone(),
            });
        }

        let mut table = self.notes_of(agent);
        match value {
            None => {
                if table.remove(&key).is_none() {
                    return Ok(());
                }
            }
            Some(text) => {
                // 闸只拦**新增**：撞顶之后改一条已有的照样让过，不然模型会陷在
                // 「满了 → 想改一条腾地方 → 也被拒」里，而它没有别的出路。
                if !table.contains_key(&key) && table.len() >= MAX_NOTES {
                    return Err(NoteDenied::TooManyNotes {
                        live: table.len(),
                        max: MAX_NOTES,
                    });
                }
                if table.get(&key).is_some_and(|old| *old == text) {
                    return Ok(());
                }
                table.insert(key, text);
            }
        }

        let atom = AtomKey::Agent(agent.clone(), Slot::Notes);
        let next = notes::to_value(&table);
        self.commit_as(agent, "set_note", |txn| txn.set_key(atom, next));
        Ok(())
    }
}

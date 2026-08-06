//! [`Session::step`]：喂一个事件，推进状态，产出宿主要执行的 effect 列表。
//!
//! **契约与 M1 的 `engine::step` 逐字相同**（001 定的三个词汇、闸的位置、
//! 「core 决定该发生什么，宿主决定怎么发生」）。变的只有状态的住处：
//! `&mut TurnState` 变成 `&mut Session`，一次转移的写入落成一条 `Entry`。
//!
//! 红线 7：这里没有 IO，**也没有 `Instant::now()`**——超时是宿主注入的
//! `Event::Timeout`，计时器活在 runner 里。于是测试能在零时间内模拟任意超时序列。
//!
//! 红线 12：这里一条模型相关的判断都没有，结构上也做不到——依赖方向是
//! providers → core，能力位那张表连类型都在 core 之外。

use crate::engine::effect::Effect;
use crate::engine::event::Event;

use super::session::Session;
use super::transitions;

impl Session {
    /// 喂一个事件，推进状态，产出 effect。
    ///
    /// # epoch 闸（红线 6）
    ///
    /// 事件带的 epoch 跟当前世代不一致 → **直接丢弃**：返回空 `Vec`，一个 primitive
    /// 都不写，**日志里也不会多出一条 entry**，也不发通报。这挡的是「幽灵结果」：
    /// 工具在飞时用户按了取消或 undo，结果回来时它属于一个已经被回滚掉的世界。
    /// `Event::epoch()` 对 `UserInput` 和 `Cancel` 返回 `None`，这两种事件永远不过
    /// 这道闸——用户意图针对的永远是当前世界。
    ///
    /// 闸判的是「不等于」不是「小于」：世代只增不减，比当前**新**的 epoch 只可能来自
    /// 一个不该存在的世代，同样是过期。
    ///
    /// 不发通报是刻意的：过期结果是**正常现象**（取消/undo 之后一定会有一批回执陆续
    /// 到达），每条都喊一声只会让 CLI 刷屏。
    ///
    /// 闸装在函数入口而不是各条转移里：转移表有几十格，漏一格就是漏一条回写路径，
    /// 而漏了不报错。哪些事件要过闸由 `Event::epoch` 一处回答。
    ///
    /// # 一次 `step` = 一个 undo 步
    ///
    /// 过闸之后整条转移包在一个 `store.batch` 里，产出的全部 `Change` 攒成一条
    /// `Entry`（`EntryMeta { turn_id, epoch, label, barrier }`）。非法转移
    /// （25 格 `ProtocolViolation`）不写任何 primitive，于是 `changes` 为空、
    /// `History::append` 拒绝空步——「状态不变」在日志这一侧也是结构事实。
    ///
    /// # agent 闸（028）：路由 + 活性
    ///
    /// 事件的 `agent` 字段（001 就有）从 028 起**真正路由**：这一步写的是那个
    /// agent 的槽位，产出的 effect 也带着它。于是每个 agent 的轮状态（status /
    /// 工具槽 / 预算）天然独立——不是「每个 agent 一份 TurnState」，是同一张原子图
    /// 上按 `AgentId` 分开的整份 `Slot::ALL`。
    ///
    /// 但**路由权不交给宿主**：事件指向的 agent 不在本会话的活名单上
    /// （[`Session::is_live`]）就直接丢弃，跟 epoch 闸一样返回空 `Vec`、不写
    /// primitive、不落 entry、不发通报。
    ///
    /// 不发通报是刻意的，理由和 epoch 闸同源：**这是正常现象**。一个子 agent 被
    /// despawn（或它的 spawn 那一轮被 undo）之后，它在飞的工具回执会陆续到达，
    /// 每条都喊一声只会刷屏。真正的宿主 bug（拼错 `AgentId`）在这里同样是静默的
    /// ——代价是一条事件被丢，而给它开一条会报错的路，就等于给「过期回执」也开了
    /// 一条，两者在类型上分不开。
    ///
    /// root 永远活着，所以单 agent 路径上这道闸恒真：M2 的行为一格不变。
    ///
    /// # 其余转移
    ///
    /// 完整的 5 态 × 7 变体转移表见 `transitions` 模块文档，语义就是 002/016/003
    /// 定下的那一张，一格不差。
    pub fn step(&mut self, event: Event) -> Vec<Effect> {
        // epoch 只增不减，所以「不等于当前」就等价于「过期」。
        if event.epoch().is_some_and(|e| e != self.epoch) {
            return Vec::new();
        }
        let agent = event.agent().clone();
        if !self.is_live(&agent) {
            return Vec::new();
        }

        let label = transitions::label_of(&event);
        self.commit_as(&agent, label, |txn| transitions::transition(txn, event))
    }
}

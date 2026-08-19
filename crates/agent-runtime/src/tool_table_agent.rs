//! 工具表的**子 agent 一族**：`spawn` / `status` / `collect` / `send` / `self` /
//! `await` / `notes` 各自的开闸档（029 / 051 / 053 / 206 / 208 / 212 / 209）。
//!
//! # 为什么它们值一个文件
//!
//! 这一族的七个 `with_*` 共享同一整套理由，而那套理由跟「工具表是什么」是两件事：
//!
//! - **每一个是一档独立授权**，不是一个「多 agent 模式」的总开关。部署方决定开
//!   哪几个（`with_shell` / `with_skills` / `with_mcp` 一套规矩）。
//! - **有几组组合是陷阱**，注释里逐条点名：开了 `background` 却不开 `collect`
//!   （领不回结果，全在轮末被拆）、开 `send` 不开 `status`（拿不到别人的 id）。
//! - **顺序是契约**（红线 11）：工具表在 prompt 最前面，新的一律追加在表尾。
//!
//! 把它们留在 `tool_table.rs` 里，「工具表是什么」那句话就得先说完这一族的
//! 授权才说得完（红线 9 的判据：说得清、且不含「和」）。
//!
//! # 这个文件不认识那些工具怎么执行
//!
//! 它只调各自的 `*_spec()` 拿声明。截获（谁真的去跑这次调用）在
//! [`crate::builtin_intercepts`]——**声明与截获同开同关**由那边的
//! `declares()` 判断保证，两处都改才算加了一个内置工具。

use agent_core::AgentLimits;

use crate::await_tool::await_spec;
use crate::collect_tool::collect_spec;
use crate::notes_tool::{notes_set_spec, notes_spec};
use crate::self_tool::self_spec;
use crate::send_tool::send_spec;
use crate::spawn_request::spawn_spec;
use crate::status_tool::status_spec;

use super::ToolTable;

impl ToolTable {
    /// 029 开闸：追加 `srv:agent/spawn`，宿主从此允许模型分解任务（决策 20）。
    ///
    /// **`limits` 必须跟 `Session` 手上那份是同一组数**（`Session::agent_limits`，
    /// 默认都是 [`AgentLimits::default`]）：这里的数字只进工具描述给模型看，真正
    /// 拦人的是 `Session::spawn_child` 里那两道闸。两边不一致不会出错，只会让模型
    /// 收到一句跟描述对不上的拒绝——所以宿主要么两边都用默认值，要么两边传同一个
    /// 值。数字进描述而不是让模型试出来，是为了省掉大部分「试→被拒→重试」的往返。
    ///
    /// 追加在末尾而不是插进 `builtin()` 内部：`builtin_specs()` 的顺序是 013 钉死
    /// 的既有契约，工具表在 prompt 最前面（红线 11），只加不改。
    pub fn with_spawn(mut self, limits: AgentLimits) -> Self {
        self.push_spec(spawn_spec(limits));
        self
    }

    /// 051 开闸：追加 `srv:agent/status`，模型从此能在子 agent 还在跑的时候看它们
    /// 此刻在干啥（M8，docs/ORCHESTRATION.md §三）。
    ///
    /// **跟 `with_spawn` 分开两个开关**（而不是塞进它）：每个 `with_*` 是一档独立的
    /// 授权，跟 `with_shell`/`with_skills`/`with_mcp` 一套规矩；而且工具表在 prompt
    /// 最前面（红线 11），把 status 折进 `with_spawn` 会让所有既有宿主的前缀无声
    /// 变一次。只开 status 不开 spawn 是**合法但没用**的组合（永远没有后代可看），
    /// 宿主该两个一起开——`agent-cli` / `ToolTableSpec::Full` 就是这么接的。
    ///
    /// 追加在末尾（红线 11：既有顺序是契约，只加不改）。宿主的链式顺序决定它落在
    /// 哪一格，`agent-cli` 把它紧跟在 `with_spawn` 之后、`with_skills`/`with_mcp`
    /// 之前：这样「静态那一段」在所有会话里逐字节相同，不随装了几个 skill / 几个
    /// MCP 工具而移位。
    pub fn with_status(mut self) -> Self {
        self.push_spec(status_spec());
        self
    }

    /// 053 开闸：追加 `srv:agent/collect`，模型从此能**择时**领后台子 agent 的结果
    /// （M8 闭环的最后一格，docs/ORCHESTRATION.md §三）。
    ///
    /// 跟 `with_spawn`/`with_status` 各自一档，理由同 `with_status`。只开 collect
    /// 不开 spawn 是**合法但没用**的组合（永远没有后台子可领）；反过来——开了
    /// spawn 不开 collect——才是真的会咬人：模型看得见 `background=true` 却没有
    /// 任何办法把结果拿回来，发出去的子全部在轮末被拆掉。宿主要么三个一起开，
    /// 要么把 spawn 也关掉。
    ///
    /// 追加在末尾（红线 11：既有顺序是契约，只加不改）。
    pub fn with_collect(mut self) -> Self {
        self.push_spec(collect_spec());
        self
    }

    /// 206 开闸：追加 `srv:agent/send`，模型从此能给会话里**任意**活 agent 说一句话
    /// （决策 35 §二，横读全开之后兄弟也发得到）。
    ///
    /// 跟 `with_spawn`/`with_status`/`with_collect` 各自一档，理由同 `with_status`：
    /// 部署方决定开哪些，不是一个「多 agent 模式」的总开关。**只开 send 不开 status
    /// 是合法但难用的组合**——模型拿不到别人的 id，只能靠自己 spawn 时记下的那些。
    pub fn with_send(mut self) -> Self {
        self.push_spec(send_spec());
        self
    }

    /// 208 开闸：追加 `srv:agent/self`，模型从此看得到**自己**还剩多少额度
    /// （决策 35 §三）。
    ///
    /// 跟 `with_status` 各自一档，理由同上——但这一档跟别的都不同：它**一个
    /// 子 agent 都不需要**就有用。单 agent 会话开它也划算（模型知道还剩几次
    /// 请求，就能在被 `TurnGuard` 切断之前把结论说出来），所以别把它当成
    /// 「多 agent 三件套」的第四件。
    ///
    /// 追加在末尾（红线 11：既有顺序是契约，只加不改）。
    pub fn with_self(mut self) -> Self {
        self.push_spec(self_spec());
        self
    }

    /// 209 开闸：追加 `srv:agent/notes` + `srv:agent/notes/set`，模型从此有一张
    /// 属于自己的草稿纸（决策 35 §三）。
    ///
    /// **一档开两个工具**，跟别的 `with_*` 刻意不同（照 `with_skills` 的先例：
    /// 那一档也是 `read` + `index` 一起）。判据是「一档是一件事」而不是
    /// 「一档是一个工具」：只读不写的草稿纸永远是空的，只写不读的记了没人看得见
    /// ——两个半边各自都没有意义，把它们拆成两档只会造出两种没用的组合。
    ///
    /// 跟这一族别的档一样，**它也不需要任何子 agent**：单 agent 会话开它就有用。
    ///
    /// 追加在末尾（红线 11：既有顺序是契约，只加不改）。
    pub fn with_notes(mut self) -> Self {
        self.push_spec(notes_spec());
        self.push_spec(notes_set_spec());
        self
    }

    /// 212 开闸：追加 `srv:agent/await`，模型从此能挂起等另一个 agent（含兄弟）
    /// 到达某个状态（决策 35 §一）。
    ///
    /// 跟 `with_collect` 各自一档：**它们不是一回事**。`collect` 领的是「我自己
    /// 开的后台子」的正文（领取即消费），`await` 只回答「它到了没有」、不给正文，
    /// 而且等得了**不归你领的**——兄弟，或者别人开的。只开 await 不开 collect 是
    /// 合法但难用的组合：模型知道对方到了，却没有办法拿到它的答案。
    ///
    /// 追加在末尾（红线 11：既有顺序是契约，只加不改）。
    pub fn with_await(mut self) -> Self {
        self.push_spec(await_spec());
        self
    }
}

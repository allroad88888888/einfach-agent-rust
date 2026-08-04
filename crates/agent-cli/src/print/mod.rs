//! 终端输出。**这是这个 CLI 唯一允许直接 `print!`/`println!`/`eprintln!` 的
//! 地方**——`repl`/`undo`/`model_switch` 只调这里的函数，格式改动不该扩散到编排
//! 逻辑里（跟 022 定的规矩一致）。
//!
//! 里面是两件事，029 把它们拆成两个文件：
//!
//! | 文件 | 管什么 |
//! |---|---|
//! | [`events`] | **事件流**：`agent_runtime::AgentEvent` → 终端。有状态（流式增量的颜色/换行/归属换人），一个会话一份 |
//! | [`receipts`] | **命令回执**：`/model` `/undo` `/redo`、启动恢复、一轮收尾。全是无状态的一次性文案 |
//! | [`agent_tree`] | **`/agents`**（047）：`agent_core::AgentTree` → 缩进文本。跟前两个一样是纯函数，跟 `receipts` 的区别只是入参不是标量而是一整棵树 |
//! | [`mcp`] | **`/mcp`**（045）：`crate::mcp::McpStatus` → 缩进文本（server 可用性 + 工具名）。纯函数，跟 `agent_tree` 同类，入参是装载期状态快照 |
//!
//! 拆开的界线是「有没有状态」：`EventPrinter` 是个状态机（上一段是思考还是正文、
//! 上一句是谁说的），回执文案和树渲染都是一串纯函数。029 之前它们挤在一个文件里
//! 恰好还没顶破行数上限，加上多 agent 归属之后顶破了——按职责拆，不是按行数拆。

mod agent_tree;
mod events;
mod mcp;
mod receipts;

pub use agent_tree::render_agent_tree;
pub use events::EventPrinter;
pub use mcp::render_mcp_status;
pub use receipts::{
    cancelled_turn_erased, cancelled_turn_kept, model_switch_error, model_switched, recovery_failed,
    redo_applied, redo_nothing, session_recovered, turn_outcome, undo_applied, undo_blocked,
    undo_force_crossed, undo_nothing, unresolved_tool_call_notice,
};

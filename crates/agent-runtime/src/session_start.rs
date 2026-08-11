//! 135：会话「新建」路径的开局驱动。
//!
//! 会话新建、`ToolTable` 装配完成之后、第一轮之前：按注册顺序执行
//! `tools.timed(CallTiming::SessionStart)` 的每一条（133 的执行体形状——本地
//! 同步函数，不走 dispatch/executor/远端等待槽，见 [`crate::tool_table::timed`]
//! 模块文档「执行体是注册时给的本地函数」），把非空文本结果拼成
//! [`agent_core::SystemChunk`]，一次性 `Session::set_prefix_chunks`（134）。
//!
//! **只有新建会话才跑这条路**——恢复出来的会话，前缀值已经在 134 的状态里
//! （journaled、从日志重放自动回来）。重跑一遍等于用「这一刻的外部世界」
//! 覆盖「那一刻的外部世界」给出的答案：历史对话是在旧答案下产生的，两者对
//! 不上就是一次静默的语境错位（134 `command/prefix.rs` 模块文档已经论证过这
//! 一条，这里不重复）。宿主接线（`agent-cli`/`agent-server`）因此只在各自
//! 「这是一个全新会话」的分支调用这个函数，恢复分支一行都不加。
//!
//! # 全有或全无
//!
//! 任一执行体 `Err` → 立刻返回，此前收集的块**一个都不写**（`set_prefix_chunks`
//! 压根不会被调）。半份前缀比整段缺席更危险：它看着像样——毕竟前面几个工具
//! 是成功的——只是缺一段，而且这半份错值会在此后**每一轮**里原样重发进
//! prompt，不会有第二次机会暴露自己是错的（跟红线 11「不报错、只是每次都
//! 不对」是同一种病）。调用方看到 `Err` 就该让整个会话创建失败，不该拿一个
//! 半成品状态开局——`agent-cli`/`agent-server` 的接线都是这么处理的。
//!
//! # 顺序与内容的确定性（红线 11）
//!
//! 迭代顺序由 [`crate::ToolTable::timed`] 保证（133：`Vec` push 顺序即注册
//! 顺序，`timed()` 只按时机过滤、不重排），这里只是原样消费，不额外排序、
//! 不经过任何无序容器中转。

use std::sync::Arc;

use agent_core::{Session, SystemChunk};
use serde_json::Value;

use crate::tool_table::{CallTiming, ToolTable};

/// 一个开局工具执行失败。`tool` 是它的全名，`message` 是执行体给的错误文本
/// （[`crate::TimedRun`] 的 `Err` 分支原样带出来，不加工）。
#[derive(Debug)]
pub struct SessionStartError {
    pub tool: Arc<str>,
    pub message: Arc<str>,
}

/// 会话「新建」路径专用：按注册顺序执行 `SessionStart` 时机工具，结果一次性
/// 落前缀块。**恢复路径不许调它**——模块文档「只有新建会话才跑这条路」。
pub fn run_session_start(
    session: &mut Session,
    tools: &ToolTable,
) -> Result<(), SessionStartError> {
    let mut chunks = Vec::new();
    for entry in tools.timed(CallTiming::SessionStart) {
        let name = Arc::clone(&entry.spec().name);
        match entry.run(tools, &Value::Null) {
            Ok(text) if text.is_empty() => {
                // 空文本不产块——不白占一段前缀（issue 135 §做什么 第 1 条）。
            }
            Ok(text) => chunks.push(SystemChunk {
                label: Arc::from(format!("init:{name}")),
                text,
            }),
            Err(message) => {
                return Err(SessionStartError {
                    tool: name,
                    message,
                })
            }
        }
    }
    // 全部成功才走到这里；空集也调（134 保证空写无痕：值跟默认值相等，
    // `record_set` 不产生 `Change`，日志不多一条幽灵 entry）。
    session.set_prefix_chunks(chunks);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use agent_core::{AgentId, ToolSpec};

    fn raw_spec(name: &str) -> ToolSpec {
        ToolSpec {
            name: Arc::from(name),
            description: Arc::from(format!("{name} 的说明书")),
            schema: Arc::new(serde_json::json!({ "type": "object" })),
        }
    }

    fn echo_run(reply: &'static str) -> crate::TimedRun {
        Box::new(move |_table, _input| Ok(Arc::from(reply)))
    }

    fn empty_run() -> crate::TimedRun {
        Box::new(|_table, _input| Ok(Arc::from("")))
    }

    fn fail_run(message: &'static str) -> crate::TimedRun {
        Box::new(move |_table, _input| Err(Arc::from(message)))
    }

    /// 两个 fake 工具按注册顺序进前缀块；交换注册顺序，前缀块的先后跟着换。
    #[test]
    fn two_fakes_land_in_registration_order_and_follow_a_swap() {
        let mut session = Session::new(AgentId::root());
        let table = ToolTable::builtin()
            .with_timed(
                raw_spec("srv:init/a"),
                CallTiming::SessionStart,
                echo_run("A"),
            )
            .with_timed(
                raw_spec("srv:init/b"),
                CallTiming::SessionStart,
                echo_run("B"),
            );
        run_session_start(&mut session, &table).expect("两个都成功");
        let chunks = session.prefix_chunks();
        assert_eq!(chunks.len(), 2);
        assert_eq!(&*chunks[0].label, "init:srv:init/a");
        assert_eq!(&*chunks[0].text, "A");
        assert_eq!(&*chunks[1].label, "init:srv:init/b");
        assert_eq!(&*chunks[1].text, "B");

        let mut swapped_session = Session::new(AgentId::root());
        let swapped = ToolTable::builtin()
            .with_timed(
                raw_spec("srv:init/b"),
                CallTiming::SessionStart,
                echo_run("B"),
            )
            .with_timed(
                raw_spec("srv:init/a"),
                CallTiming::SessionStart,
                echo_run("A"),
            );
        run_session_start(&mut swapped_session, &swapped).expect("两个都成功");
        let swapped_chunks = swapped_session.prefix_chunks();
        assert_eq!(&*swapped_chunks[0].label, "init:srv:init/b");
        assert_eq!(&*swapped_chunks[1].label, "init:srv:init/a");
    }

    /// 空文本结果不产块——不白占一段前缀。
    #[test]
    fn empty_text_result_is_skipped_not_a_placeholder() {
        let mut session = Session::new(AgentId::root());
        let table = ToolTable::builtin()
            .with_timed(
                raw_spec("srv:init/quiet"),
                CallTiming::SessionStart,
                empty_run(),
            )
            .with_timed(
                raw_spec("srv:init/loud"),
                CallTiming::SessionStart,
                echo_run("hi"),
            );
        run_session_start(&mut session, &table).expect("成功");
        let chunks = session.prefix_chunks();
        assert_eq!(chunks.len(), 1, "安静的那个不该占位");
        assert_eq!(&*chunks[0].label, "init:srv:init/loud");
    }

    /// 任一执行体 `Err` → 全有或全无：不写任何前缀块，也不留一条 journal entry
    /// （`set_prefix_chunks` 压根没被调，history 里没有 `prefix_init`）。
    #[test]
    fn any_failure_writes_nothing_at_all() {
        let mut session = Session::new(AgentId::root());
        let before = session.history_len();
        let table = ToolTable::builtin()
            .with_timed(
                raw_spec("srv:init/ok"),
                CallTiming::SessionStart,
                echo_run("ok"),
            )
            .with_timed(
                raw_spec("srv:init/boom"),
                CallTiming::SessionStart,
                fail_run("挂了"),
            );
        let err = run_session_start(&mut session, &table).expect_err("第二个失败了");
        assert_eq!(&*err.tool, "srv:init/boom");
        assert_eq!(&*err.message, "挂了");
        assert!(
            session.prefix_chunks().is_empty(),
            "第一个成功的那个也不该单独落地"
        );
        assert_eq!(
            session.history_len(),
            before,
            "失败时不该留下任何 journal entry（不多不少，包括不留一条半份的）"
        );
    }
}

//! 136：完成轮之后的收尾驱动 —— `timed(CallTiming::TurnEnd)`。
//!
//! 每个**正常完成**的轮（`TurnStatus::Done { .. }`）之后，按注册顺序执行
//! `tools.timed(CallTiming::TurnEnd)` 的每一条（133 的执行体形状：本地同步
//! 函数，不走 dispatch/executor/远端等待槽，见 `crate::tool_table` 里
//! `timed` 子模块的文档「执行体是注册时给的本地函数」）。跟 135 的
//! [`crate::session_start`] 用的是同一套执行体协议，但收工方式完全相反——那边
//! 「全有或全无」（结果要拼成前缀块，半份比不写更危险），这里是**纯副作用，
//! 结果丢弃**：
//!
//! - **不回灌**：`Ok` 的文本没有地方可去，v1 没有「结果进下一轮 prompt」这个
//!   通道。
//! - **不能续 loop**：跑完就完，不会因为一条 timed 工具而让泵多转一圈、也不
//!   会让这一轮的 `TurnStatus` 变成别的样子——`fire` 在调用方判完终态、
//!   已经准备 `return Ok(status)` 的那一刻才跑，`status` 早就定型了。
//! - **不发协议事件**：调用方（[`crate::runner`]）压根不知道这里跑过，
//!   `RunnerEvent` 一个字节都不会因为它多发。
//! - **不写 store**：从头到尾没有一处 `Session::step`/command 调用——这是红线
//!   2 最安全的遵守方式：不是「小心翼翼地只走正门」，是根本没有写入路径，
//!   连「绕过 command 层」这个问题都无从被问起。
//!
//! 这三条边界是 issue 136 决策 27 明确拍死的 v1 范围。将来要支持「hook 拦停
//! 一轮 / 结果进下一轮 / SSE 上能看到 hook 跑过」，单开 issue 重议——那会碰到
//! 001 的 loop 契约，复杂度跟这里完全不是一回事，不能顺手在这个函数里加一个
//! 分支了事。
//!
//! # 一条失败不挡下一条，只记日志
//!
//! `tools.timed(TurnEnd)` 是一批相互独立的钩子，不是像 135 那样要拼成一份
//! 结果的流水线，所以这里**不是**「全有或全无」：某一条执行体 `Err`，只记一
//! 条日志，照样接着跑下一条——不影响轮的结果（轮已经在调用方落定）、不
//! panic、不因为一条失败就让其它本该跑的钩子也陪着不跑。
//!
//! # 审计面：这段副作用不进 command log（故意，且知情）
//!
//! timed 工具的执行体不产出任何 `Effect`/`Event`，所以它做的事（如果它自己
//! 碰了外部世界——比如发一条通知、写一份外部日志）不会在 undo 历史或审计
//! 回放里留下一条 entry。这跟 docs/TOOLS.md §「服务端工具不是第四种
//! Location」点名的是同一种警觉：**「这件事发生过，但账本上找不到」**看着
//! 像一个漏洞。
//!
//! 区别在于知情前提。服务端工具站在模型的操作面上：模型能看见它、能选择
//! 调它，账本却记不下它的效果——这是「模型以为在记账、其实没记」的错位，
//! 危险在于它是隐藏的。TurnEnd timed 工具从不在模型的操作面上
//! （`declares()`/`specs()` 一个字节看不见 timed 区，见 `crate::tool_table`
//! 的 `timed` 子模块文档）——它是**部署者显式装配**进 `ToolTable` 的一个
//! 钩子：谁装的、装了什么、为什么不进账本，都是部署者自己当时就能看见的
//! 决定，不是运行时替他做的隐藏选择。半年后的读者看到这里如果以为是「漏记
//! 了」——这段注释就是留给那个读者的：不是漏，是设计，理由就是上面这段。
//!
//! # 挂点也是远端工具回传的续跑路（`resume_async`），这是刻意的
//!
//! `fire` 从 [`crate::runner`] 的收工分支（在飞表两张都空、`status.is_
//! terminal()` 之后）调用，那个分支不只是 `run_turn`/`run_turn_async` 的
//! 出口——它同样是远端工具结果回传之后 `resume_async` 续跑到底的出口（
//! `crate::remote_tool_submission` 的 `submit_remote_tool_result*` 最终都
//! 经这条路收敛）。所以一轮如果是被远端工具的结果**补完**的（模型上一次停
//! 在 `ToolsPending`，宿主喂回结果之后这一轮才落 `Done`），一样会触发一次
//! `TurnEnd`。这不是遗漏——「每个正常完成的轮」这句话本身不区分它是被哪条
//! 事件补完的，两条路殊途同归地把 `status` 判成 `Done` 之后没有理由区别对待。
//!
//! # 日志手段：`tracing`，不带 subscriber
//!
//! 这个 crate 在 136 落地之前没有任何日志设施（`rg 'tracing::|log::'` 全空）。
//! 选 `tracing` 而不是自己拿 `eprintln!` 糊一个：`tracing` 已经是工作区里被
//! 验证过的选择——`agent-server`/`agent-server-bin` 两个宿主已经在用它
//! （各自 `tracing = "0.1"`），也已经解析进 `Cargo.lock`（`tracing 0.1.44`），
//! 这里再加一份同版本号的声明不引入新的依赖树、不需要版本仲裁。
//!
//! 不引入 `tracing-subscriber`：宿主自己决定要不要装、往哪儿转发——
//! `agent-server` 已经装了，装完之后这条日志立刻可观测，不用等以后再补一层
//! 转发；`agent-cli` 没装 subscriber 时，`tracing` 的调用点是零成本的空操作
//! （宏内部先查全局 filter，没有 subscriber 直接短路返回），不会在 CLI 里
//! 意外打印一堆没人要看的行，也不会因为没人订阅就报错——这正是「失败只记
//! 日志」这句话要的语义：日志是给愿意看的宿主看的，不是每个宿主都被迫看到。
//!
//! `agent-runtime` 还要编 wasm32：`tracing` 核心（不含任何 subscriber）在
//! `wasm32-unknown-unknown` 上编译干净，不带来这里没验证过的东西——需要验证
//! 的是「一份 subscriber 实现能不能在浏览器目标上编译」，而这个 crate 从不
//! 引入 subscriber，那份验证需求根本不存在。

use agent_core::Session;

use crate::ctx::RunnerCtx;
use crate::tool_table::CallTiming;

/// 一轮**正常完成**之后跑一遍 `TurnEnd` 时机的工具，按注册顺序，结果全部
/// 丢弃、失败只记日志。
///
/// 调用方必须已经确认 `session.status()` 落在 `TurnStatus::Done { .. }`——
/// 这个函数本身不做状态判断（判断属于挂点的责任，见 [`crate::runner`] 里
/// 调用它的那一行），传进来就无条件跑一遍这份 `TurnEnd` 区。`session` 是
/// **只读**借用（153，决策 30）——调用方手里本来就攥着它，递进去而已；类型上
/// 这里写不了状态，`ext:stats/audit` 那类钩子在轮末现读账本靠的就是这个参数。
pub(crate) fn fire(ctx: &RunnerCtx, session: &Session) {
    for tool in ctx.tools.timed(CallTiming::TurnEnd) {
        if let Err(message) = tool.run(&ctx.tools, session, &serde_json::Value::Null) {
            tracing::warn!(
                tool = %tool.spec().name,
                error = %message,
                "TurnEnd timed 工具执行失败，忽略（v1 边界：结果丢弃，不影响这一轮）"
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex};

    use agent_core::{AgentId, Session, SessionConfig, ToolSpec};
    use agent_providers::deepseek::DeepSeek;
    use agent_tools::ToolExecutor;
    use agent_transport::Client;
    use serde_json::Value;

    use super::*;
    use crate::tool_table::{TimedRun, ToolTable};

    fn raw_spec(name: &str) -> ToolSpec {
        ToolSpec {
            name: Arc::from(name),
            description: Arc::from(format!("{name} 的说明书")),
            schema: Arc::new(serde_json::json!({ "type": "object" })),
        }
    }

    /// 每次跑都把自己的名字记进共享日志，再回一句固定的话——用来在测试里
    /// 断言「谁、按什么顺序」真的被调用过。`TimedRun` 是 `Fn`（不是
    /// `FnMut`），记账靠 `Arc<Mutex<..>>` 的内部可变性，同 133/135 测试里
    /// 需要跨调用留痕时的一贯写法。
    fn recording_run(log: Arc<Mutex<Vec<&'static str>>>, name: &'static str) -> TimedRun {
        Box::new(move |_table, _session, _input: &Value| {
            log.lock().unwrap().push(name);
            Ok(Arc::from("ok"))
        })
    }

    /// 同上，但记完账之后回 `Err`——验证失败不挡后续同批工具继续跑。
    fn recording_fail_run(log: Arc<Mutex<Vec<&'static str>>>, name: &'static str) -> TimedRun {
        Box::new(move |_table, _session, _input: &Value| {
            log.lock().unwrap().push(name);
            Err(Arc::from("挂了"))
        })
    }

    fn build_ctx(table: ToolTable) -> RunnerCtx {
        let fs = ToolExecutor::new(std::env::temp_dir()).unwrap();
        RunnerCtx::new(
            Arc::new(DeepSeek),
            Arc::new(Client::new()),
            "https://api.deepseek.com/chat/completions".to_string(),
            "deepseek-key".to_string(),
            fs,
            table,
            Vec::new(),
            SessionConfig {
                model: Arc::from("deepseek-v4-pro"),
                temperature: None,
                max_tokens: None,
                context_window: None,
            },
            crate::persist::open_backend(None, |_| {}),
            Box::new(|_| {}),
        )
    }

    /// 验收要点第一条：`fire` 按注册顺序调用 `TurnEnd` 区的每一条，交换注册
    /// 顺序，调用顺序跟着换——跟 [`crate::tool_table`] 的 `timed()`（只过滤、
    /// 不重排）是同一句承诺，这里钉住「驱动这一层没有偷偷插入别的顺序」。
    #[test]
    fn fire_runs_turn_end_tools_in_registration_order() {
        let log = Arc::new(Mutex::new(Vec::new()));
        let session = Session::new(AgentId::root());

        let forward = ToolTable::builtin()
            .with_timed(
                raw_spec("srv:end/a"),
                CallTiming::TurnEnd,
                recording_run(Arc::clone(&log), "a"),
            )
            .with_timed(
                raw_spec("srv:end/b"),
                CallTiming::TurnEnd,
                recording_run(Arc::clone(&log), "b"),
            );
        fire(&build_ctx(forward), &session);
        assert_eq!(*log.lock().unwrap(), vec!["a", "b"]);

        log.lock().unwrap().clear();
        let swapped = ToolTable::builtin()
            .with_timed(
                raw_spec("srv:end/b"),
                CallTiming::TurnEnd,
                recording_run(Arc::clone(&log), "b"),
            )
            .with_timed(
                raw_spec("srv:end/a"),
                CallTiming::TurnEnd,
                recording_run(Arc::clone(&log), "a"),
            );
        fire(&build_ctx(swapped), &session);
        assert_eq!(*log.lock().unwrap(), vec!["b", "a"]);
    }

    /// 验收要点第二条：一条执行体 `Err` 不 panic，也不挡同一批里排在它后面
    /// 的其它 `TurnEnd` 工具——三个钩子里中间那个失败，第一个和第三个仍然
    /// 各跑一次（`fire` 不是 135 那种「全有或全无」）。
    #[test]
    fn fire_does_not_panic_on_err_and_keeps_running_the_rest() {
        let log = Arc::new(Mutex::new(Vec::new()));
        let session = Session::new(AgentId::root());

        let table = ToolTable::builtin()
            .with_timed(
                raw_spec("srv:end/ok1"),
                CallTiming::TurnEnd,
                recording_run(Arc::clone(&log), "ok1"),
            )
            .with_timed(
                raw_spec("srv:end/boom"),
                CallTiming::TurnEnd,
                recording_fail_run(Arc::clone(&log), "boom"),
            )
            .with_timed(
                raw_spec("srv:end/ok2"),
                CallTiming::TurnEnd,
                recording_run(Arc::clone(&log), "ok2"),
            );

        fire(&build_ctx(table), &session);

        assert_eq!(*log.lock().unwrap(), vec!["ok1", "boom", "ok2"]);
    }

    /// 没有注册任何 `TurnEnd` 工具时，`fire` 是安静的空操作——不 panic，也不
    /// 误跑到 `SessionStart` 区的条目（两个时机互不干扰，同
    /// `tool_table_timed_tests.rs` 的「按时机过滤」验收）。
    #[test]
    fn fire_is_a_no_op_when_turn_end_region_is_empty_and_ignores_session_start_entries() {
        let log = Arc::new(Mutex::new(Vec::new()));
        let session = Session::new(AgentId::root());
        let table = ToolTable::builtin().with_timed(
            raw_spec("srv:init/only"),
            CallTiming::SessionStart,
            recording_run(Arc::clone(&log), "should-not-run"),
        );

        fire(&build_ctx(table), &session);

        assert!(log.lock().unwrap().is_empty());
    }
}

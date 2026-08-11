//! `ToolTable` 的 timed 区单测（133）。`#[path]` 子模块，`super` 是 `tool_table_timed`
//! 本身——`ToolTable`/`Value`/`Arc` 都经它的 `use` 语句透传进来，模式同
//! `tool_table_tests.rs`（见那边文件头注释）。

use super::*;
use agent_core::AgentLimits;

fn raw_spec(name: &str) -> ToolSpec {
    ToolSpec {
        name: Arc::from(name),
        description: Arc::from(format!("{name} 的说明书")),
        schema: Arc::new(serde_json::json!({ "type": "object" })),
    }
}

fn echo_run(reply: &'static str) -> TimedRun {
    Box::new(move |_table, _input| Ok(Arc::from(reply)))
}

/// 验收第一条：timed 工具不进 `specs()`，`declares()` 也认不出它——
/// 跟 076 disable 判据同一句话，「表里有什么」和「模型看得见什么」必须是同一个答案。
#[test]
fn timed_tools_are_invisible_to_specs_and_declares() {
    let plain = ToolTable::builtin();
    let with_timed = ToolTable::builtin().with_timed(
        raw_spec("srv:index/refresh"),
        CallTiming::SessionStart,
        echo_run("ok"),
    );

    assert_eq!(
        serde_json::to_string(plain.specs()).unwrap(),
        serde_json::to_string(with_timed.specs()).unwrap(),
        "timed 区非空不该动 specs 一个字节（红线 11 看门狗）"
    );
    assert!(!with_timed.declares("srv:index/refresh"));
}

/// 验收第二条：`timed(timing)` 按注册顺序迭代；交换两个 timed 工具的注册顺序，
/// 迭代顺序跟着换——`Vec` push 顺序即注册顺序，`timed()` 只过滤不重排。
#[test]
fn timed_iterates_in_registration_order_and_follows_it_when_swapped() {
    let forward = ToolTable::builtin()
        .with_timed(raw_spec("srv:a"), CallTiming::SessionStart, echo_run("a"))
        .with_timed(raw_spec("srv:b"), CallTiming::SessionStart, echo_run("b"));
    let forward_names: Vec<&str> = forward
        .timed(CallTiming::SessionStart)
        .map(|t| &*t.spec().name)
        .collect();
    assert_eq!(forward_names, vec!["srv:a", "srv:b"]);

    let swapped = ToolTable::builtin()
        .with_timed(raw_spec("srv:b"), CallTiming::SessionStart, echo_run("b"))
        .with_timed(raw_spec("srv:a"), CallTiming::SessionStart, echo_run("a"));
    let swapped_names: Vec<&str> = swapped
        .timed(CallTiming::SessionStart)
        .map(|t| &*t.spec().name)
        .collect();
    assert_eq!(swapped_names, vec!["srv:b", "srv:a"]);
}

/// `timed()` 只按时机过滤，不同时机的工具互不干扰，也不改变各自内部的相对顺序。
#[test]
fn timed_filters_by_timing_and_leaves_the_other_bucket_untouched() {
    let table = ToolTable::builtin()
        .with_timed(
            raw_spec("srv:start/one"),
            CallTiming::SessionStart,
            echo_run("s1"),
        )
        .with_timed(
            raw_spec("srv:end/one"),
            CallTiming::TurnEnd,
            echo_run("e1"),
        )
        .with_timed(
            raw_spec("srv:start/two"),
            CallTiming::SessionStart,
            echo_run("s2"),
        );

    let starts: Vec<&str> = table
        .timed(CallTiming::SessionStart)
        .map(|t| &*t.spec().name)
        .collect();
    assert_eq!(starts, vec!["srv:start/one", "srv:start/two"]);

    let ends: Vec<&str> = table
        .timed(CallTiming::TurnEnd)
        .map(|t| &*t.spec().name)
        .collect();
    assert_eq!(ends, vec!["srv:end/one"]);
}

/// 执行体拿到的是它注册所在的那张表本身，能读表内数据（138 的索引函数要用这个）。
#[test]
fn timed_tool_run_can_read_the_table_it_is_registered_on() {
    let table = ToolTable::builtin().with_timed(
        raw_spec("srv:index/refresh"),
        CallTiming::SessionStart,
        Box::new(|table: &ToolTable, _input: &Value| {
            Ok(Arc::from(table.specs().len().to_string()))
        }),
    );

    let tool = table.timed(CallTiming::SessionStart).next().unwrap();
    let result = tool.run(&table, &Value::Null).unwrap();
    assert_eq!(&*result, table.specs().len().to_string());
}

/// 撞名第一向：timed 名撞了 specs 区已有的名字——`with_timed` 整条丢弃，
/// debug 构建下 `debug_assert!` 点名。
#[test]
#[should_panic(expected = "srv:fs/read")]
fn with_timed_panics_in_debug_when_colliding_with_a_spec_name() {
    let _ = ToolTable::builtin().with_timed(
        raw_spec("srv:fs/read"),
        CallTiming::SessionStart,
        echo_run("boom"),
    );
}

/// 撞名第一向的 release 分支：同一次调用在没有 `debug_assert!` 的构建下不 panic，
/// timed 区也不会多出这一条。用 `catch_unwind` 兼顾两种构建（同 `tool_table_tests.rs`
/// 里 `push_spec_leaves_specs_untouched_when_the_name_already_exists` 的先例）。
#[test]
fn with_timed_leaves_timed_region_untouched_when_colliding_with_a_spec_name() {
    let build = || {
        ToolTable::builtin().with_timed(
            raw_spec("srv:fs/read"),
            CallTiming::SessionStart,
            echo_run("boom"),
        )
    };
    let result = std::panic::catch_unwind(build);
    if cfg!(debug_assertions) {
        assert!(result.is_err(), "debug 构建下撞名必须 panic");
    } else {
        let table = result.expect("release 构建下 with_timed 不该 panic");
        assert_eq!(table.timed(CallTiming::SessionStart).count(), 0);
    }
}

/// 撞名第二向：两条 timed 工具自己撞名——同一条判据，同一个 `debug_assert!`。
#[test]
#[should_panic(expected = "srv:dup/timed")]
fn with_timed_panics_in_debug_when_colliding_within_the_timed_region() {
    let _ = ToolTable::builtin()
        .with_timed(
            raw_spec("srv:dup/timed"),
            CallTiming::SessionStart,
            echo_run("first"),
        )
        .with_timed(
            raw_spec("srv:dup/timed"),
            CallTiming::TurnEnd,
            echo_run("second"),
        );
}

/// 撞名第三向（反向）：timed 区先注册了一个名字，`push_spec` 那条路（`with_spawn`
/// 等）后来才想用同一个名字——两者的调用顺序不是任何一个函数能控制的，所以
/// `push_spec` 也要查 timed 区，不能只让 `with_timed` 单向查 specs 区。
#[test]
#[should_panic(expected = "srv:agent/spawn")]
fn push_spec_panics_in_debug_when_colliding_with_an_already_registered_timed_name() {
    let table = ToolTable::builtin().with_timed(
        raw_spec("srv:agent/spawn"),
        CallTiming::SessionStart,
        echo_run("boom"),
    );
    let _ = table.with_spawn(AgentLimits::default());
}

/// 上一条的 release 分支：`push_spec` 反向撞名时，specs 区不会真的多出这一条，
/// timed 区里先注册的那条也原样留着。
#[test]
fn push_spec_leaves_specs_untouched_when_colliding_with_an_already_registered_timed_name() {
    let build = || {
        let table = ToolTable::builtin().with_timed(
            raw_spec("srv:agent/spawn"),
            CallTiming::SessionStart,
            echo_run("boom"),
        );
        table.with_spawn(AgentLimits::default())
    };
    let result = std::panic::catch_unwind(build);
    if cfg!(debug_assertions) {
        assert!(result.is_err(), "debug 构建下撞名必须 panic");
    } else {
        let table = result.expect("release 构建下 push_spec 不该 panic");
        assert!(
            !table.declares("srv:agent/spawn"),
            "specs 区不该真的多出撞名的这一条"
        );
        assert_eq!(table.timed(CallTiming::SessionStart).count(), 1);
    }
}

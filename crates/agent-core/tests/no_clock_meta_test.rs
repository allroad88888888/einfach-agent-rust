//! 验收 9：无时钟元测试。转移表必须不读时钟——超时是外部注入的 `Timeout` 事件，
//! 计时器活在 core 外面（001 验收原文：「core 里没有 `Instant::now()`」）。这条用
//! `grep` 而不是编译期检查：要抓的是「压根没调用」，不是某个类型的形状，静态分析
//! 抓不到「函数体里完全没有某个调用」这件事本身，只有全文搜索能直接回答。
//!
//! 026 起扫三个目录：`engine/`（M1 那一路）、`command/`（`Session` 这一路）、
//! `graph/`（**红线 1 的落点**——derived 的 read fn 住在那里，重放要能得出同样的
//! 结果，读了时钟就得不出）。`scripts/check-invariants.sh` 的
//! `check_derived_purity` 覆盖同一批路径。

use std::path::Path;

#[test]
fn the_transition_tables_and_the_derived_never_read_the_clock() {
    for dir in ["src/engine", "src/command", "src/graph"] {
        assert_no_clock_under(dir);
    }
}

fn assert_no_clock_under(rel: &str) {
    let engine_dir = Path::new(env!("CARGO_MANIFEST_DIR")).join(rel);
    assert!(engine_dir.is_dir(), "目录应该存在：{}", engine_dir.display());

    // 用全限定路径而不是 `use std::process::Command`：这个 crate 的红线 7 检查
    // （scripts/check-invariants.sh）按 `crates/agent-core/*` 整包扫描 `use
    // std::process` 之类的 IO 引用，不区分 src/ 与 tests/。这个断言本身要跑
    // `grep` 子进程，且只存在于 tests/（不进制品、不随库一起编译进任何调用方），
    // 没有违反红线 7 的实际意图（「agent-core 库不做 IO」），只是不触发这条
    // 按文本匹配、没有对 tests/ 开洞的粗筛规则。
    let output = std::process::Command::new("grep")
        .args(["-rn", "-E", "Instant::now|SystemTime::now|rand::|thread_rng"])
        .arg(&engine_dir)
        .output()
        .expect("grep 应该能正常执行");

    // grep 找不到匹配时退出码是 1，不是执行错误——那正是我们想要的结果（零命中）。
    let stdout = String::from_utf8_lossy(&output.stdout);
    let offending: Vec<&str> = stdout
        .lines()
        .filter(|line| {
            // 剔除以 `//` 开头的注释行：去掉 `path:lineno:` 前缀后再看内容开头。
            let content = line.splitn(3, ':').nth(2).unwrap_or(line).trim_start();
            !content.starts_with("//")
        })
        .collect();

    assert!(
        offending.is_empty(),
        "{rel} 下不许出现时钟/随机源（超时靠注入的 Timeout 事件；重放要得出同样的结果）：\n{}",
        offending.join("\n")
    );
}

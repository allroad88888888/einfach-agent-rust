//! 守一类**编译期一个 error 都没有、只在浏览器里真跑到那行才炸**的 bug：
//! `std::time::Instant::now()` / `SystemTime::now()` 在 `wasm32-unknown-unknown`
//! 上编译通过、运行时 panic（那个 target 没有时钟源）。
//!
//! 为什么值得写一条扫源码的测试，而不是相信「大家都记得用 `web-time`」：
//!
//! - `cargo check --target wasm32-unknown-unknown` **抓不到**——它零 error；
//! - native 上的全部测试也抓不到——native 上 `web-time` 就是 `std::time` 本尊，
//!   两种写法行为完全一致；
//! - 114b 收尾时就漏了一处（`io_bus.rs` 的泵循环，每轮对话都走），原因是它被
//!   划在另一个 agent 的文件范围里。**这类 bug 的成因是分工缝隙，不是粗心**，
//!   靠人盯守不住。
//!
//! 所以判据只能是文本扫描。规则：**wasm 会编进去的 crate 里，非测试代码不许
//! 直接用 `std::time` 的两个时钟类型**，一律走 `web_time`。
//! `Duration` 不在此列——它是纯值类型，不碰时钟。
//!
//! 要豁免某一处，就在那一行写明理由，而不是删掉这条测试。

use std::fs;
use std::path::{Path, PathBuf};

/// 会被编进浏览器产物的 crate。`agent-server` / `agent-mcp` 不在其中：
/// 前者是 native-only 的 HTTP 服务，后者要子进程 stdio，浏览器里根本不成立。
const WASM_REACHABLE: &[&str] = &[
    "agent-core",
    "agent-store",
    "agent-providers",
    "agent-transport",
    "agent-tools",
    "agent-runtime",
    // 123 补：`agent-wasm` 是**唯一一个只在浏览器里存在**的 crate，却一直不在这张
    // 表里——它是独立 workspace，`cargo test --workspace` 编都不编它，所以这条文本
    // 扫描是它唯一够得着的守卫。123 让它第一次跟时间打交道（宿主工具的截止线），
    // 办法是只从 `agent-runtime` 拿**相对时长**、自己一次时钟都不读；这一行把
    // 「不读」锁住，下一个人想在这里 `Instant::now()` 会当场变红。
    "agent-wasm",
];

fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(|p| p.parent())
        .expect("agent-runtime 应该在 <repo>/crates/agent-runtime 下")
        .to_path_buf()
}

fn rs_files(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            rs_files(&path, out);
        } else if path.extension().is_some_and(|e| e == "rs") {
            out.push(path);
        }
    }
}

/// 把 `#[cfg(test)]` 之后那个大括号配平的块整段挖掉。
///
/// 测试代码只在 native 上跑，用 `std::time` 没问题，不该被这条规则拦住。
/// 这个剥离是**故意保守**的：配平失败就把剩下的全当测试代码丢掉，
/// 宁可漏报也不误报——一条天天误报的测试等于没有测试。
fn strip_test_blocks(src: &str) -> String {
    let mut out = String::with_capacity(src.len());
    let mut rest = src;

    while let Some(marker) = rest.find("#[cfg(test)]") {
        out.push_str(&rest[..marker]);
        let after = &rest[marker..];
        let Some(open) = after.find('{') else {
            return out;
        };

        let mut depth = 0usize;
        let mut end = None;
        for (i, c) in after[open..].char_indices() {
            match c {
                '{' => depth += 1,
                '}' => {
                    depth -= 1;
                    if depth == 0 {
                        end = Some(open + i + 1);
                        break;
                    }
                }
                _ => {}
            }
        }
        match end {
            Some(e) => rest = &after[e..],
            None => return out,
        }
    }
    out.push_str(rest);
    out
}

#[test]
fn wasm_reachable_crates_never_take_the_clock_from_std_time() {
    let root = repo_root();
    let mut offenders: Vec<String> = Vec::new();

    for krate in WASM_REACHABLE {
        let src = root.join("crates").join(krate).join("src");
        let mut files = Vec::new();
        rs_files(&src, &mut files);

        for file in files {
            // `*_tests.rs` 整个文件都是测试代码——本仓的 20 处 `mod *_tests;` 声明
            // **无一例外**都在 `#[cfg(test)]`（或 `cfg(all(test, …))`）之下，
            // 已核实。这类文件里的 `#[cfg(test)]` 在**声明处**而不在文件里，
            // 下面按块剥离的办法看不见它，只能按文件名认。
            if file
                .file_name()
                .is_some_and(|n| n.to_string_lossy().ends_with("_tests.rs"))
            {
                continue;
            }
            let Ok(text) = fs::read_to_string(&file) else {
                continue;
            };
            let code = strip_test_blocks(&text);

            for (i, line) in code.lines().enumerate() {
                // 注释里提到 `std::time::Instant` 是在解释「为什么不用它」，不算违规。
                let trimmed = line.trim_start();
                if trimmed.starts_with("//") || trimmed.starts_with("*") {
                    continue;
                }
                let uses_std_clock = line.contains("std::time::Instant")
                    || line.contains("std::time::SystemTime")
                    || (line.contains("use std::time::{")
                        && (line.contains("Instant") || line.contains("SystemTime")));
                if uses_std_clock {
                    let rel = file.strip_prefix(&root).unwrap_or(&file);
                    offenders.push(format!("{}:{}: {}", rel.display(), i + 1, line.trim()));
                }
            }
        }
    }

    assert!(
        offenders.is_empty(),
        "下面这些地方直接从 `std::time` 取时钟。\n\
         wasm32-unknown-unknown 上 `Instant::now()` / `SystemTime::now()` **编译得过、\
         运行时 panic**，所以 cargo check 和 native 测试都拦不住，只有浏览器里真跑到\
         那一行才炸。改成 `web_time::{{Instant, SystemTime}}`——native 上它就是 std 本尊，\
         行为一字不变。\n\n{}",
        offenders.join("\n")
    );
}

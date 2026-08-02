//! 红线 12 元测试：`agent-core` / `agent-store` 里不许出现厂商名、`Capabilities`、
//! `caps.`（docs/INVARIANTS.md 红线 12，docs/issues/025-provider-seam.md 验收
//! 最后一条）。检查脚本是 `scripts/check-invariants.sh --all`，本测试只是把它
//! 接进 `cargo test`，让「core 没漏模型判断」成为 CI 会跑的一部分，而不是只在
//! 手动跑脚本时才发现。
//!
//! 这个脚本同时也覆盖红线 9（文件行数）等其它可 grep 判定的红线——`--all`
//! 是全仓检查，不只查 agent-core，但验收要的正是「全绿」而不是「只查 core
//! 那一半」。

use std::path::PathBuf;
use std::process::Command;

#[test]
fn check_invariants_script_passes_for_whole_repo() {
    // crates/agent-providers/tests/ -> 仓库根目录。
    let manifest_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let repo_root = manifest_dir
        .parent() // crates/
        .and_then(|p| p.parent()) // 仓库根
        .expect("agent-providers 应该在 <repo>/crates/agent-providers 下");

    let script = repo_root.join("scripts/check-invariants.sh");
    assert!(script.is_file(), "scripts/check-invariants.sh 必须存在: {}", script.display());

    let output = Command::new("bash")
        .arg(&script)
        .arg("--all")
        .current_dir(repo_root)
        .output()
        .expect("跑 scripts/check-invariants.sh --all 失败（无法启动进程）");

    assert!(
        output.status.success(),
        "红线检查未通过（red line 12 等）:\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr),
    );
}

//! 026 独立测试：元测试——`scripts/check-invariants.sh --all` 在全仓范围内
//! 必须 exit 0。这一条把红线 2 的白名单目录（`command/` 是唯一允许裸
//! `store.set` 的地方）、红线 1 的检查路径（026 把构图函数放进了 `graph/`，
//! 脚本要跟着扩到那里）、以及红线 12 等其余全部靠 grep 判定的红线，
//! 一次性钉在 CI 能看见的地方——不依赖任何人记得手动跑它。

#[test]
fn the_invariants_script_passes_across_the_whole_repo() {
    let repo_root = concat!(env!("CARGO_MANIFEST_DIR"), "/../..");
    let script = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../scripts/check-invariants.sh"
    );

    let output = std::process::Command::new("bash")
        .arg(script)
        .arg("--all")
        .current_dir(repo_root)
        .output()
        .expect("必须能拉起 bash 跑 check-invariants.sh");

    assert!(
        output.status.success(),
        "check-invariants.sh --all 应该 exit 0，实际 {:?}\nstdout:\n{}\nstderr:\n{}",
        output.status.code(),
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr),
    );
}

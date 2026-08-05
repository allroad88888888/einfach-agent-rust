//! 独立测试：红线 1 的实检——`crates/agent-core/src/cache/` 下不许出现
//! 时钟/随机（issue 024「注意」：判读函数必须是纯函数，不读时钟不读随机）。
//!
//! 用 `std::process::Command`（全限定写法，不 `use`）调系统 `grep`——
//! `agent-core` 本体禁止 IO（红线 7），但 `tests/` 已豁免这条检查，
//! 集成测试进程本身不受此限。

#[test]
fn cache_module_has_no_clock_or_random_outside_comments() {
    let manifest_dir = env!("CARGO_MANIFEST_DIR");
    let cache_dir = format!("{manifest_dir}/src/cache/");

    let violations = grep_pattern_excluding_comment_lines(
        &cache_dir,
        r"Instant::now|SystemTime::now|rand::|thread_rng",
    );

    assert!(
        violations.is_empty(),
        "src/cache/ 里不该出现时钟/随机（红线 1），命中：\n{violations:#?}"
    );
}

/// 元测试的元测试：确认上面那条检查的机制本身有效——不是因为路径写错、
/// 正则写错而永远零匹配。往一个临时文件里塞一行真违规，同样的抓取逻辑
/// 必须抓到它，抓不到就说明检查本身是摆设。
#[test]
fn the_grep_mechanism_actually_detects_a_planted_violation() {
    let dir = std::env::temp_dir().join(format!("guard_indep_meta_probe_{}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("create temp probe dir");
    let probe_file = dir.join("planted.rs");
    std::fs::write(
        &probe_file,
        "// 上面这行是注释，不该被算作命中\nlet _t = std::time::Instant::now();\n",
    )
    .expect("write probe file");

    let dir_str = format!("{}/", dir.display());
    let violations = grep_pattern_excluding_comment_lines(
        &dir_str,
        r"Instant::now|SystemTime::now|rand::|thread_rng",
    );

    std::fs::remove_dir_all(&dir).ok();

    assert_eq!(
        violations.len(),
        1,
        "抓取机制应该只抓到那一行真代码，注释行不算，实际: {violations:#?}"
    );
}

/// 对目标目录跑 `grep -RnE <pattern>`，剔除注释行（内容 trim 后以 `//` 开头
/// 的整行注释）后返回剩下的命中行。
fn grep_pattern_excluding_comment_lines(dir: &str, pattern: &str) -> Vec<String> {
    let output = std::process::Command::new("grep")
        .args(["-RnE", pattern, dir])
        .output()
        .expect("system grep must be available to run this meta test");

    // grep 找不到匹配时退出码是 1，不是 IO 错误，stdout 为空即可。
    let stdout = String::from_utf8_lossy(&output.stdout);

    stdout
        .lines()
        .filter(|line| {
            // 行格式是 path:lineno:content，取第三段判断是不是整行注释。
            let content = line.splitn(3, ':').nth(2).unwrap_or("");
            !content.trim_start().starts_with("//")
        })
        .map(|line| line.to_string())
        .collect()
}

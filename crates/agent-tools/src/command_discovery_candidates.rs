//! 从单个项目 manifest 提取可执行 argv 候选。
//!
//! 本模块只把已声明的任务名或生态约定变成 argv；它不读取文件、遍历目录或执行命令。

use serde_json::Value;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum ManifestKind {
    Cargo,
    PackageJson,
    PyProject,
    GoMod,
}

impl ManifestKind {
    pub(crate) fn from_file_name(name: &str) -> Option<Self> {
        match name {
            "Cargo.toml" => Some(Self::Cargo),
            "package.json" => Some(Self::PackageJson),
            "pyproject.toml" => Some(Self::PyProject),
            "go.mod" => Some(Self::GoMod),
            _ => None,
        }
    }

    pub(crate) fn label(self) -> &'static str {
        match self {
            Self::Cargo => "cargo",
            Self::PackageJson => "node",
            Self::PyProject => "python",
            Self::GoMod => "go",
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct Candidate {
    pub(crate) kind: &'static str,
    pub(crate) argv: Vec<String>,
    pub(crate) origin: &'static str,
    pub(crate) evidence: String,
    pub(crate) confidence: &'static str,
}

/// 解析一个已受读取上限保护的 manifest。`cwd` 不在此处保存，因为它属于发现层。
pub(crate) fn extract(kind: ManifestKind, content: &str) -> Vec<Candidate> {
    match kind {
        ManifestKind::Cargo => cargo_candidates(),
        ManifestKind::PackageJson => package_json_candidates(content),
        ManifestKind::PyProject => pyproject_candidates(content),
        ManifestKind::GoMod => go_candidates(),
    }
}

fn cargo_candidates() -> Vec<Candidate> {
    vec![
        inferred("test", ["cargo", "test"], "Cargo.toml"),
        inferred(
            "lint",
            ["cargo", "clippy", "--all-targets", "--", "-D", "warnings"],
            "Cargo.toml",
        ),
    ]
}

fn go_candidates() -> Vec<Candidate> {
    vec![
        inferred("test", ["go", "test", "./..."], "go.mod"),
        inferred("lint", ["go", "vet", "./..."], "go.mod"),
    ]
}

fn pyproject_candidates(content: &str) -> Vec<Candidate> {
    let lower = content.to_ascii_lowercase();
    let mut candidates = Vec::new();
    if has_table(&lower, "[tool.pytest") {
        candidates.push(inferred(
            "test",
            ["python", "-m", "pytest"],
            "pyproject.toml [tool.pytest*]",
        ));
    }
    if has_table(&lower, "[tool.ruff") {
        candidates.push(inferred(
            "lint",
            ["python", "-m", "ruff", "check", "."],
            "pyproject.toml [tool.ruff*]",
        ));
    }
    if has_table(&lower, "[tool.black") {
        candidates.push(inferred(
            "lint",
            ["python", "-m", "black", "--check", "."],
            "pyproject.toml [tool.black*]",
        ));
    }
    if has_table(&lower, "[tool.mypy") {
        candidates.push(inferred(
            "lint",
            ["python", "-m", "mypy", "."],
            "pyproject.toml [tool.mypy*]",
        ));
    }
    candidates
}

fn has_table(content: &str, prefix: &str) -> bool {
    content
        .lines()
        .any(|line| line.trim_start().starts_with(prefix))
}

fn package_json_candidates(content: &str) -> Vec<Candidate> {
    let Ok(value) = serde_json::from_str::<Value>(content) else {
        return Vec::new();
    };
    let runner = package_runner(&value);
    let Some(scripts) = value.get("scripts").and_then(Value::as_object) else {
        return Vec::new();
    };
    let mut names: Vec<&str> = scripts
        .iter()
        .filter_map(|(name, value)| {
            (value.is_string() && recognized_script_name(name)).then_some(name.as_str())
        })
        .collect();
    names.sort_by(|left, right| {
        script_order(left)
            .cmp(&script_order(right))
            .then(left.cmp(right))
    });

    names
        .into_iter()
        .map(|name| Candidate {
            kind: if name == "test" || name.starts_with("test:") {
                "test"
            } else {
                "lint"
            },
            argv: runner.argv_for(name),
            origin: "declared",
            evidence: format!("package.json scripts.{name}"),
            confidence: "high",
        })
        .collect()
}

fn inferred<const N: usize>(kind: &'static str, argv: [&str; N], evidence: &str) -> Candidate {
    Candidate {
        kind,
        argv: argv.into_iter().map(str::to_owned).collect(),
        origin: "inferred",
        evidence: evidence.to_owned(),
        confidence: "medium",
    }
}

fn recognized_script_name(name: &str) -> bool {
    (name == "test" || name.starts_with("test:") || name == "lint" || name.starts_with("lint:"))
        && name.len() <= 64
        && name
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b':' | b'-' | b'_'))
}

fn script_order(name: &str) -> u8 {
    match name {
        "test" => 0,
        "lint" => 1,
        _ if name.starts_with("test:") => 2,
        _ => 3,
    }
}

enum PackageRunner {
    Npm,
    Pnpm,
    Yarn,
    Bun,
}

impl PackageRunner {
    fn argv_for(&self, script: &str) -> Vec<String> {
        match self {
            Self::Npm => ["npm", "run", script],
            Self::Pnpm => ["pnpm", "run", script],
            Self::Yarn => ["yarn", "run", script],
            Self::Bun => ["bun", "run", script],
        }
        .into_iter()
        .map(str::to_owned)
        .collect()
    }
}

fn package_runner(value: &Value) -> PackageRunner {
    let declared = value
        .get("packageManager")
        .and_then(Value::as_str)
        .and_then(|manager| manager.split_once('@').map(|(name, _)| name));
    match declared {
        Some("pnpm") => PackageRunner::Pnpm,
        Some("yarn") => PackageRunner::Yarn,
        Some("bun") => PackageRunner::Bun,
        _ => PackageRunner::Npm,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn package_scripts_become_safe_runner_argv_not_shell_strings() {
        let candidates = package_json_candidates(
            r#"{"packageManager":"pnpm@9","scripts":{"lint":"eslint . && rm -rf /","test":"vitest run"}}"#,
        );
        assert_eq!(candidates[0].argv, ["pnpm", "run", "test"]);
        assert_eq!(candidates[1].argv, ["pnpm", "run", "lint"]);
    }

    #[test]
    fn pyproject_requires_a_recognized_tool_section_before_inferring() {
        assert!(pyproject_candidates("[project]\nname = 'only-metadata'").is_empty());
        assert_eq!(pyproject_candidates("[tool.pytest.ini_options]").len(), 1);
    }
}

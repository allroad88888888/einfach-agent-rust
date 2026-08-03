//! `srv:workspace/find_test_lint_commands` 的有界、只读命令发现。
//! 它只检查少量 manifest 并返回候选 argv，绝不启动 shell 或执行返回的命令。

use crate::ToolError;
use crate::command_discovery_candidates::{Candidate, ManifestKind, extract};
use crate::exec::tool_err;
use serde_json::{Value, json};
use std::io::Read;
use std::path::{Path, PathBuf};

const MAX_VISITED_ENTRIES: usize = 4_000;
const MAX_MANIFESTS: usize = 16;
const MAX_BYTES_PER_MANIFEST: usize = 8_000;
const MAX_TOTAL_MANIFEST_BYTES: usize = 48_000;
const MAX_ENCODED_PATH_BYTES: usize = 192;
const MAX_COMMANDS: usize = 32;
const MAX_RESPONSE_BYTES: usize = 24 * 1024;
pub(crate) fn discover(root: &Path, input: &Value) -> Result<String, ToolError> {
    require_empty_object(input)?;
    let scan = scan_manifests(root)?;
    let mut warnings = scan.warnings;
    let mut remaining = MAX_TOTAL_MANIFEST_BYTES;
    let mut manifests = Vec::new();
    let mut candidates = Vec::new();
    let mut truncated = scan.truncated;

    for manifest in scan.manifests {
        if remaining == 0 {
            warnings.push(
                "manifest excerpt budget was exhausted before every candidate was inspected".into(),
            );
            truncated = true;
            break;
        }
        let allowed = MAX_BYTES_PER_MANIFEST.min(remaining);
        match read_utf8_excerpt(&manifest.absolute, allowed) {
            Ok((content, manifest_truncated)) => {
                remaining -= content.len();
                let path = manifest.path.clone();
                let cwd = manifest.cwd.clone();
                let kind = manifest.kind;
                manifests.push(json!({
                    "path": path,
                    "cwd": cwd,
                    "kind": kind.label(),
                    "truncated": manifest_truncated,
                }));
                if manifest_truncated {
                    warnings.push(
                        "one manifest excerpt reached its byte limit; candidates may be incomplete"
                            .into(),
                    );
                    truncated = true;
                }
                candidates.extend(
                    extract(kind, &content)
                        .into_iter()
                        .map(|candidate| (candidate, manifest.cwd.clone())),
                );
            }
            Err(ManifestReadError::NotUtf8) => {
                warnings.push("one manifest was skipped because it was not valid UTF-8".into());
                truncated = true;
            }
            Err(ManifestReadError::Io) => {
                warnings
                    .push("one manifest could not be read; candidates may be incomplete".into());
                truncated = true;
            }
        }
    }

    let commands = command_json(candidates, &manifests);
    Ok(render_response(manifests, commands, warnings, truncated))
}
struct Manifest {
    absolute: PathBuf,
    path: String,
    cwd: String,
    kind: ManifestKind,
}

struct ManifestScan {
    manifests: Vec<Manifest>,
    warnings: Vec<String>,
    truncated: bool,
}

fn require_empty_object(input: &Value) -> Result<(), ToolError> {
    let object = input
        .as_object()
        .ok_or_else(|| tool_err("bad_input", "input 必须是不带参数的对象"))?;
    if let Some(field) = object.keys().next() {
        return Err(tool_err("bad_input", format!("不支持的参数：{field}")));
    }
    Ok(())
}

fn scan_manifests(root: &Path) -> Result<ManifestScan, ToolError> {
    let mut pending = vec![root.to_path_buf()];
    let mut found = Vec::new();
    let mut visited = 0usize;
    let mut truncated = false;
    let mut omitted_long_paths = 0usize;

    'directories: while let Some(directory) = pending.pop() {
        let mut entries: Vec<_> = std::fs::read_dir(&directory)
            .map_err(|error| {
                tool_err(
                    "workspace_read_failed",
                    format!("读取工作区目录失败：{error}"),
                )
            })?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|error| {
                tool_err(
                    "workspace_read_failed",
                    format!("读取工作区目录失败：{error}"),
                )
            })?;
        entries.sort_by_key(|entry| entry.file_name());
        for entry in entries {
            if visited == MAX_VISITED_ENTRIES {
                truncated = true;
                break 'directories;
            }
            visited += 1;
            let file_type = entry.file_type().map_err(|error| {
                tool_err(
                    "workspace_read_failed",
                    format!("读取文件类型失败：{error}"),
                )
            })?;
            if file_type.is_symlink() {
                continue;
            }
            if file_type.is_dir() {
                if !ignored_directory(&entry.file_name()) {
                    pending.push(entry.path());
                }
                continue;
            }
            if !file_type.is_file() {
                continue;
            }
            let Some(kind) = entry
                .file_name()
                .to_str()
                .and_then(ManifestKind::from_file_name)
            else {
                continue;
            };
            let Some((path, cwd)) = bounded_display_path(root, &entry.path()) else {
                omitted_long_paths += 1;
                continue;
            };
            found.push(Manifest {
                absolute: entry.path(),
                path,
                cwd,
                kind,
            });
        }
    }
    found.sort_by(|left, right| manifest_order(&left.path, &right.path));
    if found.len() > MAX_MANIFESTS {
        found.truncate(MAX_MANIFESTS);
        truncated = true;
    }
    let mut warnings = Vec::new();
    if truncated {
        warnings.push(
            "manifest scan reached a fixed entry or manifest cap; absence is not conclusive".into(),
        );
    }
    if omitted_long_paths > 0 {
        warnings.push("manifest paths exceeding the response-safe path limit were omitted".into());
    }
    Ok(ManifestScan {
        manifests: found,
        warnings,
        truncated,
    })
}

fn ignored_directory(name: &std::ffi::OsStr) -> bool {
    matches!(
        name.to_str(),
        Some(
            ".git"
                | "node_modules"
                | "vendor"
                | "dist"
                | "build"
                | "target"
                | ".next"
                | ".venv"
                | "venv"
        )
    )
}

fn bounded_display_path(root: &Path, path: &Path) -> Option<(String, String)> {
    let display = path
        .strip_prefix(root)
        .ok()?
        .to_string_lossy()
        .replace('\\', "/");
    (serde_json::to_string(&display).ok()?.len() <= MAX_ENCODED_PATH_BYTES).then(|| {
        let cwd = display
            .rsplit_once('/')
            .map_or(".", |(parent, _)| parent)
            .to_owned();
        (display, cwd)
    })
}

fn manifest_order(left: &str, right: &str) -> std::cmp::Ordering {
    left.matches('/')
        .count()
        .cmp(&right.matches('/').count())
        .then(left.cmp(right))
}

enum ManifestReadError {
    Io,
    NotUtf8,
}

fn read_utf8_excerpt(path: &Path, maximum: usize) -> Result<(String, bool), ManifestReadError> {
    let mut bytes = Vec::with_capacity(maximum.min(8_192));
    std::fs::File::open(path)
        .map_err(|_| ManifestReadError::Io)?
        .take((maximum + 1) as u64)
        .read_to_end(&mut bytes)
        .map_err(|_| ManifestReadError::Io)?;
    let truncated = bytes.len() > maximum;
    if truncated {
        bytes.truncate(maximum);
    }
    String::from_utf8(bytes)
        .map(|content| (content, truncated))
        .map_err(|_| ManifestReadError::NotUtf8)
}
fn command_json(candidates: Vec<(Candidate, String)>, manifests: &[Value]) -> Vec<Value> {
    let available_cwds: Vec<&str> = manifests
        .iter()
        .filter_map(|manifest| manifest.get("cwd")?.as_str())
        .collect();
    let mut commands = Vec::new();
    let mut seen = Vec::new();
    for (candidate, cwd) in candidates {
        if !available_cwds.contains(&cwd.as_str()) || commands.len() == MAX_COMMANDS {
            continue;
        }
        let key = format!("{}\0{}\0{}", candidate.kind, cwd, candidate.argv.join("\0"));
        if seen.contains(&key) {
            continue;
        }
        seen.push(key);
        commands.push(json!({
            "kind": candidate.kind,
            "argv": candidate.argv,
            "cwd": cwd,
            "origin": candidate.origin,
            "evidence": candidate.evidence,
            "confidence": candidate.confidence,
        }));
    }
    commands
}
fn render_response(
    manifests: Vec<Value>,
    commands: Vec<Value>,
    warnings: Vec<String>,
    truncated: bool,
) -> String {
    let result = json!({
        "manifests": manifests,
        "commands": commands,
        "warnings": warnings,
        "truncated": truncated,
    });
    let encoded = serde_json::to_string(&result).expect("命令发现结果必须可 JSON 编码");
    if encoded.len() <= MAX_RESPONSE_BYTES {
        return encoded;
    }
    serde_json::to_string(&json!({
        "manifests": [],
        "commands": [],
        "warnings": ["command discovery output reached its fixed byte budget"],
        "truncated": true,
    }))
    .expect("固定命令发现错误结果必须可 JSON 编码")
}

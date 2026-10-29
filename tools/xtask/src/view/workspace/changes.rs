//! 已暂存、未暂存和未跟踪改动及其逐行 diff。

use std::collections::BTreeMap;
use std::path::Path;

use axum::http::StatusCode;
use serde_json::{Value, json};

use super::files::read_revision;
use super::shared::{
    MAX_DIFF_ROWS, OUTSIDE_REPOSITORY_MESSAGE, Payload, extension, format_size, git_error,
    git_output_bytes, is_image_extension, is_pruned_path, normalize_relative_path, nul_paths,
    raw_url, resolve_existing, run_git_output,
};

/// 列出三类 Git 改动及其增删统计。
pub(in crate::view) fn changes(repository_root: &Path) -> Payload {
    let staged = collect_changed_files(repository_root, "staged");
    let unstaged = collect_changed_files(repository_root, "unstaged");
    let untracked_paths = git_output_bytes(
        repository_root,
        &["ls-files", "--others", "--exclude-standard", "-z"],
    )
    .map(|bytes| {
        nul_paths(&bytes)
            .into_iter()
            .filter(|path| !is_pruned_path(path))
            .map(|path| json!({"path": path, "status": "A", "rename_from": null, "add": null, "del": null}))
            .collect::<Vec<_>>()
    })
    .unwrap_or_default();
    let sections = vec![
        section("staged", "已暂存", true, staged),
        section("unstaged", "未暂存", true, unstaged),
        section("untracked", "未跟踪", false, untracked_paths),
    ];
    let all_files: Vec<Value> = sections
        .iter()
        .filter_map(|section| section.get("files").and_then(Value::as_array))
        .flatten()
        .cloned()
        .collect();
    Payload::ok(json!({"sections": sections, "totals": totals(&all_files)}))
}

fn collect_changed_files(repository_root: &Path, section_name: &str) -> Vec<Value> {
    let cached = section_name == "staged";
    let mut status_arguments = vec!["diff"];
    let mut numstat_arguments = vec!["diff"];
    if cached {
        status_arguments.push("--cached");
        numstat_arguments.push("--cached");
    }
    status_arguments.extend(["-M", "--name-status", "-z"]);
    numstat_arguments.extend(["-M", "--numstat", "-z"]);
    let status_bytes = git_output_bytes(repository_root, &status_arguments).unwrap_or_default();
    let numstat_bytes = git_output_bytes(repository_root, &numstat_arguments).unwrap_or_default();
    let statuses = parse_name_status(&status_bytes);
    let statistics = parse_numstat(&numstat_bytes);
    statuses
        .into_iter()
        .map(|(path, status, rename_from)| {
            let (added, deleted) = statistics.get(&path).copied().unwrap_or((None, None));
            json!({"path": path, "status": status, "rename_from": rename_from, "add": added, "del": deleted})
        })
        .collect()
}

fn section(section_name: &str, label: &str, stats_available: bool, files: Vec<Value>) -> Value {
    json!({
        "section": section_name,
        "label": label,
        "stats_available": stats_available,
        "totals": totals(&files),
        "files": files
    })
}

fn totals(files: &[Value]) -> Value {
    let added = files
        .iter()
        .filter_map(|file| file["add"].as_u64())
        .sum::<u64>();
    let deleted = files
        .iter()
        .filter_map(|file| file["del"].as_u64())
        .sum::<u64>();
    json!({"files": files.len(), "add": added, "del": deleted})
}

fn parse_name_status(output: &[u8]) -> Vec<(String, String, Option<String>)> {
    let tokens: Vec<&str> = output
        .split(|byte| *byte == 0)
        .filter(|token| !token.is_empty())
        .map(|token| std::str::from_utf8(token).unwrap_or_default())
        .collect();
    let mut entries = Vec::new();
    let mut index = 0;
    while index < tokens.len() {
        let status_token = tokens[index];
        index += 1;
        let status = status_token.chars().next().unwrap_or('M').to_string();
        if matches!(status.as_str(), "R" | "C") && index + 1 < tokens.len() {
            let old_path = tokens[index].to_string();
            let new_path = tokens[index + 1].to_string();
            index += 2;
            entries.push((new_path, status, Some(old_path)));
        } else if index < tokens.len() {
            entries.push((tokens[index].to_string(), status, None));
            index += 1;
        }
    }
    entries.sort_by(|left, right| left.0.cmp(&right.0));
    entries
}

fn parse_numstat(output: &[u8]) -> BTreeMap<String, (Option<u64>, Option<u64>)> {
    let tokens: Vec<&str> = output
        .split(|byte| *byte == 0)
        .filter(|token| !token.is_empty())
        .map(|token| std::str::from_utf8(token).unwrap_or_default())
        .collect();
    let mut entries = BTreeMap::new();
    let mut index = 0;
    while index < tokens.len() {
        let mut fields = tokens[index].splitn(3, '\t');
        let added = fields.next().unwrap_or_default().parse().ok();
        let deleted = fields.next().unwrap_or_default().parse().ok();
        let path_field = fields.next().unwrap_or_default();
        index += 1;
        if !path_field.is_empty() {
            entries.insert(path_field.to_string(), (added, deleted));
        } else if index + 1 < tokens.len() {
            entries.insert(tokens[index + 1].to_string(), (added, deleted));
            index += 2;
        }
    }
    entries
}

/// 读取一个分区下的逐行 diff。
pub(in crate::view) fn diff(
    repository_root: &Path,
    requested_path: &str,
    section_name: &str,
) -> Payload {
    let section_label = match section_name {
        "staged" => "已暂存",
        "unstaged" => "未暂存",
        "untracked" => "未跟踪",
        _ => return Payload::error(StatusCode::BAD_REQUEST, "拒绝：改动分区无效。"),
    };
    let Some(normalized_path) = normalize_relative_path(requested_path) else {
        return Payload::error(StatusCode::FORBIDDEN, OUTSIDE_REPOSITORY_MESSAGE);
    };
    if normalized_path.is_empty() {
        return Payload::error(StatusCode::BAD_REQUEST, "拒绝：diff 需要给出文件路径。");
    }
    let full_path = repository_root.join(&normalized_path);
    if section_name == "untracked" {
        match resolve_existing(repository_root, requested_path) {
            Ok((path, _)) if path.is_file() => {}
            Ok(_) => return Payload::error(StatusCode::NOT_FOUND, "未找到未跟踪文件。"),
            Err(payload) => return payload,
        }
    } else if full_path.exists() && resolve_existing(repository_root, requested_path).is_err() {
        return Payload::error(StatusCode::FORBIDDEN, OUTSIDE_REPOSITORY_MESSAGE);
    }

    let rename_from = if section_name == "untracked" {
        None
    } else {
        let mut status_arguments = vec!["diff"];
        if section_name == "staged" {
            status_arguments.push("--cached");
        }
        status_arguments.extend(["-M", "--name-status", "-z"]);
        parse_name_status(&git_output_bytes(repository_root, &status_arguments).unwrap_or_default())
            .into_iter()
            .find_map(|(new_path, _, old_path)| {
                (new_path == normalized_path).then_some(old_path).flatten()
            })
    };
    let mut arguments: Vec<String> = if section_name == "untracked" {
        vec![
            "diff".into(),
            "--no-index".into(),
            "--no-ext-diff".into(),
            "--no-color".into(),
            "--unified=3".into(),
            "--".into(),
            "/dev/null".into(),
            normalized_path.clone(),
        ]
    } else if section_name == "staged" {
        vec![
            "diff".into(),
            "--cached".into(),
            "--no-ext-diff".into(),
            "--no-color".into(),
            "--unified=3".into(),
            "--".into(),
            normalized_path.clone(),
        ]
    } else {
        vec![
            "diff".into(),
            "--no-ext-diff".into(),
            "--no-color".into(),
            "--unified=3".into(),
            "--".into(),
            normalized_path.clone(),
        ]
    };
    if section_name != "untracked" {
        let delimiter_index = arguments
            .iter()
            .position(|argument| argument == "--")
            .unwrap_or(arguments.len());
        arguments.insert(delimiter_index, "-M".into());
        if let Some(old_path) = &rename_from {
            arguments.push(old_path.clone());
        }
    }
    let output = run_git_output(repository_root, &arguments);
    let diff_text = match output {
        Ok(output)
            if output.status.success()
                || (section_name == "untracked" && output.status.code() == Some(1)) =>
        {
            String::from_utf8_lossy(&output.stdout).into_owned()
        }
        Ok(output) => {
            return Payload::error(StatusCode::INTERNAL_SERVER_ERROR, &git_error(&output));
        }
        Err(error) => return Payload::error(StatusCode::INTERNAL_SERVER_ERROR, &error),
    };
    let (rows, truncated, binary) = parse_diff_rows(&diff_text);
    let image_comparison = if binary && is_image_extension(extension(&normalized_path)) {
        image_comparison(
            repository_root,
            &normalized_path,
            rename_from.as_deref(),
            section_name,
        )
    } else {
        None
    };
    Payload::ok(json!({
        "path": normalized_path,
        "section": section_name,
        "section_label": section_label,
        "rename_from": rename_from,
        "rows": rows,
        "truncated": truncated,
        "binary": binary,
        "empty": !diff_text.lines().any(|line| line.starts_with("@@ ")),
        "image_comparison": image_comparison
    }))
}

fn parse_diff_rows(diff_text: &str) -> (Vec<Value>, bool, bool) {
    let mut rows = Vec::new();
    let mut old_line = 0_u64;
    let mut new_line = 0_u64;
    let mut binary = false;
    let mut truncated = false;
    for line in diff_text.lines() {
        if rows.len() >= MAX_DIFF_ROWS {
            truncated = true;
            break;
        }
        if line.starts_with("Binary files ") || line.starts_with("GIT binary patch") {
            binary = true;
            continue;
        }
        if line.starts_with("@@ ") {
            if let Some((old, new)) = parse_hunk_header(line) {
                old_line = old;
                new_line = new;
            }
            rows.push(json!({"kind": "hunk", "old_no": null, "new_no": null, "text": line}));
            continue;
        }
        if line.starts_with("diff --git ")
            || line.starts_with("index ")
            || line.starts_with("--- ")
            || line.starts_with("+++ ")
            || line.starts_with("new file mode ")
            || line.starts_with("deleted file mode ")
            || line.starts_with("old mode ")
            || line.starts_with("new mode ")
            || line.starts_with("similarity index ")
            || line.starts_with("rename from ")
            || line.starts_with("rename to ")
            || line.starts_with("\\ No newline")
        {
            continue;
        }
        if let Some(text) = line.strip_prefix('+') {
            rows.push(json!({"kind": "add", "old_no": null, "new_no": new_line, "text": text}));
            new_line += 1;
        } else if let Some(text) = line.strip_prefix('-') {
            rows.push(json!({"kind": "del", "old_no": old_line, "new_no": null, "text": text}));
            old_line += 1;
        } else if let Some(text) = line.strip_prefix(' ') {
            rows.push(json!({"kind": "ctx", "old_no": old_line, "new_no": new_line, "text": text}));
            old_line += 1;
            new_line += 1;
        }
    }
    (rows, truncated, binary)
}

fn parse_hunk_header(line: &str) -> Option<(u64, u64)> {
    let rest = line.strip_prefix("@@ -")?;
    let (old, rest) = rest.split_once(' ')?;
    let rest = rest.strip_prefix('+')?;
    let (new, _) = rest.split_once(' ')?;
    Some((
        old.split(',').next()?.parse().ok()?,
        new.split(',').next()?.parse().ok()?,
    ))
}

fn image_comparison(
    repository_root: &Path,
    path: &str,
    old_path: Option<&str>,
    section: &str,
) -> Option<Value> {
    let (old_revision, new_revision) = match section {
        "staged" => (Some("head"), Some("index")),
        "unstaged" => (Some("index"), None),
        "untracked" => (None, None),
        _ => return None,
    };
    let mut panes = Vec::new();
    let mut missing = Vec::new();
    if let Some(revision) = old_revision {
        if let Some(bytes) = read_revision(repository_root, old_path.unwrap_or(path), revision) {
            panes.push(image_pane(
                old_path.unwrap_or(path),
                revision,
                bytes.len() as u64,
            ));
        } else {
            missing.push("舊版本");
        }
    } else {
        missing.push("舊版本");
    }
    if let Some(revision) = new_revision {
        if let Some(bytes) = read_revision(repository_root, path, revision) {
            panes.push(image_pane(path, revision, bytes.len() as u64));
        } else {
            missing.push("新版本");
        }
    } else if let Ok((full_path, _)) = resolve_existing(repository_root, path) {
        if let Ok(metadata) = full_path.metadata() {
            panes.push(image_pane(path, "worktree", metadata.len()));
        } else {
            missing.push("新版本");
        }
    } else {
        missing.push("新版本");
    }
    if panes.is_empty() {
        return None;
    }
    let note = if missing.is_empty() {
        "".to_string()
    } else {
        format!("{}不存在；只显示可读取的一侧。", missing.join("与"))
    };
    Some(json!({"panes": panes, "note": note}))
}

fn image_pane(path: &str, revision: &str, size: u64) -> Value {
    let label = match revision {
        "head" => "HEAD 版本",
        "index" => "索引版本",
        _ => "工作区版本",
    };
    json!({"label": label, "url": raw_url(path, revision), "size_label": format_size(size)})
}

//! 文件树、文件内容、Markdown、原始文件和受控动作。

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use axum::http::StatusCode;
use pulldown_cmark::{Options, Parser, html};
use serde_json::{Value, json};

use super::shared::{
    BINARY_SNIFF_BYTES, MAX_FILE_BYTES, OUTSIDE_REPOSITORY_MESSAGE, Payload, content_type,
    extension, format_size, git_error, git_output, git_output_bytes, highlight_source,
    is_html_extension, is_image_extension, is_markdown_extension, is_pruned_path,
    normalize_reference_path, normalize_relative_path, nul_paths, percent_encode, raw_url,
    resolve_existing, resolve_git_path, run_git_output,
};

/// 返回仓库名称、分支和正文渲染上限。
pub(in crate::view) fn info(repository_root: &Path) -> Payload {
    let branch = git_output(repository_root, &["branch", "--show-current"])
        .unwrap_or_default()
        .trim()
        .to_string();
    Payload::ok(json!({
        "repo_name": repository_root.file_name().unwrap_or_default().to_string_lossy(),
        "branch": branch,
        "limits": {"max_file_bytes": MAX_FILE_BYTES, "max_file_label": "256 KiB"}
    }))
}

/// 列出受跟踪、未跟踪路径，可按需加入忽略文件。
pub(in crate::view) fn tree(repository_root: &Path, include_ignored: bool) -> Payload {
    let tracked_paths =
        git_output_bytes(repository_root, &["ls-files", "-z", "--cached"]).unwrap_or_default();
    let untracked_paths = git_output_bytes(
        repository_root,
        &["ls-files", "--others", "--exclude-standard", "-z"],
    )
    .unwrap_or_default();
    let ignored_paths = if include_ignored {
        git_output_bytes(
            repository_root,
            &[
                "ls-files",
                "--others",
                "--ignored",
                "--exclude-standard",
                "-z",
            ],
        )
        .unwrap_or_default()
    } else {
        Vec::new()
    };

    let tracked: BTreeSet<String> = nul_paths(&tracked_paths).into_iter().collect();
    let ignored: BTreeSet<String> = nul_paths(&ignored_paths)
        .into_iter()
        .filter(|path| !is_pruned_path(path))
        .collect();
    let mut paths = tracked.clone();
    paths.extend(
        nul_paths(&untracked_paths)
            .into_iter()
            .filter(|path| !is_pruned_path(path)),
    );
    if include_ignored {
        paths.extend(ignored.iter().cloned());
    }
    Payload::ok(json!({
        "paths": paths,
        "ignored_paths": ignored,
        "count": paths.len()
    }))
}

/// 按文件类型返回源码、图片或不可渲染说明。
pub(in crate::view) fn file_payload(repository_root: &Path, requested_path: &str) -> Payload {
    let (resolved_path, normalized_path) = match resolve_existing(repository_root, requested_path) {
        Ok(value) => value,
        Err(payload) => return payload,
    };
    if resolved_path.is_dir() {
        let displayed_path = if normalized_path.is_empty() {
            "."
        } else {
            &normalized_path
        };
        return Payload::error(
            StatusCode::BAD_REQUEST,
            &format!("拒绝：{displayed_path} 是一个目录，请选择一个文件。"),
        );
    }
    if !resolved_path.is_file() {
        return Payload::error(
            StatusCode::NOT_FOUND,
            &format!("未找到文件：{normalized_path}"),
        );
    }
    let file_size = match resolved_path.metadata() {
        Ok(metadata) => metadata.len(),
        Err(error) => return Payload::error(StatusCode::INTERNAL_SERVER_ERROR, &error.to_string()),
    };
    let extension = extension(&normalized_path);
    let size_label = format_size(file_size);
    if is_image_extension(extension) {
        return Payload::ok(json!({
            "path": normalized_path,
            "kind": "image",
            "url": raw_url(&normalized_path, ""),
            "size": file_size,
            "size_label": size_label
        }));
    }
    if file_size > MAX_FILE_BYTES {
        return Payload::ok(json!({
            "path": normalized_path,
            "kind": "oversize",
            "size": file_size,
            "size_label": size_label,
            "limit_label": "256 KiB",
            "note": format!("文件为 {size_label}，超过 256 KiB 的渲染上限，未读取正文。")
        }));
    }
    let bytes = match std::fs::read(&resolved_path) {
        Ok(bytes) => bytes,
        Err(error) => return Payload::error(StatusCode::INTERNAL_SERVER_ERROR, &error.to_string()),
    };
    if bytes.iter().take(BINARY_SNIFF_BYTES).any(|byte| *byte == 0) {
        return Payload::ok(json!({
            "path": normalized_path,
            "kind": "binary",
            "size": file_size,
            "size_label": size_label,
            "note": "二进制文件，不渲染正文。"
        }));
    }

    let source = String::from_utf8_lossy(&bytes);
    let (lines, language, highlighted) = highlight_source(&source, &normalized_path);
    let preview = if is_markdown_extension(extension) {
        Some(json!({"mode": "markdown"}))
    } else if is_html_extension(extension) {
        Some(json!({"mode": "external", "url": raw_url(&normalized_path, "")}))
    } else {
        None
    };
    Payload::ok(json!({
        "path": normalized_path,
        "kind": "text",
        "language": language,
        "size": file_size,
        "size_label": size_label,
        "line_count": lines.len(),
        "highlighted": highlighted,
        "lines": lines,
        "preview": preview
    }))
}

/// 按需渲染 Markdown，支持相对图片和仓库内链接。
pub(in crate::view) fn markdown(repository_root: &Path, requested_path: &str) -> Payload {
    let (resolved_path, normalized_path) = match resolve_existing(repository_root, requested_path) {
        Ok(value) => value,
        Err(payload) => return payload,
    };
    if !is_markdown_extension(extension(&normalized_path)) {
        return Payload::error(StatusCode::BAD_REQUEST, "拒绝：该路径不是 Markdown 文件。");
    }
    let metadata = match resolved_path.metadata() {
        Ok(metadata) => metadata,
        Err(error) => return Payload::error(StatusCode::INTERNAL_SERVER_ERROR, &error.to_string()),
    };
    if metadata.len() > MAX_FILE_BYTES {
        return Payload::error(
            StatusCode::BAD_REQUEST,
            "拒绝：Markdown 文件超过 256 KiB 渲染上限。",
        );
    }
    let source_bytes = match std::fs::read(&resolved_path) {
        Ok(source_bytes) => source_bytes,
        Err(error) => return Payload::error(StatusCode::INTERNAL_SERVER_ERROR, &error.to_string()),
    };
    let source = String::from_utf8_lossy(&source_bytes);
    let mut options = Options::empty();
    options.insert(Options::ENABLE_TABLES);
    options.insert(Options::ENABLE_TASKLISTS);
    options.insert(Options::ENABLE_STRIKETHROUGH);
    let parser = Parser::new_ext(&source, options);
    let mut rendered_html = String::with_capacity(source.len().saturating_mul(2));
    html::push_html(&mut rendered_html, parser);
    if let Ok(mermaid_pattern) =
        regex::Regex::new(r#"<pre><code class="language-mermaid">(?P<source>.*?)</code></pre>"#)
    {
        rendered_html = mermaid_pattern
            .replace_all(&rendered_html, |captures: &regex::Captures<'_>| {
                format!("<pre class=\"mermaid\">{}</pre>", &captures["source"])
            })
            .into_owned();
    }
    let rebased_html = rebase_markdown_references(&rendered_html, &normalized_path);
    Payload::ok(json!({"format": "markdown", "html": rebased_html}))
}

fn rebase_markdown_references(rendered_html: &str, markdown_path: &str) -> String {
    let Ok(tag_pattern) =
        regex::Regex::new(r#"<[a-zA-Z][a-zA-Z0-9-]*(?:[^>"']|"[^"]*"|'[^']*')*>"#)
    else {
        return rendered_html.to_string();
    };
    let Ok(attribute_pattern) = regex::Regex::new(r#"(\b(?:src|href)\s*=\s*)("[^"]*"|'[^']*')"#)
    else {
        return rendered_html.to_string();
    };
    let base_directory = Path::new(markdown_path)
        .parent()
        .unwrap_or_else(|| Path::new(""));
    tag_pattern
        .replace_all(rendered_html, |tag: &regex::Captures<'_>| {
            attribute_pattern
                .replace_all(&tag[0], |attribute: &regex::Captures<'_>| {
                    let raw_attribute = &attribute[2];
                    let quote = raw_attribute.chars().next().unwrap_or('"');
                    let raw_url = raw_attribute.trim_matches(['"', '\'']);
                    let Some(rebased) = rebase_reference(base_directory, raw_url) else {
                        return attribute[0].to_string();
                    };
                    format!("{}{}{}{}", &attribute[1], quote, rebased, quote)
                })
                .into_owned()
        })
        .into_owned()
}

fn rebase_reference(base_directory: &Path, reference_url: &str) -> Option<String> {
    if reference_url.is_empty()
        || reference_url.starts_with('#')
        || reference_url.starts_with("//")
        || reference_url.contains('?')
        || reference_url.contains('#')
        || reference_url.contains(':')
    {
        return None;
    }
    let reference_path = if let Some(root_relative) = reference_url.strip_prefix('/') {
        PathBuf::from(root_relative)
    } else {
        base_directory.join(reference_url)
    };
    let normalized_path = normalize_reference_path(&reference_path)?;
    if normalized_path.is_empty() {
        return None;
    }
    if is_markdown_extension(extension(&normalized_path)) {
        Some(format!(
            "/?view=files&path={}",
            percent_encode(&normalized_path)
        ))
    } else {
        Some(raw_url(&normalized_path, ""))
    }
}

/// 返回供 HTML 文档与历史图片对比使用的原始文件字节。
pub(in crate::view) fn raw_file(
    repository_root: &Path,
    encoded_path: &str,
    revision: &str,
) -> Result<(String, Vec<u8>), Payload> {
    let Some(normalized_path) = normalize_relative_path(encoded_path) else {
        return Err(Payload::error(
            StatusCode::FORBIDDEN,
            OUTSIDE_REPOSITORY_MESSAGE,
        ));
    };
    let bytes = if revision.is_empty() {
        let (resolved_path, _) = resolve_existing(repository_root, encoded_path)?;
        if !resolved_path.is_file() {
            return Err(Payload::error(
                StatusCode::BAD_REQUEST,
                "拒绝：该路径不是文件。",
            ));
        }
        std::fs::read(resolved_path).map_err(|error| {
            Payload::error(StatusCode::INTERNAL_SERVER_ERROR, &error.to_string())
        })?
    } else {
        let Some(bytes) = read_revision(repository_root, &normalized_path, revision) else {
            return Err(Payload::error(
                StatusCode::NOT_FOUND,
                "未找到该版本中的文件。",
            ));
        };
        bytes
    };
    Ok((content_type(&normalized_path).to_string(), bytes))
}

pub(super) fn read_revision(repository_root: &Path, path: &str, revision: &str) -> Option<Vec<u8>> {
    let prefix = match revision {
        "head" => "HEAD",
        "index" => ":0",
        _ => return None,
    };
    let expression = format!("{prefix}:{path}");
    let output = Command::new("git")
        .current_dir(repository_root)
        .args(["--no-pager", "cat-file", "blob", &expression])
        .output()
        .ok()?;
    output.status.success().then_some(output.stdout)
}

/// 暂存单个路径或全部工作区改动。
pub(in crate::view) fn stage(repository_root: &Path, request: &Value) -> Payload {
    let Some(request) = request.as_object() else {
        return Payload::error(StatusCode::BAD_REQUEST, "拒绝：请求体必须是 JSON 对象。");
    };
    match request.get("scope").and_then(Value::as_str) {
        Some("all") => match run_git_output(repository_root, &["add", "-A"].map(str::to_string)) {
            Ok(output) if output.status.success() => Payload::ok(json!({"scope": "all"})),
            Ok(output) => Payload::error(StatusCode::INTERNAL_SERVER_ERROR, &git_error(&output)),
            Err(error) => Payload::error(StatusCode::INTERNAL_SERVER_ERROR, &error),
        },
        Some("path") => {
            let Some(path) = request.get("path").and_then(Value::as_str) else {
                return Payload::error(
                    StatusCode::BAD_REQUEST,
                    "拒绝：scope 为 path 时必须给出字符串 path。",
                );
            };
            let normalized_path = match resolve_git_path(repository_root, path) {
                Ok(path) if !path.is_empty() => path,
                Ok(_) => {
                    return Payload::error(
                        StatusCode::BAD_REQUEST,
                        "拒绝：暂存需要给出一个仓库内的路径。",
                    );
                }
                Err(payload) => return payload,
            };
            match run_git_output(
                repository_root,
                &["add".to_string(), "--".to_string(), normalized_path.clone()],
            ) {
                Ok(output) if output.status.success() => {
                    Payload::ok(json!({"scope": "path", "path": normalized_path}))
                }
                Ok(output) => {
                    Payload::error(StatusCode::INTERNAL_SERVER_ERROR, &git_error(&output))
                }
                Err(error) => Payload::error(StatusCode::INTERNAL_SERVER_ERROR, &error),
            }
        }
        _ => Payload::error(StatusCode::BAD_REQUEST, "拒绝：scope 只能是 path 或 all。"),
    }
}

/// 将仓库内路径交给系统默认应用打开。
pub(in crate::view) fn open(repository_root: &Path, request: &Value) -> Payload {
    let Some(path) = request.get("path").and_then(Value::as_str) else {
        return Payload::error(StatusCode::BAD_REQUEST, "拒绝：打开必须给出字符串 path。");
    };
    let (resolved_path, normalized_path) = match resolve_existing(repository_root, path) {
        Ok(value) => value,
        Err(payload) => return payload,
    };
    let mut command = if cfg!(target_os = "macos") {
        let mut command = Command::new("open");
        command.arg(&resolved_path);
        command
    } else if cfg!(target_os = "windows") {
        let mut command = Command::new("cmd");
        command.args(["/C", "start", "", &resolved_path.to_string_lossy()]);
        command
    } else {
        let mut command = Command::new("xdg-open");
        command.arg(&resolved_path);
        command
    };
    match command
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
    {
        Ok(_) => Payload::ok(json!({"opened": normalized_path})),
        Err(error) => Payload::error(
            StatusCode::INTERNAL_SERVER_ERROR,
            &format!("打开失败：{error}"),
        ),
    }
}

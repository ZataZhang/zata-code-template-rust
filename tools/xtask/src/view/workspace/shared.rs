//! 查看器共用的路径校验、Git 调用和文件格式工具。

use std::path::{Component, Path, PathBuf};
use std::process::{Command, Output};
use std::sync::OnceLock;

use axum::http::StatusCode;
use serde_json::{Value, json};
use syntect::easy::HighlightLines;
use syntect::highlighting::ThemeSet;
use syntect::html::{IncludeBackground, styled_line_to_highlighted_html};
use syntect::parsing::SyntaxSet;
use syntect::util::LinesWithEndings;

pub(super) const MAX_FILE_BYTES: u64 = 256 * 1024;
pub(super) const MAX_DIFF_ROWS: usize = 4000;
pub(super) const BINARY_SNIFF_BYTES: usize = 8192;
pub(super) const OUTSIDE_REPOSITORY_MESSAGE: &str =
    "拒绝：该路径越出仓库范围，只读查看器不读取仓库外的文件。";
const PRUNED_DIRECTORY_NAMES: &[&str] = &[
    ".git",
    ".mypy_cache",
    ".next",
    ".pytest_cache",
    ".ruff_cache",
    ".tox",
    ".venv",
    "__pycache__",
    "coverage",
    "node_modules",
    "site-packages",
    "venv",
];

/// HTTP 应答状态与 JSON 正文。
pub(in crate::view) struct Payload {
    pub(in crate::view) status: StatusCode,
    pub(in crate::view) body: Value,
}

impl Payload {
    pub(super) fn ok(body: Value) -> Self {
        Self {
            status: StatusCode::OK,
            body,
        }
    }

    pub(in crate::view) fn error(status: StatusCode, message: &str) -> Self {
        Self {
            status,
            body: json!({"error": message}),
        }
    }
}

pub(super) fn resolve_existing(
    repository_root: &Path,
    requested_path: &str,
) -> Result<(PathBuf, String), Payload> {
    let Some(normalized_path) = normalize_relative_path(requested_path) else {
        return Err(Payload::error(
            StatusCode::FORBIDDEN,
            OUTSIDE_REPOSITORY_MESSAGE,
        ));
    };
    let candidate_path = repository_root.join(&normalized_path);
    let resolved_path = candidate_path.canonicalize().map_err(|_| {
        Payload::error(
            StatusCode::NOT_FOUND,
            &format!("未找到文件：{normalized_path}"),
        )
    })?;
    if !resolved_path.starts_with(repository_root) {
        return Err(Payload::error(
            StatusCode::FORBIDDEN,
            OUTSIDE_REPOSITORY_MESSAGE,
        ));
    }
    Ok((resolved_path, normalized_path))
}

pub(super) fn normalize_relative_path(requested_path: &str) -> Option<String> {
    if requested_path.contains('\0') {
        return None;
    }
    let requested_path = Path::new(requested_path);
    if requested_path.is_absolute() {
        return None;
    }
    let mut parts = Vec::new();
    for component in requested_path.components() {
        match component {
            Component::Normal(part) => parts.push(part.to_string_lossy().into_owned()),
            Component::CurDir => {}
            Component::ParentDir | Component::RootDir | Component::Prefix(_) => return None,
        }
    }
    Some(parts.join("/"))
}

pub(super) fn resolve_git_path(
    repository_root: &Path,
    requested_path: &str,
) -> Result<String, Payload> {
    let Some(normalized_path) = normalize_relative_path(requested_path) else {
        return Err(Payload::error(
            StatusCode::FORBIDDEN,
            OUTSIDE_REPOSITORY_MESSAGE,
        ));
    };
    let candidate_path = repository_root.join(&normalized_path);
    if let Ok(resolved_path) = candidate_path.canonicalize()
        && !resolved_path.starts_with(repository_root)
    {
        return Err(Payload::error(
            StatusCode::FORBIDDEN,
            OUTSIDE_REPOSITORY_MESSAGE,
        ));
    }
    Ok(normalized_path)
}

pub(super) fn normalize_reference_path(requested_path: &Path) -> Option<String> {
    let mut parts: Vec<String> = Vec::new();
    for component in requested_path.components() {
        match component {
            Component::Normal(part) => parts.push(part.to_string_lossy().into_owned()),
            Component::CurDir => {}
            Component::ParentDir if !parts.is_empty() => {
                parts.pop();
            }
            Component::ParentDir | Component::RootDir | Component::Prefix(_) => return None,
        }
    }
    Some(parts.join("/"))
}

pub(super) fn nul_paths(output: &[u8]) -> Vec<String> {
    output
        .split(|byte| *byte == 0)
        .filter(|path| !path.is_empty())
        .map(|path| String::from_utf8_lossy(path).into_owned())
        .collect()
}

pub(super) fn is_pruned_path(path: &str) -> bool {
    path.split('/')
        .any(|part| PRUNED_DIRECTORY_NAMES.contains(&part))
}

pub(super) fn extension(path: &str) -> &str {
    Path::new(path)
        .extension()
        .and_then(|extension| extension.to_str())
        .unwrap_or_default()
}

pub(super) fn is_markdown_extension(extension: &str) -> bool {
    matches!(extension.to_ascii_lowercase().as_str(), "md" | "markdown")
}

pub(super) fn is_html_extension(extension: &str) -> bool {
    matches!(extension.to_ascii_lowercase().as_str(), "html" | "htm")
}

pub(super) fn is_image_extension(extension: &str) -> bool {
    matches!(
        extension.to_ascii_lowercase().as_str(),
        "gif" | "ico" | "jpeg" | "jpg" | "png" | "webp"
    )
}

pub(super) fn content_type(path: &str) -> &'static str {
    match extension(path).to_ascii_lowercase().as_str() {
        "css" => "text/css; charset=utf-8",
        "gif" => "image/gif",
        "htm" | "html" => "text/html; charset=utf-8",
        "ico" => "image/vnd.microsoft.icon",
        "jpeg" | "jpg" => "image/jpeg",
        "js" | "mjs" => "text/javascript; charset=utf-8",
        "json" => "application/json; charset=utf-8",
        "png" => "image/png",
        "svg" => "image/svg+xml",
        "webp" => "image/webp",
        "woff" => "font/woff",
        "woff2" => "font/woff2",
        _ => "application/octet-stream",
    }
}

pub(super) fn format_size(byte_count: u64) -> String {
    if byte_count < 1000 {
        format!("{byte_count} B")
    } else if byte_count < 1_000_000 {
        format!("{:.1} KB", byte_count as f64 / 1000.0)
    } else {
        format!("{:.1} MB", byte_count as f64 / 1_000_000.0)
    }
}

pub(super) fn raw_url(path: &str, revision: &str) -> String {
    let mut url = format!("/raw/{}", percent_encode(path));
    if !revision.is_empty() && revision != "worktree" {
        url.push_str("?rev=");
        url.push_str(revision);
    }
    url
}

pub(super) fn percent_encode(value: &str) -> String {
    let mut encoded = String::with_capacity(value.len());
    for byte in value.bytes() {
        if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.' | b'~' | b'/') {
            encoded.push(char::from(byte));
        } else {
            encoded.push_str(&format!("%{byte:02X}"));
        }
    }
    encoded
}

pub(super) fn highlight_source(source: &str, path: &str) -> (Vec<String>, String, bool) {
    static SYNTAXES: OnceLock<SyntaxSet> = OnceLock::new();
    static THEMES: OnceLock<ThemeSet> = OnceLock::new();
    let syntax_set = SYNTAXES.get_or_init(SyntaxSet::load_defaults_newlines);
    let theme_set = THEMES.get_or_init(ThemeSet::load_defaults);
    let file_name = Path::new(path)
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or_default();
    let syntax = if file_name == "justfile" {
        syntax_set.find_syntax_by_extension("mk")
    } else if file_name == "Cargo.lock" {
        syntax_set.find_syntax_by_extension("toml")
    } else {
        syntax_set.find_syntax_by_extension(extension(path))
    };
    let Some(syntax) = syntax else {
        return (
            source.lines().map(escape_html).collect(),
            "纯文本".to_string(),
            false,
        );
    };
    let theme = theme_set
        .themes
        .get("base16-ocean.dark")
        .or_else(|| theme_set.themes.values().next());
    let Some(theme) = theme else {
        return (
            source.lines().map(escape_html).collect(),
            "纯文本".to_string(),
            false,
        );
    };
    let mut highlighter = HighlightLines::new(syntax, theme);
    let mut rendered_lines = Vec::new();
    for line in LinesWithEndings::from(source) {
        match highlighter.highlight_line(line, syntax_set) {
            Ok(ranges) => match styled_line_to_highlighted_html(&ranges, IncludeBackground::No) {
                Ok(rendered) => rendered_lines.push(rendered.trim_end_matches('\n').to_string()),
                Err(_) => rendered_lines.push(escape_html(line.trim_end_matches('\n'))),
            },
            Err(_) => rendered_lines.push(escape_html(line.trim_end_matches('\n'))),
        }
    }
    if source.is_empty() {
        rendered_lines.push(String::new());
    }
    (
        rendered_lines,
        syntax.name.clone(),
        syntax.name != "Plain Text",
    )
}

fn escape_html(source: &str) -> String {
    source
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&#x27;")
}

pub(super) fn git_output(repository_root: &Path, arguments: &[&str]) -> Option<String> {
    let output = Command::new("git")
        .current_dir(repository_root)
        .args(arguments)
        .output()
        .ok()?;
    output
        .status
        .success()
        .then(|| String::from_utf8_lossy(&output.stdout).into_owned())
}

pub(super) fn git_output_bytes(repository_root: &Path, arguments: &[&str]) -> Option<Vec<u8>> {
    let output = Command::new("git")
        .current_dir(repository_root)
        .args(arguments)
        .output()
        .ok()?;
    output.status.success().then_some(output.stdout)
}

pub(super) fn run_git_output(
    repository_root: &Path,
    arguments: &[String],
) -> Result<Output, String> {
    Command::new("git")
        .current_dir(repository_root)
        .arg("--literal-pathspecs")
        .args(arguments)
        .output()
        .map_err(|error| format!("无法运行 git：{error}"))
}

pub(super) fn git_error(output: &Output) -> String {
    let error = String::from_utf8_lossy(&output.stderr).trim().to_string();
    if error.is_empty() {
        format!("git 退出码 {}", output.status.code().unwrap_or(1))
    } else {
        error
    }
}

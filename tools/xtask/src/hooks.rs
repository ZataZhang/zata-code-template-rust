//! 提交钩子用的检查：文件行数上限、守卫测试修改保护、PRD 锁冲突警告。

use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::LazyLock;

use regex::Regex;
use time::OffsetDateTime;

use crate::git;
use crate::prd::lock::{self, LockState};

/// 目录递归时跳过的目录。
const SKIPPED_DIRS: [&str; 5] = [".git", "target", "node_modules", ".next", "dist"];

/// 文件中非空行（含只有空白的行以外的所有行）的数量。注释与文档字符串都算，只排除纯空行。
pub fn count_non_empty_lines(text: &str) -> usize {
    text.lines().filter(|line| !line.trim().is_empty()).count()
}

/// 把 `*.rs` 这类简单 glob 转成正则：`*` 匹配任意非 `/` 字符，`?` 匹配单个非 `/` 字符。
fn glob_to_regex(pattern: &str) -> Regex {
    let mut source = String::from("^");
    for character in pattern.chars() {
        match character {
            '*' => source.push_str("[^/]*"),
            '?' => source.push_str("[^/]"),
            other => source.push_str(&regex::escape(&other.to_string())),
        }
    }
    source.push('$');
    Regex::new(&source).expect("转义后的 glob 一定是合法正则")
}

/// 路径是否匹配 glob：不含 `/` 的模式只匹配文件名，含 `/` 的模式匹配路径的末尾部分。
fn glob_matches(path: &Path, pattern: &str) -> bool {
    let regex = glob_to_regex(pattern);
    if pattern.contains('/') {
        // 含 `/` 的模式匹配路径的最后 N 段（N = 模式的段数），例如 `src/*.rs` 匹配任何以 `src/<文件>.rs` 结尾的路径。
        let text = path.to_string_lossy().replace('\\', "/");
        let segments: Vec<&str> = text.split('/').collect();
        let wanted = pattern.matches('/').count() + 1;
        let tail = segments[segments.len().saturating_sub(wanted)..].join("/");
        regex.is_match(&tail)
    } else {
        path.file_name()
            .is_some_and(|name| regex.is_match(&name.to_string_lossy()))
    }
}

/// 把位置参数展开成待检查的文件：文件直接保留；目录递归展开（可用 glob 过滤）；不存在的路径记为错误。
pub fn expand_paths(raw_paths: &[PathBuf], glob: Option<&str>) -> (Vec<PathBuf>, Vec<String>) {
    let mut files: Vec<PathBuf> = Vec::new();
    let mut errors = Vec::new();
    for raw in raw_paths {
        if raw.is_file() {
            files.push(raw.clone());
        } else if raw.is_dir() {
            let mut stack = vec![raw.clone()];
            while let Some(directory) = stack.pop() {
                let mut entries: Vec<PathBuf> = fs::read_dir(&directory)
                    .into_iter()
                    .flatten()
                    .flatten()
                    .map(|entry| entry.path())
                    .collect();
                entries.sort();
                for path in entries {
                    if path.is_dir() {
                        let skip = path.file_name().is_some_and(|name| {
                            SKIPPED_DIRS.contains(&name.to_string_lossy().as_ref())
                        });
                        if !skip {
                            stack.push(path);
                        }
                    } else if path.is_file()
                        && glob.is_none_or(|pattern| glob_matches(&path, pattern))
                    {
                        files.push(path);
                    }
                }
            }
        } else {
            errors.push(format!(
                "Path does not exist or is not a regular file/directory: {}",
                raw.display()
            ));
        }
    }
    // 去重同时保持顺序。
    let mut seen = HashSet::new();
    files.retain(|path| seen.insert(path.canonicalize().unwrap_or_else(|_| path.clone())));
    (files, errors)
}

/// 读取白名单文件：每行一个相对仓库根的文件路径，`#` 开头为注释。路径给了却不存在时返回错误，
/// 避免白名单静默失效、让硬门禁形同虚设。
pub fn load_allow_list(path: Option<&Path>) -> (HashSet<String>, Vec<String>) {
    let Some(path) = path else {
        return (HashSet::new(), Vec::new());
    };
    match fs::read_to_string(path) {
        Ok(text) => (
            text.lines()
                .map(str::trim)
                .filter(|line| !line.is_empty() && !line.starts_with('#'))
                .map(str::to_string)
                .collect(),
            Vec::new(),
        ),
        Err(_) => (
            HashSet::new(),
            vec![format!(
                "Allow-list file does not exist: {}",
                path.display()
            )],
        ),
    }
}

/// `max-lines` 检查的参数。
#[derive(Debug, Clone)]
pub struct MaxLinesOptions {
    /// 非空行数上限。
    pub max_lines: usize,
    /// 仅警告，不返回非零退出码。
    pub warn_only: bool,
    /// 目录递归时的 glob 过滤。
    pub glob: Option<String>,
    /// 白名单文件。
    pub allow_list_file: Option<PathBuf>,
}

/// 运行文件行数检查，返回 `(退出码, 输出行)`。
pub fn check_max_lines(
    paths: &[PathBuf],
    options: &MaxLinesOptions,
    cwd: &Path,
) -> (i32, Vec<String>) {
    let (files, mut errors) = expand_paths(paths, options.glob.as_deref());
    let (allowed, allow_list_errors) = load_allow_list(options.allow_list_file.as_deref());
    errors.extend(allow_list_errors);

    let mut output: Vec<String> = errors
        .iter()
        .map(|error| format!("[ERROR] {error}"))
        .collect();
    let mut violations = 0;
    for file in &files {
        // 读不了（例如二进制文件）就跳过：行数检查只针对文本源码。
        let Ok(text) = fs::read_to_string(file) else {
            continue;
        };
        let lines = count_non_empty_lines(&text);
        if lines <= options.max_lines {
            continue;
        }
        let label = file
            .canonicalize()
            .ok()
            .and_then(|absolute| {
                absolute
                    .strip_prefix(cwd.canonicalize().unwrap_or_else(|_| cwd.to_path_buf()))
                    .map(Path::to_path_buf)
                    .ok()
            })
            .unwrap_or_else(|| file.clone())
            .to_string_lossy()
            .replace('\\', "/");
        if allowed.contains(&label) {
            output.push(format!(
                "[WARNING] {}: {lines} 非空行，超过上限 {} 行（已在白名单中）。",
                file.display(),
                options.max_lines
            ));
        } else {
            violations += 1;
            let level = if options.warn_only {
                "WARNING"
            } else {
                "ERROR"
            };
            output.push(format!(
                "[{level}] {}: {lines} 非空行，超过上限 {} 行。",
                file.display(),
                options.max_lines
            ));
        }
    }
    let failed = (!errors.is_empty() || violations > 0) && !options.warn_only;
    (i32::from(failed), output)
}

/// 守卫测试文件头的标记：约定每个守卫测试在文件头标注，让 AI 代理第一眼识别「这是规则本身，不是被规则约束的对象」。
pub const GUARD_TEST_MARKER: &str = "守卫测试（guard test）";
/// 只看文件头前这么多行。
const GUARD_MARKER_SCAN_LINES: usize = 40;

static TEST_FILE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(^|/)tests/.+\.rs$").expect("内置正则有效"));

/// 文件内容是否标注为守卫测试。
pub fn is_guard_test_content(text: &str) -> bool {
    text.lines()
        .take(GUARD_MARKER_SCAN_LINES)
        .any(|line| line.contains(GUARD_TEST_MARKER))
}

/// 暂存区里发生变更的守卫测试文件：`tests/` 目录下、文件头带 [`GUARD_TEST_MARKER`] 的 `.rs` 文件。
/// 新增/修改看暂存区里的内容，删除看 HEAD 里的内容（删除守卫测试同样是「改了规则」）。
pub fn staged_guard_tests(repo_root: &Path) -> Vec<String> {
    git::staged_files(repo_root, "ACDMRTUXB", &[])
        .into_iter()
        .filter(|path| TEST_FILE.is_match(path))
        .filter(|path| {
            let in_index = git::run(&["show", &format!(":{path}")], Some(repo_root));
            let content =
                in_index.or_else(|| git::run(&["show", &format!("HEAD:{path}")], Some(repo_root)));
            content.is_some_and(|text| is_guard_test_content(&text))
        })
        .collect()
}

/// 守卫测试保护的结论。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GuardDecision {
    /// 没有守卫测试被改动。
    NothingGuarded,
    /// 有改动，但已用 `GUARD_UPDATE_ACK` 确认。
    Acknowledged(Vec<String>),
    /// 有改动，没有确认：拒绝。
    Denied(Vec<String>),
}

/// 根据被改动的守卫测试与是否已确认，得出结论。
pub fn decide_guard(changed: Vec<String>, acknowledged: bool) -> GuardDecision {
    match (changed.is_empty(), acknowledged) {
        (true, _) => GuardDecision::NothingGuarded,
        (false, true) => GuardDecision::Acknowledged(changed),
        (false, false) => GuardDecision::Denied(changed),
    }
}

const PENDING_PREFIX: &str = "tasks/pending/";
const EVIDENCE_PREFIX: &str = "tasks/evidence/";

/// 从 staged 文件路径解析对应的 PRD stem：`tasks/pending/<stem>.md` 或 `tasks/evidence/<stem>/...`。
pub fn extract_prd_stem(staged_path: &str) -> Option<String> {
    let normalized = staged_path.replace('\\', "/");
    if let Some(name) = normalized.strip_prefix(PENDING_PREFIX) {
        return (!name.contains('/'))
            .then(|| name.strip_suffix(".md"))
            .flatten()
            .map(str::to_string);
    }
    let relative = normalized.strip_prefix(EVIDENCE_PREFIX)?;
    let first = relative.split('/').next().unwrap_or_default();
    (!first.is_empty()).then(|| first.to_string())
}

/// 提交触及他人新鲜锁持有的 PRD 时的警告文本（只警告，永不阻断）。
pub fn lock_conflict_warnings(
    staged_files: &[String],
    main_repo_root: &Path,
    current_label: &str,
    now: OffsetDateTime,
) -> Vec<String> {
    let mut stems: Vec<String> = staged_files
        .iter()
        .filter_map(|path| extract_prd_stem(path))
        .collect::<HashSet<_>>()
        .into_iter()
        .collect();
    stems.sort();

    let mut warnings = Vec::new();
    for stem in stems {
        let snapshot = lock::inspect(main_repo_root, &stem, now);
        if snapshot.state != LockState::Fresh {
            continue;
        }
        let field = |key: &str, default: &str| {
            snapshot
                .metadata
                .get(key)
                .and_then(|value| value.as_str())
                .filter(|value| !value.is_empty())
                .unwrap_or(default)
                .to_string()
        };
        if field("worktree", "") == current_label {
            continue;
        }
        warnings.push(format!(
            "[WARNING] tasks 路径触及 PRD '{stem}'，该 PRD 正被其他会话执行：工具 {}、分支 {}、worktree {}、最后心跳 {}。提交不被阻断，但请确认没有与对方会话重复开工。",
            field("ai_tool", "unknown"),
            field("branch", "unknown"),
            field("worktree", "主仓库"),
            field("heartbeat_at", "unknown"),
        ));
    }
    warnings
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write(root: &Path, relative: &str, content: &str) -> PathBuf {
        let path = root.join(relative);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, content).unwrap();
        path
    }

    fn options(max_lines: usize, warn_only: bool) -> MaxLinesOptions {
        MaxLinesOptions {
            max_lines,
            warn_only,
            glob: None,
            allow_list_file: None,
        }
    }

    #[test]
    fn only_blank_lines_are_excluded_from_the_count() {
        assert_eq!(count_non_empty_lines("a\n\n  \n// comment\n\t\nb\n"), 3);
        assert_eq!(count_non_empty_lines(""), 0);
    }

    #[test]
    fn files_over_the_limit_fail_unless_warn_only_and_the_boundary_is_inclusive() {
        let root = tempfile::tempdir().unwrap();
        let big = write(root.path(), "big.rs", &"x\n".repeat(11));
        let exact = write(root.path(), "exact.rs", &"x\n".repeat(10));

        let (code, output) = check_max_lines(
            std::slice::from_ref(&exact),
            &options(10, false),
            root.path(),
        );
        assert_eq!((code, output.len()), (0, 0), "恰好等于上限算通过");

        let (code, output) =
            check_max_lines(std::slice::from_ref(&big), &options(10, false), root.path());
        assert_eq!(code, 1);
        assert!(
            output[0].starts_with("[ERROR]") && output[0].contains("11 非空行"),
            "{output:?}"
        );

        let (code, output) = check_max_lines(&[big], &options(10, true), root.path());
        assert_eq!(code, 0, "warn-only 不阻断");
        assert!(output[0].starts_with("[WARNING]"), "{output:?}");
    }

    #[test]
    fn directories_expand_recursively_with_glob_and_skip_build_directories() {
        let root = tempfile::tempdir().unwrap();
        write(root.path(), "src/a.rs", &"x\n".repeat(20));
        write(root.path(), "src/deep/b.rs", &"x\n".repeat(20));
        write(root.path(), "src/c.txt", &"x\n".repeat(20));
        write(root.path(), "src/target/generated.rs", &"x\n".repeat(20));
        let mut opts = options(5, false);
        opts.glob = Some("*.rs".into());

        let (code, output) = check_max_lines(&[root.path().join("src")], &opts, root.path());

        assert_eq!(code, 1);
        let joined = output.join("\n");
        assert!(
            joined.contains("a.rs") && joined.contains("b.rs"),
            "{joined}"
        );
        assert!(
            !joined.contains("c.txt") && !joined.contains("generated.rs"),
            "{joined}"
        );
    }

    #[test]
    fn allow_listed_files_only_warn_and_a_missing_allow_list_is_an_error() {
        let root = tempfile::tempdir().unwrap();
        let legacy = write(root.path(), "legacy.rs", &"x\n".repeat(20));
        let allow = write(root.path(), "allow.txt", "# 存量超额\nlegacy.rs\n");
        let mut opts = options(5, false);
        opts.allow_list_file = Some(allow);

        let (code, output) = check_max_lines(std::slice::from_ref(&legacy), &opts, root.path());
        assert_eq!(code, 0);
        assert!(output[0].contains("已在白名单中"), "{output:?}");

        opts.allow_list_file = Some(root.path().join("missing.txt"));
        let (code, output) = check_max_lines(&[legacy], &opts, root.path());
        assert_eq!(code, 1, "白名单文件不存在不能静默失效");
        assert!(
            output
                .iter()
                .any(|line| line.contains("Allow-list file does not exist")),
            "{output:?}"
        );
    }

    #[test]
    fn a_missing_path_is_an_error() {
        let root = tempfile::tempdir().unwrap();
        let (code, output) = check_max_lines(
            &[root.path().join("nope.rs")],
            &options(5, false),
            root.path(),
        );
        assert_eq!(code, 1);
        assert!(output[0].contains("does not exist"));
    }

    #[test]
    fn glob_matching_handles_names_and_path_tails() {
        assert!(glob_matches(Path::new("a/b/c.rs"), "*.rs"));
        assert!(!glob_matches(Path::new("a/b/c.rs"), "*.py"));
        assert!(glob_matches(Path::new("crates/api/src/lib.rs"), "src/*.rs"));
        assert!(!glob_matches(
            Path::new("crates/api/tests/lib.rs"),
            "src/*.rs"
        ));
        assert!(glob_matches(Path::new("x.test.ts"), "*.test.ts"));
    }

    #[test]
    fn guard_tests_are_recognised_by_their_header_marker_only() {
        assert!(is_guard_test_content(
            "//! 守卫测试（guard test）：守护依赖方向。\nfn main() {}"
        ));
        assert!(!is_guard_test_content("//! 普通测试\nfn main() {}"));
        let far_below = format!("{}// 守卫测试（guard test）", "\n".repeat(60));
        assert!(
            !is_guard_test_content(&far_below),
            "只看文件头，正文里提到不算"
        );
    }

    #[test]
    fn guard_decision_requires_an_explicit_acknowledgement() {
        let changed = vec!["crates/app/tests/architecture.rs".to_string()];
        assert_eq!(decide_guard(vec![], false), GuardDecision::NothingGuarded);
        assert_eq!(decide_guard(vec![], true), GuardDecision::NothingGuarded);
        assert_eq!(
            decide_guard(changed.clone(), false),
            GuardDecision::Denied(changed.clone())
        );
        assert_eq!(
            decide_guard(changed.clone(), true),
            GuardDecision::Acknowledged(changed)
        );
    }

    #[test]
    fn prd_stems_are_extracted_from_pending_and_evidence_paths_only() {
        assert_eq!(
            extract_prd_stem("tasks/pending/P1-FEAT-20260601-000000-alpha.md").as_deref(),
            Some("P1-FEAT-20260601-000000-alpha")
        );
        assert_eq!(
            extract_prd_stem("tasks/evidence/stem/evidence-report.md").as_deref(),
            Some("stem")
        );
        assert_eq!(extract_prd_stem("tasks/pending/sub/x.md"), None);
        assert_eq!(extract_prd_stem("tasks/pending/notes.txt"), None);
        assert_eq!(
            extract_prd_stem("tasks/archive/P1-FEAT-20260601-000000-alpha.md"),
            None
        );
        assert_eq!(extract_prd_stem("src/main.rs"), None);
    }

    #[test]
    fn lock_conflict_warns_only_about_other_sessions_fresh_locks() {
        use crate::prd::lock::{Caller, claim};
        let root = tempfile::tempdir().unwrap();
        let now = OffsetDateTime::now_utc();
        let stem = "P1-FEAT-20260601-000000-alpha";
        let theirs = Caller {
            main_repo_root: root.path().to_path_buf(),
            worktree_label: "wt-other".into(),
            branch: "feat/x".into(),
        };
        claim(&theirs, stem, Some("codex"), now).unwrap();
        let staged = vec![
            format!("tasks/pending/{stem}.md"),
            format!("tasks/evidence/{stem}/plan.md"),
        ];

        let warnings = lock_conflict_warnings(&staged, root.path(), "wt-me", now);
        assert_eq!(warnings.len(), 1, "同一个 stem 只警告一次：{warnings:?}");
        assert!(
            warnings[0].contains("codex")
                && warnings[0].contains("wt-other")
                && warnings[0].contains("feat/x"),
            "{warnings:?}"
        );

        assert!(
            lock_conflict_warnings(&staged, root.path(), "wt-other", now).is_empty(),
            "自己持有的锁不警告"
        );
        assert!(
            lock_conflict_warnings(
                &staged,
                root.path(),
                "wt-me",
                now + time::Duration::minutes(45)
            )
            .is_empty(),
            "过期的锁不警告"
        );
        assert!(
            lock_conflict_warnings(&["src/main.rs".into()], root.path(), "wt-me", now).is_empty()
        );
    }
}

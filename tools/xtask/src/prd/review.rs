//! `prd review`：定位并打开 PRD 的人工审查清单。
//!
//! 交付收尾的人工审查入口：打开 `tasks/evidence/<prd-stem>/human-review-checklist.html`（交互版，不存在时回退到
//! 静态的 `.md`）——未完成 Human-Confirmed 项、人读呈递区与人工待决事项的集中审查页。清单尚不存在时回退到证据报告；
//! 两者都没有时由调用方列出证据目录现状并给出下一步提示。
//!
//! 与 Python 版相比不做「分支 worktree 副本优先」：只看主仓库的证据目录。

use std::fs;
use std::path::{Path, PathBuf};

use super::filename::{ARCHIVE_DIR, PENDING_DIR, parse_prd_filename};

/// 打开目标的槽位，按优先级排列：清单优先，证据报告兜底。清单槽位内交互 HTML 优先于静态 Markdown。
const REVIEW_TARGET_SLOTS: [(&str, &[&str]); 2] = [
    ("human-review-checklist", &[".html", ".md"]),
    ("evidence-report", &[".md"]),
];

/// 把命令行参数解析成 PRD 文件路径。
///
/// 参数可以是路径、文件名，或 **slug**（文件名里日期时间之后的短标识）。后两种宽形态解决一个真实痛点：
/// PRD 在交付收尾时由 `tasks/pending` 归档到 `tasks/archive`，于是「该传 pending 还是 archive」随当前状态而变；
/// 这里把它归一化，先查 `tasks/pending`，再查 `tasks/archive`。
pub fn resolve_prd_path(argument: &str, repo_root: &Path) -> Option<PathBuf> {
    for candidate in [PathBuf::from(argument), repo_root.join(argument)] {
        if candidate.is_file() {
            return Some(candidate);
        }
    }
    let wanted_name = Path::new(argument)
        .file_name()?
        .to_string_lossy()
        .into_owned();
    let wanted_stem = wanted_name
        .strip_suffix(".md")
        .unwrap_or(&wanted_name)
        .to_string();

    for bucket in [PENDING_DIR, ARCHIVE_DIR] {
        let directory = repo_root.join("tasks").join(bucket);
        let exact = directory.join(format!("{wanted_stem}.md"));
        if exact.is_file() {
            return Some(exact);
        }
        let mut entries: Vec<PathBuf> = fs::read_dir(&directory)
            .into_iter()
            .flatten()
            .flatten()
            .map(|entry| entry.path())
            .collect();
        entries.sort();
        if let Some(found) = entries
            .into_iter()
            .find(|path| path.is_file() && parse_prd_filename(path).slug == wanted_stem)
        {
            return Some(found);
        }
    }
    None
}

/// 在证据目录里按槽位优先级找到要打开的文件。
pub fn find_review_target(evidence_dir: &Path) -> Option<PathBuf> {
    let dir_name = evidence_dir.file_name()?.to_string_lossy().into_owned();
    for (kind, extensions) in REVIEW_TARGET_SLOTS {
        for extension in extensions {
            for file_name in [
                format!("{dir_name}.{kind}{extension}"),
                format!("{kind}{extension}"),
            ] {
                let candidate = evidence_dir.join(file_name);
                if candidate.is_file() {
                    return Some(candidate);
                }
            }
        }
    }
    None
}

/// 证据目录里现有的文件名（排序），用于「没找到可打开的目标」时给出现状。
pub fn list_evidence_files(evidence_dir: &Path) -> Vec<String> {
    let mut names: Vec<String> = fs::read_dir(evidence_dir)
        .into_iter()
        .flatten()
        .flatten()
        .map(|entry| entry.file_name().to_string_lossy().into_owned())
        .collect();
    names.sort();
    names
}

#[cfg(test)]
mod tests {
    use super::*;

    fn touch(root: &Path, relative: &str) {
        let path = root.join(relative);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, "x").unwrap();
    }

    #[test]
    fn a_prd_can_be_named_by_path_filename_stem_or_slug_in_either_directory() {
        let root = tempfile::tempdir().unwrap();
        touch(
            root.path(),
            "tasks/pending/P1-FEAT-20260601-000000-alpha.md",
        );
        touch(root.path(), "tasks/archive/P2-FIX-20260502-000000-beta.md");
        let found = |argument: &str| {
            resolve_prd_path(argument, root.path())
                .map(|path| path.file_name().unwrap().to_string_lossy().into_owned())
        };

        assert_eq!(
            found("tasks/pending/P1-FEAT-20260601-000000-alpha.md").as_deref(),
            Some("P1-FEAT-20260601-000000-alpha.md")
        );
        assert_eq!(
            found("P1-FEAT-20260601-000000-alpha.md").as_deref(),
            Some("P1-FEAT-20260601-000000-alpha.md")
        );
        assert_eq!(
            found("P1-FEAT-20260601-000000-alpha").as_deref(),
            Some("P1-FEAT-20260601-000000-alpha.md")
        );
        assert_eq!(
            found("alpha").as_deref(),
            Some("P1-FEAT-20260601-000000-alpha.md"),
            "slug"
        );
        assert_eq!(
            found("beta").as_deref(),
            Some("P2-FIX-20260502-000000-beta.md"),
            "归档后同一个参数仍然找得到"
        );
        assert_eq!(found("gamma"), None);
    }

    #[test]
    fn interactive_checklist_beats_static_which_beats_the_evidence_report() {
        let root = tempfile::tempdir().unwrap();
        let evidence = root.path().join("tasks/evidence/stem");
        assert_eq!(find_review_target(&evidence), None);

        touch(root.path(), "tasks/evidence/stem/evidence-report.md");
        assert_eq!(
            find_review_target(&evidence).unwrap().file_name().unwrap(),
            "evidence-report.md"
        );
        touch(root.path(), "tasks/evidence/stem/human-review-checklist.md");
        assert_eq!(
            find_review_target(&evidence).unwrap().file_name().unwrap(),
            "human-review-checklist.md"
        );
        touch(
            root.path(),
            "tasks/evidence/stem/stem.human-review-checklist.html",
        );
        assert_eq!(
            find_review_target(&evidence).unwrap().file_name().unwrap(),
            "stem.human-review-checklist.html"
        );

        assert_eq!(
            list_evidence_files(&evidence),
            [
                "evidence-report.md",
                "human-review-checklist.md",
                "stem.human-review-checklist.html"
            ]
        );
    }
}

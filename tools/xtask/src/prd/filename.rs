//! PRD 文件名与路径的解析。

use std::path::Path;
use std::sync::LazyLock;

use regex::Regex;

/// PRD 目录：执行态。
pub const PENDING_DIR: &str = "pending";
/// PRD 目录：收尾态。
pub const ARCHIVE_DIR: &str = "archive";

/// `P1-FEAT-20260608-173014-slug.md`；优先级与类型前缀、时间部分都可省略（旧式命名）。
static FILENAME: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"^(?:P(?P<priority>\d+)-)?(?:(?P<kind>[A-Z]+)-)?(?P<date>\d{8})-(?:(?P<time>\d{6})-)?(?P<slug>.+)\.md$",
    )
    .expect("内置正则有效")
});

/// 「看起来是 PRD」的文件名：旧式 `<anything>-prd-<anything>.md`，或 `P0-3` 优先级加类型的新式命名。
/// 用来把 `tasks/README.md` 之类的路由页排除在外。
static PRD_NAME: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^(?:[^/]+-prd-[^/]+|P[0-3]-[A-Z]+-\d{8}-\d{6}-[^/]+)\.md$").expect("内置正则有效")
});

/// 从 PRD 文件名解析出的信息。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PrdName {
    /// 优先级标签，例如 `P1`；旧式命名无前缀时为空串。
    pub priority: String,
    /// 类型标签，例如 `FEAT`；旧式命名无类型时为空串。
    pub kind: String,
    /// 创建日期 `YYYY-MM-DD`，无法解析时为空串。
    pub created: String,
    /// 去掉前缀与扩展名后的短标识。
    pub slug: String,
}

/// 解析 PRD 文件名。任何一项无法解析时为空串；完全不符合命名时 `slug` 取文件的 stem。
pub fn parse_prd_filename(path: &Path) -> PrdName {
    let file_name = path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or_default();
    let Some(captures) = FILENAME.captures(file_name) else {
        return PrdName {
            slug: path
                .file_stem()
                .and_then(|stem| stem.to_str())
                .unwrap_or_default()
                .to_string(),
            ..PrdName::default()
        };
    };
    let date = &captures["date"];
    PrdName {
        priority: captures
            .name("priority")
            .map(|priority| format!("P{}", priority.as_str()))
            .unwrap_or_default(),
        kind: captures
            .name("kind")
            .map(|kind| kind.as_str().to_string())
            .unwrap_or_default(),
        created: format!("{}-{}-{}", &date[0..4], &date[4..6], &date[6..8]),
        slug: captures["slug"].to_string(),
    }
}

/// 文件名是否像一个 PRD。
pub fn looks_like_prd(file_name: &str) -> bool {
    PRD_NAME.is_match(file_name)
}

/// 路径（仓库相对，正斜杠）是否是 `tasks/archive/` 下的 PRD。
pub fn is_archived_prd(repo_relative_path: &str) -> bool {
    repo_relative_path
        .strip_prefix("tasks/archive/")
        .is_some_and(|name| !name.contains('/') && looks_like_prd(name))
}

/// 路径（仓库相对，正斜杠）是否是 `tasks/pending/` 下的 PRD。
pub fn is_pending_prd(repo_relative_path: &str) -> bool {
    repo_relative_path
        .strip_prefix("tasks/pending/")
        .is_some_and(|name| !name.contains('/') && looks_like_prd(name))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn full_name_is_split_into_priority_kind_date_and_slug() {
        let name = parse_prd_filename(Path::new(
            "tasks/pending/P2-FEAT-20260915-112126-prd-execution-lock.md",
        ));
        assert_eq!(
            name,
            PrdName {
                priority: "P2".into(),
                kind: "FEAT".into(),
                created: "2026-09-15".into(),
                slug: "prd-execution-lock".into(),
            }
        );
    }

    #[test]
    fn legacy_name_without_priority_or_kind_still_yields_date_and_slug() {
        let name = parse_prd_filename(Path::new("20260516-184900-prd-decouple-planning.md"));
        assert_eq!((name.priority.as_str(), name.kind.as_str()), ("", ""));
        assert_eq!(name.created, "2026-05-16");
        assert_eq!(name.slug, "prd-decouple-planning");
    }

    #[test]
    fn unparseable_name_falls_back_to_the_stem() {
        let name = parse_prd_filename(Path::new("tasks/pending/notes.md"));
        assert_eq!(name.slug, "notes");
        assert_eq!(name.created, "");
    }

    #[test]
    fn only_prd_shaped_files_in_the_right_directory_are_recognised() {
        assert!(is_archived_prd(
            "tasks/archive/P1-FEAT-20260608-000000-extract-ops-toolkit.md"
        ));
        assert!(is_archived_prd(
            "tasks/archive/20260516-184900-prd-old-style.md"
        ));
        assert!(is_pending_prd(
            "tasks/pending/P2-REFACTOR-20260605-181245-split.md"
        ));

        assert!(
            !is_archived_prd("tasks/archive/README.md"),
            "路由页不是 PRD"
        );
        assert!(
            !is_archived_prd("tasks/pending/P1-FEAT-20260608-000000-x.md"),
            "目录不对"
        );
        assert!(
            !is_pending_prd("tasks/pending/sub/P1-FEAT-20260608-000000-x.md"),
            "子目录里的不算"
        );
        assert!(!is_archived_prd(
            "docs/tasks/archive/P1-FEAT-20260608-000000-x.md"
        ));
    }
}

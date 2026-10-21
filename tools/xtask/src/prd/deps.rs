//! 交付依赖（PRD §8 Delivery Dependencies）的解析与判定。
//!
//! 依赖可见性是开工前判断交付顺序的唯一入口：被挡住的下游 PRD 不能静默显示 `-`，所以三态
//! （`blocked` / `无法判定` / `deps ok`）的判定收在这里。

use std::path::Path;
use std::sync::LazyLock;

use regex::Regex;

use super::filename::parse_prd_filename;

static SECTION: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^##\s+(?:\d+[.、]\s*)?(?:Delivery Dependencies|交付依赖)\s*$")
        .expect("内置正则有效")
});
static GATE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)^[-*]\s*Gate type\s*[:：]\s*(?P<gate>.+?)\s*$").expect("内置正则有效")
});
static REFS_HEADING: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)^[-*]\s*Depends on tasks/issues\s*[:：]\s*$").expect("内置正则有效")
});
static NESTED_REF: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^\s+[-*]\s+(?P<ref>.+?)\s*$").expect("内置正则有效"));

/// 解析出的交付依赖。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct DeliveryDependencies {
    /// `Gate type`，统一小写；未声明时为空串。
    pub gate: String,
    /// `Depends on tasks/issues` 下的引用原文；`none` 已剔除。
    pub refs: Vec<String>,
}

/// 解析 §8 的 `Gate type` 与 `Depends on tasks/issues`。组依赖（`Depends on groups`）需要展开
/// 成员，不参与本地判定，忽略。无 §8 章节时返回默认值。
pub fn parse_delivery_dependencies(content: &str) -> DeliveryDependencies {
    let lines: Vec<&str> = content.lines().collect();
    let Some(start) = lines.iter().position(|line| SECTION.is_match(line)) else {
        return DeliveryDependencies::default();
    };

    let mut result = DeliveryDependencies::default();
    let mut collecting_refs = false;
    for line in &lines[start + 1..] {
        if line.starts_with("## ") {
            break;
        }
        if let Some(gate) = GATE.captures(line) {
            result.gate = gate["gate"].trim().trim_matches('`').to_lowercase();
            collecting_refs = false;
            continue;
        }
        if REFS_HEADING.is_match(line) {
            collecting_refs = true;
            continue;
        }
        if collecting_refs {
            let Some(reference) = NESTED_REF.captures(line) else {
                collecting_refs = false;
                continue;
            };
            let reference = reference["ref"].trim().trim_matches('`').trim();
            if !reference.is_empty() && !reference.eq_ignore_ascii_case("none") {
                result.refs.push(reference.to_string());
            }
        }
    }
    result
}

/// 单条依赖引用的交付状态。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RefState {
    /// 依赖的 PRD 还在 `tasks/pending`：未交付。
    Pending(String),
    /// 依赖的 PRD 已在 `tasks/archive`：已交付。
    Delivered(String),
    /// 两处都没有（悬空引用），或是 Issue 引用（需要远端状态，本地无法判定）。
    Unknown(String),
}

/// 判定一条依赖引用的交付状态。引用可能是 `tasks/pending/xxx.md` 完整路径、裸文件名或 stem，
/// 或 `#123` Issue 引用。
pub fn classify_ref(reference: &str, pending_dir: &Path, archive_dir: &Path) -> RefState {
    if reference.starts_with('#') || reference.chars().all(|c| c.is_ascii_digit()) {
        return RefState::Unknown(format!("#{}", reference.trim_start_matches('#')));
    }
    let name = Path::new(reference)
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or(reference);
    let stem = name.strip_suffix(".md").unwrap_or(name);
    let slug = parse_prd_filename(Path::new(&format!("{stem}.md"))).slug;

    if pending_dir.join(format!("{stem}.md")).is_file() {
        RefState::Pending(slug)
    } else if archive_dir.join(format!("{stem}.md")).is_file() {
        RefState::Delivered(slug)
    } else {
        RefState::Unknown(slug)
    }
}

/// 依赖列的展示结果。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DepsCell {
    /// 无需关注（`none` gate 或无 §8）。
    None,
    /// `hard` gate 下存在未交付的依赖。
    Blocked(String),
    /// 无未交付依赖，但有本地无法判定的引用。
    Unknown(String),
    /// 全部满足。
    Ok,
    /// `soft` gate 下的提示。
    Soft(String),
}

fn shorten(slug: &str) -> String {
    const MAX: usize = 26;
    if slug.chars().count() <= MAX {
        slug.to_string()
    } else {
        format!("{}…", slug.chars().take(MAX - 1).collect::<String>())
    }
}

/// 判定一条 PRD 的依赖列：`hard` gate 下有未交付依赖 → 被挡住；没有但有无法判定的引用 → 无法判定；
/// 全部满足 → ok。`soft` gate 只在有未交付依赖时提示。
pub fn classify_dependencies(
    dependencies: &DeliveryDependencies,
    pending_dir: &Path,
    archive_dir: &Path,
) -> DepsCell {
    if dependencies.gate.is_empty() || dependencies.gate == "none" {
        return DepsCell::None;
    }
    let mut blocked = Vec::new();
    let mut unknown = Vec::new();
    for reference in &dependencies.refs {
        match classify_ref(reference, pending_dir, archive_dir) {
            RefState::Pending(slug) => blocked.push(slug),
            RefState::Unknown(slug) => unknown.push(slug),
            RefState::Delivered(_) => {}
        }
    }

    if dependencies.gate == "hard" {
        if let Some(first) = blocked.first() {
            let extra = if blocked.len() > 1 {
                format!(" +{}", blocked.len() - 1)
            } else {
                String::new()
            };
            return DepsCell::Blocked(format!("{}{extra}", shorten(first)));
        }
        if !unknown.is_empty() {
            let shown: Vec<_> = unknown.iter().take(2).map(|slug| shorten(slug)).collect();
            let extra = if unknown.len() > 2 {
                format!(" +{}", unknown.len() - 2)
            } else {
                String::new()
            };
            return DepsCell::Unknown(format!("{}{extra}", shown.join(", ")));
        }
        return DepsCell::Ok;
    }

    match blocked.first() {
        Some(first) => DepsCell::Soft(shorten(first)),
        None => DepsCell::None,
    }
}

#[cfg(test)]
mod tests {
    use std::fs;

    use super::*;

    const SECTION_TEXT: &str = "\
## 8. Delivery Dependencies

### Delivery Dependencies
- Gate type: hard
- Depends on tasks/issues:
  - `tasks/pending/P1-FEAT-20260601-000000-upstream.md`
  - #42
  - none
- Notes: prose
";

    #[test]
    fn parses_gate_and_nested_refs_and_drops_none() {
        let parsed = parse_delivery_dependencies(SECTION_TEXT);
        assert_eq!(parsed.gate, "hard");
        assert_eq!(
            parsed.refs,
            ["tasks/pending/P1-FEAT-20260601-000000-upstream.md", "#42"]
        );
        assert_eq!(
            parse_delivery_dependencies("# no section"),
            DeliveryDependencies::default()
        );
    }

    fn dirs() -> (tempfile::TempDir, std::path::PathBuf, std::path::PathBuf) {
        let root = tempfile::tempdir().unwrap();
        let pending = root.path().join("pending");
        let archive = root.path().join("archive");
        fs::create_dir_all(&pending).unwrap();
        fs::create_dir_all(&archive).unwrap();
        (root, pending, archive)
    }

    #[test]
    fn hard_gate_is_blocked_by_a_pending_upstream_and_ok_once_it_is_archived() {
        let (_root, pending, archive) = dirs();
        let dependencies = DeliveryDependencies {
            gate: "hard".into(),
            refs: vec!["P1-FEAT-20260601-000000-upstream".into()],
        };

        fs::write(pending.join("P1-FEAT-20260601-000000-upstream.md"), "").unwrap();
        assert_eq!(
            classify_dependencies(&dependencies, &pending, &archive),
            DepsCell::Blocked("upstream".into())
        );

        fs::rename(
            pending.join("P1-FEAT-20260601-000000-upstream.md"),
            archive.join("P1-FEAT-20260601-000000-upstream.md"),
        )
        .unwrap();
        assert_eq!(
            classify_dependencies(&dependencies, &pending, &archive),
            DepsCell::Ok
        );
    }

    #[test]
    fn dangling_and_issue_refs_are_unknown_not_silently_ok() {
        let (_root, pending, archive) = dirs();
        let dependencies = DeliveryDependencies {
            gate: "hard".into(),
            refs: vec!["#42".into(), "P9-FEAT-20260101-000000-ghost.md".into()],
        };
        assert_eq!(
            classify_dependencies(&dependencies, &pending, &archive),
            DepsCell::Unknown("#42, ghost".into())
        );
    }

    #[test]
    fn soft_gate_only_hints_and_none_gate_is_quiet() {
        let (_root, pending, archive) = dirs();
        fs::write(pending.join("P1-FEAT-20260601-000000-upstream.md"), "").unwrap();
        let soft = DeliveryDependencies {
            gate: "soft".into(),
            refs: vec!["P1-FEAT-20260601-000000-upstream".into()],
        };
        assert_eq!(
            classify_dependencies(&soft, &pending, &archive),
            DepsCell::Soft("upstream".into())
        );
        assert_eq!(
            classify_dependencies(&DeliveryDependencies::default(), &pending, &archive),
            DepsCell::None
        );
    }
}

//! PRD 状态看板。
//!
//! 扫描 `tasks/pending` 与 `tasks/archive`，汇总每条 PRD 的优先级、类型、创建时间、验收清单勾选进度、
//! 验收状态横幅、交付依赖（DEPS）、执行锁（LOCK）与证据包状态（EVIDENCE），供开工前判断哪些尚未交付、
//! 哪些被上游依赖挡住、哪些正被其他会话执行。
//!
//! 目录轴与验收轴相互正交：`tasks/archive` 只代表执行侧交付已完成，人工验收可以晚于归档。因此看板在
//! `PENDING`（未开工 / 进行中）与 `ARCHIVE` 之外另列一段 `AWAITING HUMAN`——读到的那份 PRD 副本已在
//! `tasks/archive`，且验收状态横幅为 `🧍 待人工验收`，才算「机器证据齐备、只等人确认」，并从各自的目录分区里
//! 摘出，免得归档月视图把它们尚未勾选的 Human-Confirmed 项当成异常。分区只做呈现，不搬文件。
//!
//! **落在 `tasks/pending` 的副本即使横幅写着 🧍 也不进这一段**：按生命周期，归档与横幅翻转在同一次交付里完成；
//! pending 里的 🧍 只可能是重开后忘了复位横幅，或翻了横幅还没 `git mv`——此时执行侧的活可能还没做完，
//! 必须留在 PENDING 里被看见。同名 PRD 在 pending 与 archive 并存时以 pending 副本为准。
//!
//! 与 Python 版相比刻意**不做**的两件事（见 `docs/migration-status.md`）：按分支 worktree 副本优先读取进度、
//! 以及「影响树触达进度（FILES 列）」。看板读取主仓库里的副本，进度以主线为准。
//!
//! 进度、依赖满足与 verifier 结论都是从文件内容推断的启发式结果，看板只用于快速定位，最终判断以 PRD 正文与证据文件为准。

use std::collections::{BTreeMap, HashSet};
use std::fmt::Write as _;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::LazyLock;

use regex::Regex;
use time::OffsetDateTime;
use unicode_width::UnicodeWidthStr;

use super::contract::{AcceptanceStatus, parse_acceptance_status, parse_checklist};
use super::deps::{DepsCell, classify_dependencies, parse_delivery_dependencies};
use super::filename::{ARCHIVE_DIR, PENDING_DIR, PrdName, looks_like_prd, parse_prd_filename};
use super::lock::{self, LockState};

static STRONG_VERDICT: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?m)^\s*[-*>#\s]*(?:VERDICT|最终结论|结论)\s*[:：]\s*\**\s*(PASS|REJECT)\b")
        .expect("内置正则有效")
});
static STANDALONE_VERDICT: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?m)^\s*[-*>#\s]*\**\s*(PASS|REJECT)\**\s*(?:[（(].*)?$").expect("内置正则有效")
});
static ANY_VERDICT: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\b(PASS|REJECT)\b").expect("内置正则有效"));
static TITLE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^#\s+(?P<title>[^!].*?)\s*$").expect("内置正则有效"));
static INTRODUCTION_HEADING: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^##\s+(?:\d+[.、]\s*)?Introduction(?:\s*&\s*Goals)?\s*$").expect("内置正则有效")
});

const SUMMARY_MAX_LINES: usize = 4;
const SUMMARY_LINE_MAX_CHARS: usize = 110;

/// 看板展示哪一部分。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Scope {
    /// 全部。
    All,
    /// 只看 pending（含待人工验收那一段之外的）。
    Pending,
    /// 只看 archive。
    Archive,
}

/// 单条 PRD 的看板信息。
#[derive(Debug, Clone)]
pub struct PrdRecord {
    /// PRD 文件路径。
    pub path: PathBuf,
    /// 所在目录：`pending` 或 `archive`。
    pub bucket: String,
    /// 文件名解析结果。
    pub name: PrdName,
    /// 文件名去掉 `.md`。
    pub stem: String,
    /// 已勾选的验收项数。
    pub checked: usize,
    /// 验收项总数；无清单时为 0。
    pub total: usize,
    /// 验收状态横幅；无横幅或无法识别时为 `None`。
    pub acceptance: Option<AcceptanceStatus>,
    /// 是否有验证计划文件。
    pub has_verification_plan: bool,
    /// 是否有证据报告文件。
    pub has_evidence_report: bool,
    /// verifier 结论：`PASS` / `REJECT` / 空串。
    pub verifier_verdict: String,
    /// 结论是否来自显式结论行而非散文推断。
    pub verdict_is_certain: bool,
    /// §8 声明的依赖。
    pub dependencies: super::deps::DeliveryDependencies,
    /// 一级标题。
    pub title: String,
    /// `--detail` 摘要行。
    pub summary_lines: Vec<String>,
}

impl PrdRecord {
    /// 清单是否已全部勾选；无清单时视为不完整。
    pub fn checklist_complete(&self) -> bool {
        self.total > 0 && self.checked == self.total
    }

    /// 是否「机器证据齐备、只等人确认」：读到的副本在 archive，且横幅为 `🧍 待人工验收`。
    pub fn awaits_human_review(&self) -> bool {
        self.bucket == ARCHIVE_DIR && self.acceptance == Some(AcceptanceStatus::AwaitingHuman)
    }
}

/// 在证据目录里找 `<目录名>.<kind>.md` 或 `<kind>.md`。
fn find_evidence_file(evidence_dir: &Path, kind: &str) -> Option<PathBuf> {
    let dir_name = evidence_dir.file_name()?.to_string_lossy().into_owned();
    [format!("{dir_name}.{kind}.md"), format!("{kind}.md")]
        .into_iter()
        .map(|file_name| evidence_dir.join(file_name))
        .find(|candidate| candidate.is_file())
}

/// 从 verifier 报告推断最终结论。报告可能是「先 REJECT、修复后复审 PASS」的多段结构，所以取**最后一次**出现的结论。
/// 显式结论行（`VERDICT:` / `结论：`）优先，其次整行独立的 `PASS`/`REJECT`，最后才回退到散文里的任意出现。
/// 返回 `(结论, 是否来自显式结论行)`。
pub fn parse_verifier_verdict(report: &str) -> (String, bool) {
    for pattern in [&*STRONG_VERDICT, &*STANDALONE_VERDICT] {
        if let Some(last) = pattern.captures_iter(report).last() {
            return (last[1].to_uppercase(), true);
        }
    }
    match ANY_VERDICT.captures_iter(report).last() {
        Some(last) => (last[1].to_uppercase(), false),
        None => (String::new(), false),
    }
}

/// 提取一级标题与描述摘要：优先取 `Introduction & Goals` 章节正文，缺章节时退化为一级标题之后、首个小节之前的引言。
fn extract_title_and_summary(content: &str) -> (String, Vec<String>) {
    let lines: Vec<&str> = content.lines().collect();
    let title_index = lines.iter().position(|line| TITLE.is_match(line));
    let title = title_index
        .and_then(|index| TITLE.captures(lines[index]))
        .map(|captures| captures["title"].to_string())
        .unwrap_or_default();

    let body_start = match lines
        .iter()
        .position(|line| INTRODUCTION_HEADING.is_match(line))
    {
        Some(index) => index + 1,
        None => title_index.map_or(0, |index| index + 1),
    };
    let body_end = (body_start..lines.len())
        .find(|&index| lines[index].starts_with("## ") || lines[index].starts_with("# "))
        .unwrap_or(lines.len());
    // 摘要只取正文：标题行（`### Problem Statement` 之类）不是描述，混进来只会占掉有限的行数。
    let body: Vec<&str> = lines[body_start..body_end]
        .iter()
        .map(|line| line.trim())
        .filter(|line| !line.is_empty() && !line.starts_with('#'))
        .collect();

    let summary = body
        .iter()
        .take(SUMMARY_MAX_LINES)
        .enumerate()
        .map(|(index, line)| {
            let overflow = index == SUMMARY_MAX_LINES - 1 && body.len() > SUMMARY_MAX_LINES;
            if line.chars().count() > SUMMARY_LINE_MAX_CHARS {
                format!(
                    "{}…",
                    line.chars()
                        .take(SUMMARY_LINE_MAX_CHARS - 1)
                        .collect::<String>()
                )
            } else if overflow {
                format!("{line}…")
            } else {
                (*line).to_string()
            }
        })
        .collect();
    (title, summary)
}

/// 读取单个 PRD 与它的证据目录，生成看板记录。
pub fn collect_record(
    prd_path: &Path,
    bucket: &str,
    evidence_root: &Path,
) -> std::io::Result<PrdRecord> {
    let content = fs::read_to_string(prd_path)?;
    let name = parse_prd_filename(prd_path);
    let stem = prd_path
        .file_stem()
        .map(|stem| stem.to_string_lossy().into_owned())
        .unwrap_or_default();
    let (checked, total) = parse_checklist(&content).progress();
    let (title, summary_lines) = extract_title_and_summary(&content);

    let evidence_dir = evidence_root.join(&stem);
    let (verifier_verdict, verdict_is_certain) =
        find_evidence_file(&evidence_dir, "verifier-report")
            .and_then(|path| fs::read_to_string(path).ok())
            .map(|report| parse_verifier_verdict(&report))
            .unwrap_or_default();

    Ok(PrdRecord {
        path: prd_path.to_path_buf(),
        bucket: bucket.to_string(),
        name,
        stem,
        checked,
        total,
        acceptance: parse_acceptance_status(&content),
        has_verification_plan: find_evidence_file(&evidence_dir, "verification-plan").is_some(),
        has_evidence_report: find_evidence_file(&evidence_dir, "evidence-report").is_some(),
        verifier_verdict,
        verdict_is_certain,
        dependencies: parse_delivery_dependencies(&content),
        title,
        summary_lines,
    })
}

/// 扫描一个目录下的 PRD，按文件名排序（优先级前缀在前）。读不了的文件跳过，并把原因放进返回的警告里。
fn collect_bucket(
    tasks_dir: &Path,
    bucket: &str,
    evidence_root: &Path,
) -> (Vec<PrdRecord>, Vec<String>) {
    let directory = tasks_dir.join(bucket);
    let mut paths: Vec<PathBuf> = fs::read_dir(&directory)
        .into_iter()
        .flatten()
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| {
            path.is_file()
                && path
                    .file_name()
                    .and_then(|name| name.to_str())
                    .is_some_and(looks_like_prd)
        })
        .collect();
    paths.sort();

    let mut records = Vec::new();
    let mut warnings = Vec::new();
    for path in paths {
        match collect_record(&path, bucket, evidence_root) {
            Ok(record) => records.push(record),
            Err(error) => warnings.push(format!("无法读取 {}：{error}", path.display())),
        }
    }
    (records, warnings)
}

/// 颜色包装：只在输出到终端且没设 `NO_COLOR` 时才着色。
#[derive(Debug, Clone, Copy)]
pub struct Palette {
    /// 是否输出 ANSI 颜色。
    pub enabled: bool,
}

impl Palette {
    fn paint(&self, code: &str, text: &str) -> String {
        if self.enabled {
            format!("\x1b[{code}m{text}\x1b[0m")
        } else {
            text.to_string()
        }
    }
    fn bold(&self, text: &str) -> String {
        self.paint("1", text)
    }
    fn dim(&self, text: &str) -> String {
        self.paint("2", text)
    }
    fn red(&self, text: &str) -> String {
        self.paint("31", text)
    }
    fn green(&self, text: &str) -> String {
        self.paint("32", text)
    }
    fn yellow(&self, text: &str) -> String {
        self.paint("33", text)
    }
}

/// 把记录按验收轴分流：`(待人工验收, 其余 pending, 其余 archive)`。同名 PRD 在两处并存时以 pending 为准。
pub fn split_awaiting_human(
    pending: Vec<PrdRecord>,
    archive: Vec<PrdRecord>,
) -> (Vec<PrdRecord>, Vec<PrdRecord>, Vec<PrdRecord>) {
    let pending_stems: HashSet<String> = pending.iter().map(|record| record.stem.clone()).collect();
    let mut awaiting: Vec<PrdRecord> = pending
        .iter()
        .chain(archive.iter())
        .filter(|record| record.awaits_human_review())
        .filter(|record| record.bucket == PENDING_DIR || !pending_stems.contains(&record.stem))
        .cloned()
        .collect();
    awaiting.sort_by(|a, b| a.path.file_name().cmp(&b.path.file_name()));
    let claimed: HashSet<PathBuf> = awaiting.iter().map(|record| record.path.clone()).collect();
    let rest = |records: Vec<PrdRecord>| {
        records
            .into_iter()
            .filter(|record| !claimed.contains(&record.path))
            .collect::<Vec<_>>()
    };
    (awaiting, rest(pending), rest(archive))
}

/// 渲染上下文。
pub struct RenderContext<'a> {
    /// 主仓库根目录（查询执行锁、依赖状态）。
    pub repo_root: &'a Path,
    /// 判定锁过期用的当前时间。
    pub now: OffsetDateTime,
    /// 颜色。
    pub palette: Palette,
    /// 是否在每行下方追加标题与摘要。
    pub detail: bool,
}

fn pad(text: &str, width: usize) -> String {
    let visible = visible_width(text);
    format!("{text}{}", " ".repeat(width.saturating_sub(visible)))
}

/// 去掉 ANSI 转义后的显示宽度（中日韩字符与 emoji 按两列算）。
fn visible_width(text: &str) -> usize {
    static ANSI: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r"\x1b\[[0-9;]*m").expect("内置正则有效"));
    ANSI.replace_all(text, "").width()
}

fn checklist_cell(record: &PrdRecord, palette: Palette) -> String {
    if record.total == 0 {
        return palette.dim("-");
    }
    let text = format!("{}/{}", record.checked, record.total);
    if record.checklist_complete() {
        palette.green(&format!("{text} ✓"))
    } else if record.checked == 0 {
        palette.dim(&text)
    } else {
        palette.yellow(&text)
    }
}

fn acceptance_cell(record: &PrdRecord, palette: Palette) -> String {
    match record.acceptance {
        Some(AcceptanceStatus::NotStarted) => palette.dim("⬜ 未开工"),
        Some(AcceptanceStatus::AwaitingHuman) => palette.yellow("🧍 待人工验收"),
        Some(AcceptanceStatus::Accepted) => palette.green("✅ 已验收"),
        None => palette.dim("-"),
    }
}

fn deps_cell(record: &PrdRecord, ctx: &RenderContext<'_>) -> String {
    let tasks_dir = ctx.repo_root.join("tasks");
    match classify_dependencies(
        &record.dependencies,
        &tasks_dir.join(PENDING_DIR),
        &tasks_dir.join(ARCHIVE_DIR),
    ) {
        DepsCell::None => ctx.palette.dim("-"),
        DepsCell::Blocked(slug) => ctx.palette.red(&format!("⛔ blocked by {slug}")),
        DepsCell::Unknown(slug) => ctx.palette.yellow(&format!("? {slug}")),
        DepsCell::Ok => ctx.palette.green("✔ deps ok"),
        DepsCell::Soft(slug) => ctx.palette.dim(&format!("soft → {slug}")),
    }
}

fn lock_cell(record: &PrdRecord, ctx: &RenderContext<'_>) -> String {
    if record.bucket != PENDING_DIR {
        return ctx.palette.dim("-");
    }
    let snapshot = lock::inspect(ctx.repo_root, &record.stem, ctx.now);
    let text = |key: &str| {
        snapshot
            .metadata
            .get(key)
            .and_then(|value| value.as_str())
            .filter(|value| !value.is_empty())
            .unwrap_or("?")
            .to_string()
    };
    match snapshot.state {
        LockState::None => ctx.palette.dim("-"),
        LockState::Fresh => {
            ctx.palette
                .green(&format!("🔒 {}@{}", text("ai_tool"), text("branch")))
        }
        LockState::Stale => {
            ctx.palette
                .yellow(&format!("⚠ stale {}@{}", text("ai_tool"), text("branch")))
        }
    }
}

fn evidence_cell(record: &PrdRecord, palette: Palette) -> String {
    let mut parts = Vec::new();
    if record.has_verification_plan {
        parts.push("plan".to_string());
    }
    if record.has_evidence_report {
        parts.push("report".to_string());
    }
    let verdict = match (record.verifier_verdict.as_str(), record.verdict_is_certain) {
        ("", _) => None,
        ("PASS", true) => Some(palette.green("PASS")),
        ("PASS", false) => Some(palette.dim("PASS?")),
        ("REJECT", true) => Some(palette.red("REJECT")),
        (_, _) => Some(palette.yellow("REJECT?")),
    };
    if parts.is_empty() && verdict.is_none() {
        return palette.dim("-");
    }
    let mut text = parts.join("+");
    if let Some(verdict) = verdict {
        if !text.is_empty() {
            text.push(' ');
        }
        text.push_str(&verdict);
    }
    text
}

fn render_table(records: &[PrdRecord], ctx: &RenderContext<'_>, out: &mut String) {
    let headers = [
        "PRI",
        "TYPE",
        "DATE",
        "SLUG",
        "CHECKLIST",
        "ACCEPT",
        "DEPS",
        "LOCK",
        "EVIDENCE",
    ];
    let rows: Vec<Vec<String>> = records
        .iter()
        .map(|record| {
            vec![
                if record.name.priority.is_empty() {
                    ctx.palette.dim("-")
                } else {
                    record.name.priority.clone()
                },
                if record.name.kind.is_empty() {
                    ctx.palette.dim("-")
                } else {
                    record.name.kind.clone()
                },
                if record.name.created.is_empty() {
                    ctx.palette.dim("-")
                } else {
                    record.name.created.clone()
                },
                record.name.slug.clone(),
                checklist_cell(record, ctx.palette),
                acceptance_cell(record, ctx.palette),
                deps_cell(record, ctx),
                lock_cell(record, ctx),
                evidence_cell(record, ctx.palette),
            ]
        })
        .collect();

    let widths: Vec<usize> = (0..headers.len())
        .map(|column| {
            rows.iter()
                .map(|row| visible_width(&row[column]))
                .chain(std::iter::once(headers[column].width()))
                .max()
                .unwrap_or(0)
        })
        .collect();

    let header_line = headers
        .iter()
        .zip(&widths)
        .map(|(header, width)| pad(&ctx.palette.dim(header), *width))
        .collect::<Vec<_>>()
        .join("  ");
    let _ = writeln!(out, "  {}", header_line.trim_end());
    for (record, row) in records.iter().zip(&rows) {
        let line = row
            .iter()
            .zip(&widths)
            .map(|(cell, width)| pad(cell, *width))
            .collect::<Vec<_>>()
            .join("  ");
        let _ = writeln!(out, "  {}", line.trim_end());
        if ctx.detail {
            if !record.title.is_empty() {
                let _ = writeln!(out, "      {}", ctx.palette.bold(&record.title));
            }
            for summary in &record.summary_lines {
                let _ = writeln!(out, "      {}{summary}", ctx.palette.dim("▏ "));
            }
        }
    }
}

/// archive 按月折叠：每月一行概览，清单没勾完的逐条点名。
fn render_archive_months(records: &[PrdRecord], palette: Palette, out: &mut String) {
    let mut by_month: BTreeMap<String, Vec<&PrdRecord>> = BTreeMap::new();
    for record in records {
        let month = if record.name.created.len() >= 7 {
            record.name.created[..7].to_string()
        } else {
            "未知日期".to_string()
        };
        by_month.entry(month).or_default().push(record);
    }
    for (month, monthly) in by_month.iter().rev() {
        let with_checklist: Vec<_> = monthly.iter().filter(|record| record.total > 0).collect();
        let incomplete: Vec<_> = with_checklist
            .iter()
            .filter(|record| !record.checklist_complete())
            .collect();
        let evidenced = monthly
            .iter()
            .filter(|record| record.has_evidence_report || !record.verifier_verdict.is_empty())
            .count();
        let rejected = monthly
            .iter()
            .filter(|record| record.verifier_verdict == "REJECT")
            .count();

        let mut parts = vec![
            format!("{} 条", monthly.len()),
            format!(
                "清单完成 {}/{}",
                with_checklist.len() - incomplete.len(),
                with_checklist.len()
            ),
        ];
        if evidenced > 0 {
            parts.push(format!("有证据包 {evidenced}"));
        }
        if rejected > 0 {
            parts.push(palette.red(&format!("verifier REJECT {rejected}")));
        }
        let line = format!("  {month}  {}", parts.join(" · "));
        if incomplete.is_empty() {
            let _ = writeln!(out, "{}", palette.green(&line));
        } else {
            let _ = writeln!(
                out,
                "{}",
                palette.yellow(&format!("{line}  ⚠ 有未勾完的清单"))
            );
            for record in incomplete {
                let _ = writeln!(
                    out,
                    "{}",
                    palette.yellow(&format!(
                        "      - {}  {}/{}",
                        record.name.slug, record.checked, record.total
                    ))
                );
            }
        }
    }
}

fn render_section(
    title: &str,
    relative_dir: &str,
    records: &[PrdRecord],
    ctx: &RenderContext<'_>,
    collapse_months: bool,
    out: &mut String,
) {
    let _ = writeln!(
        out,
        "{}{}\n",
        ctx.palette.bold(&format!("{title} ({})", records.len())),
        ctx.palette.dim(&format!("  — {relative_dir}"))
    );
    if records.is_empty() {
        let _ = writeln!(out, "  {}", ctx.palette.dim("（空）"));
    } else if collapse_months {
        render_archive_months(records, ctx.palette, out);
    } else {
        render_table(records, ctx, out);
    }
    out.push('\n');
}

/// 生成整张看板的文本。返回 `(文本, 警告)`。
pub fn render_board(
    repo_root: &Path,
    scope: Scope,
    ctx: &RenderContext<'_>,
) -> (String, Vec<String>) {
    let tasks_dir = repo_root.join("tasks");
    let evidence_root = tasks_dir.join("evidence");
    let (pending, mut warnings) = collect_bucket(&tasks_dir, PENDING_DIR, &evidence_root);
    let (archive, archive_warnings) = collect_bucket(&tasks_dir, ARCHIVE_DIR, &evidence_root);
    warnings.extend(archive_warnings);
    let (awaiting, pending, archive) = split_awaiting_human(pending, archive);

    let mut out = String::new();
    if scope != Scope::Archive {
        render_section("PENDING", "tasks/pending", &pending, ctx, false, &mut out);
    }
    if scope == Scope::All && !awaiting.is_empty() {
        render_section(
            "AWAITING HUMAN",
            "已归档，等人确认（横幅 🧍 待人工验收）",
            &awaiting,
            ctx,
            false,
            &mut out,
        );
    }
    if scope != Scope::Pending {
        render_section("ARCHIVE", "tasks/archive", &archive, ctx, true, &mut out);
    }
    (out, warnings)
}

#[cfg(test)]
mod tests {
    use time::Duration;

    use super::*;
    use crate::prd::lock::{Caller, claim};

    fn prd_text(banner: &str, checklist: &str, extra: &str) -> String {
        format!(
            "# PRD: Example\n\n> {banner}\n\n## 1. Introduction & Goals\n\nFirst line of intro.\nSecond line.\n\n## 8. Delivery Dependencies\n\n- Depends on tasks/issues:\n  - none\n- Gate type: none\n{extra}\n## 9. Acceptance Checklist\n\n{checklist}\n"
        )
    }

    fn write(root: &Path, relative: &str, content: &str) {
        let path = root.join(relative);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, content).unwrap();
    }

    fn render(root: &Path, scope: Scope, detail: bool) -> String {
        let ctx = RenderContext {
            repo_root: root,
            now: OffsetDateTime::now_utc(),
            palette: Palette { enabled: false },
            detail,
        };
        render_board(root, scope, &ctx).0
    }

    const PENDING_A: &str = "tasks/pending/P1-FEAT-20260601-000000-alpha.md";
    const PENDING_B: &str = "tasks/pending/P2-FIX-20260602-000000-beta.md";
    const ARCHIVED_DONE: &str = "tasks/archive/P2-FEAT-20260515-000000-done.md";
    const ARCHIVED_WAITING: &str = "tasks/archive/P2-FEAT-20260516-000000-waiting.md";

    #[test]
    fn board_shows_progress_banner_and_sections() {
        let root = tempfile::tempdir().unwrap();
        write(
            root.path(),
            PENDING_A,
            &prd_text("⬜ **验收状态**：未开工。", "- [x] a\n- [ ] b\n- [ ] c", ""),
        );
        write(
            root.path(),
            PENDING_B,
            &prd_text("⬜ **验收状态**：未开工。", "- [x] a", ""),
        );
        write(
            root.path(),
            ARCHIVED_DONE,
            &prd_text("✅ **验收状态**：已验收。", "- [x] a", ""),
        );
        write(
            root.path(),
            ARCHIVED_WAITING,
            &prd_text(
                "🧍 **验收状态**：待人工验收。",
                "- [x] a\n### Human-Confirmed\n- [ ] ok?",
                "",
            ),
        );

        let board = render(root.path(), Scope::All, false);

        assert!(board.contains("PENDING (2)"), "{board}");
        assert!(board.contains("AWAITING HUMAN (1)"), "{board}");
        assert!(board.contains("ARCHIVE (1)"), "{board}");
        let alpha = board.lines().find(|line| line.contains("alpha")).unwrap();
        assert!(
            alpha.contains("1/3")
                && alpha.contains("P1")
                && alpha.contains("FEAT")
                && alpha.contains("2026-06-01"),
            "{alpha}"
        );
        let beta = board.lines().find(|line| line.contains("beta")).unwrap();
        assert!(beta.contains("1/1 ✓"), "{beta}");
        let waiting = board.lines().find(|line| line.contains("waiting")).unwrap();
        assert!(
            waiting.contains("🧍 待人工验收") && waiting.contains("1/2"),
            "{waiting}"
        );
        // 待人工验收的那条不该再出现在 ARCHIVE 月视图的异常里，而已验收的 done 是月概览的一部分。
        assert!(board.contains("2026-05  1 条 · 清单完成 1/1"), "{board}");
        assert!(!board.contains("⚠ 有未勾完的清单"), "{board}");
    }

    #[test]
    fn a_stale_awaiting_banner_in_pending_does_not_hide_the_prd_from_pending() {
        let root = tempfile::tempdir().unwrap();
        // pending 里横幅写着 🧍 且执行项还没做完：必须留在 PENDING 里被看见。
        write(
            root.path(),
            PENDING_A,
            &prd_text("🧍 **验收状态**：待人工验收。", "- [ ] still owed", ""),
        );

        let board = render(root.path(), Scope::All, false);

        assert!(board.contains("PENDING (1)"), "{board}");
        assert!(!board.contains("AWAITING HUMAN"), "{board}");
    }

    #[test]
    fn same_stem_in_pending_and_archive_prefers_pending() {
        let root = tempfile::tempdir().unwrap();
        let content = prd_text(
            "🧍 **验收状态**：待人工验收。",
            "- [x] a\n### Human-Confirmed\n- [ ] ok?",
            "",
        );
        write(
            root.path(),
            PENDING_A,
            &prd_text("⬜ **验收状态**：未开工。", "- [ ] reopened", ""),
        );
        write(
            root.path(),
            "tasks/archive/P1-FEAT-20260601-000000-alpha.md",
            &content,
        );

        let board = render(root.path(), Scope::All, false);

        assert!(
            !board.contains("AWAITING HUMAN"),
            "重开的 PRD 不该再等人验收：{board}"
        );
        assert!(
            board.contains("PENDING (1)") && board.contains("ARCHIVE (1)"),
            "{board}"
        );
        assert!(
            board.contains("⚠ 有未勾完的清单"),
            "archive 那份没勾完的空框留在月视图里报异常：{board}"
        );
    }

    #[test]
    fn lock_column_shows_the_holder_and_flags_stale_locks() {
        let root = tempfile::tempdir().unwrap();
        write(
            root.path(),
            PENDING_A,
            &prd_text("⬜ **验收状态**：未开工。", "- [ ] a", ""),
        );
        write(
            root.path(),
            PENDING_B,
            &prd_text("⬜ **验收状态**：未开工。", "- [ ] a", ""),
        );
        let caller = Caller {
            main_repo_root: root.path().to_path_buf(),
            worktree_label: "wt-a".into(),
            branch: "feat/alpha".into(),
        };
        claim(
            &caller,
            "P1-FEAT-20260601-000000-alpha",
            Some("claude"),
            OffsetDateTime::now_utc(),
        )
        .unwrap();
        claim(
            &caller,
            "P2-FIX-20260602-000000-beta",
            Some("codex"),
            OffsetDateTime::now_utc() - Duration::minutes(45),
        )
        .unwrap();

        let board = render(root.path(), Scope::Pending, false);

        let alpha = board.lines().find(|line| line.contains("alpha")).unwrap();
        assert!(alpha.contains("🔒 claude@feat/alpha"), "{alpha}");
        let beta = board.lines().find(|line| line.contains("beta")).unwrap();
        assert!(beta.contains("⚠ stale codex@feat/alpha"), "{beta}");
    }

    #[test]
    fn deps_column_reports_a_blocked_upstream() {
        let root = tempfile::tempdir().unwrap();
        write(
            root.path(),
            "tasks/pending/P1-FEAT-20260601-000000-upstream.md",
            &prd_text("⬜ **验收状态**：未开工。", "- [ ] a", ""),
        );
        let downstream = prd_text("⬜ **验收状态**：未开工。", "- [ ] a", "").replace(
            "  - none\n- Gate type: none",
            "  - P1-FEAT-20260601-000000-upstream\n- Gate type: hard",
        );
        write(root.path(), PENDING_B, &downstream);

        let board = render(root.path(), Scope::Pending, false);

        let beta = board.lines().find(|line| line.contains("beta")).unwrap();
        assert!(beta.contains("⛔ blocked by upstream"), "{beta}");
    }

    #[test]
    fn evidence_column_and_verdict_use_the_last_explicit_verdict() {
        assert_eq!(
            parse_verifier_verdict("结论：REJECT\n...修复后...\nVERDICT: PASS\n"),
            ("PASS".into(), true)
        );
        assert_eq!(
            parse_verifier_verdict("PASS（附条件）\n"),
            ("PASS".into(), true)
        );
        assert_eq!(
            parse_verifier_verdict("looks like it will PASS eventually"),
            ("PASS".into(), false)
        );
        assert_eq!(parse_verifier_verdict("nothing"), (String::new(), false));

        let root = tempfile::tempdir().unwrap();
        write(
            root.path(),
            ARCHIVED_DONE,
            &prd_text("✅ **验收状态**：已验收。", "- [x] a", ""),
        );
        write(
            root.path(),
            "tasks/evidence/P2-FEAT-20260515-000000-done/verification-plan.md",
            "plan",
        );
        write(
            root.path(),
            "tasks/evidence/P2-FEAT-20260515-000000-done/P2-FEAT-20260515-000000-done.verifier-report.md",
            "REJECT\n后来\nVERDICT: REJECT\n",
        );
        let board = render(root.path(), Scope::Archive, false);
        assert!(
            board.contains("有证据包 1") && board.contains("verifier REJECT 1"),
            "{board}"
        );
    }

    #[test]
    fn detail_mode_adds_title_and_a_bounded_summary() {
        let root = tempfile::tempdir().unwrap();
        write(
            root.path(),
            PENDING_A,
            &prd_text("⬜ **验收状态**：未开工。", "- [ ] a", ""),
        );

        let board = render(root.path(), Scope::Pending, true);

        assert!(board.contains("PRD: Example"), "{board}");
        assert!(board.contains("▏ First line of intro."), "{board}");
        assert!(board.contains("▏ Second line."), "{board}");
        assert!(!render(root.path(), Scope::Pending, false).contains("First line of intro."));
    }

    #[test]
    fn long_intros_are_truncated_with_an_ellipsis() {
        let long_intro = "x".repeat(300);
        let content = format!("# T\n\n## 1. Introduction & Goals\n\n{long_intro}\n1\n2\n3\n4\n5\n");
        let (_, summary) = extract_title_and_summary(&content);
        assert_eq!(summary.len(), 4);
        assert!(summary[0].ends_with('…') && summary[0].chars().count() == 110);
        assert!(
            summary[3].ends_with('…'),
            "取满行数仍有剩余正文时给末行补省略号：{summary:?}"
        );
    }

    #[test]
    fn summary_skips_heading_lines() {
        let content = "# T\n\n## 1. Introduction & Goals\n\n### Problem Statement\n\nReal description.\n\n### Interpretation\n\nMore.\n";
        let (_, summary) = extract_title_and_summary(content);
        assert_eq!(summary, ["Real description.", "More."]);
    }

    #[test]
    fn missing_directories_render_as_empty_sections_instead_of_failing() {
        let root = tempfile::tempdir().unwrap();
        let board = render(root.path(), Scope::All, false);
        assert!(
            board.contains("PENDING (0)") && board.contains("ARCHIVE (0)"),
            "{board}"
        );
    }

    #[test]
    fn non_prd_files_in_the_task_directories_are_ignored() {
        let root = tempfile::tempdir().unwrap();
        write(root.path(), "tasks/pending/README.md", "# not a prd");
        write(root.path(), "tasks/pending/notes.txt", "x");
        assert!(render(root.path(), Scope::Pending, false).contains("PENDING (0)"));
    }
}

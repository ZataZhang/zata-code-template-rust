//! 各子命令的实现：解析环境（仓库根、worktree、当前时间），调用库函数，并把结果打印给人看。
//!
//! 这里只放「胶水」：真正的规则都在 [`crate::prd`] 与 [`crate::hooks`] 里，那里有单元测试。
//! 约定：函数返回进程退出码；`0` 成功，`1` 检查未通过或操作被拒绝。

use std::io::IsTerminal;
use std::path::{Path, PathBuf};
use std::process::Command;

use time::OffsetDateTime;

use crate::git;
use crate::hooks::{self, GuardDecision, MaxLinesOptions};
use crate::prd::check::{Gate, validate};
use crate::prd::contract::parse_checklist;
use crate::prd::filename::{ARCHIVE_DIR, is_archived_prd, looks_like_prd};
use crate::prd::lock::{
    self, Caller, ClaimOutcome, HeartbeatOutcome, Metadata, RefusalReason, ReleaseOutcome,
};
use crate::prd::review;
use crate::prd::status::{Palette, RenderContext, Scope, render_board};

fn current_dir() -> PathBuf {
    std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."))
}

/// 当前 git 仓库根；不在仓库内时回退到当前目录。
fn repo_root(cwd: &Path) -> PathBuf {
    git::run(&["rev-parse", "--show-toplevel"], Some(cwd))
        .filter(|text| !text.is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(|| cwd.to_path_buf())
}

fn caller_for(cwd: &Path, branch: Option<String>) -> Caller {
    let main_repo_root = git::main_repo_root(cwd);
    let worktree_label = git::worktree_label(cwd, &main_repo_root);
    Caller {
        main_repo_root,
        worktree_label,
        branch: branch.unwrap_or_else(|| git::current_branch(cwd)),
    }
}

fn describe_worktree(label: &str) -> &str {
    if label.is_empty() { "主仓库" } else { label }
}

fn holder_field(metadata: &Metadata, key: &str, default: &str) -> String {
    metadata
        .get(key)
        .and_then(|value| value.as_str())
        .filter(|value| !value.is_empty())
        .unwrap_or(default)
        .to_string()
}

fn print_holder_info(holder: &Metadata, prd_argument: &str) {
    println!("❌ 该 PRD 正被其他会话执行：");
    println!(
        "   工具: {}",
        holder_field(holder, "ai_tool", lock::UNKNOWN_TOOL_LABEL)
    );
    println!("   分支: {}", holder_field(holder, "branch", "unknown"));
    println!(
        "   Worktree: {}",
        describe_worktree(&holder_field(holder, "worktree", ""))
    );
    println!(
        "   开始时间: {}",
        holder_field(holder, "started_at", "unknown")
    );
    println!(
        "   最后心跳: {}",
        holder_field(holder, "heartbeat_at", "unknown")
    );
    println!("确认对方已停止后，显式释放再重试: just prd release {prd_argument}");
}

fn stem_of(prd_argument: &str) -> String {
    Path::new(prd_argument)
        .file_stem()
        .map(|stem| stem.to_string_lossy().into_owned())
        .unwrap_or_else(|| prd_argument.to_string())
}

/// `prd status`：状态看板。
pub fn prd_status(scope: Scope, detail: bool) -> i32 {
    let cwd = current_dir();
    let repo_root = git::main_repo_root(&cwd);
    let palette = Palette {
        enabled: std::io::stdout().is_terminal() && std::env::var_os("NO_COLOR").is_none(),
    };
    let ctx = RenderContext {
        repo_root: &repo_root,
        now: OffsetDateTime::now_utc(),
        palette,
        detail,
    };
    let (board, warnings) = render_board(&repo_root, scope, &ctx);
    print!("{board}");
    for warning in warnings {
        eprintln!("⚠️ {warning}");
    }
    0
}

/// `prd start`：领取执行锁。
pub fn prd_start(prd_argument: &str, tool: Option<String>, branch: Option<String>) -> i32 {
    let cwd = current_dir();
    let caller = caller_for(&cwd, branch);
    let Some(prd_path) = review::resolve_prd_path(prd_argument, &caller.main_repo_root)
        .or_else(|| Some(PathBuf::from(prd_argument)).filter(|path| path.is_file()))
    else {
        println!("❌ PRD 文件不存在: {prd_argument}");
        return 1;
    };
    let stem = stem_of(&prd_path.to_string_lossy());

    let outcome = match lock::claim(&caller, &stem, tool.as_deref(), OffsetDateTime::now_utc()) {
        Ok(outcome) => outcome,
        Err(error) => {
            println!("❌ 领锁失败: {error:#}");
            return 1;
        }
    };
    match outcome {
        ClaimOutcome::Claimed {
            lock_path,
            took_over,
        } => {
            if let Some(archive) = took_over {
                let name = archive
                    .file_name()
                    .map(|name| name.to_string_lossy().into_owned())
                    .unwrap_or_default();
                println!("⚠️ 旧锁已过期，已留档为 {name} 并接管。");
            }
            println!("✅ 已领取 PRD 执行锁: {stem}");
            println!("   锁文件: {}", lock_path.display());
            if let Ok(content) = std::fs::read_to_string(&prd_path) {
                let (checked, total) = parse_checklist(&content).progress();
                if total > 0 {
                    println!("   清单进度: {checked}/{total}");
                }
            }
            0
        }
        ClaimOutcome::Refreshed => {
            println!("ℹ️ 锁已由当前会话持有，已刷新心跳: {stem}");
            0
        }
        ClaimOutcome::HandedOff { worktree } => {
            println!("🤝 锁由主仓库开工入口领取，已移交到当前 worktree: {worktree}");
            0
        }
        ClaimOutcome::Refused { holder, reason } => {
            if reason == RefusalReason::HolderStillActive {
                println!("⚠️ 锁心跳已过期，但归属 worktree 近期仍有文件改动，视为仍在执行。");
            }
            print_holder_info(&holder, prd_argument);
            1
        }
    }
}

/// `prd heartbeat`：续期自己持有的锁。
pub fn prd_heartbeat(prd_argument: &str) -> i32 {
    let caller = caller_for(&current_dir(), None);
    let stem = stem_of(prd_argument);
    match lock::heartbeat(&caller, &stem, OffsetDateTime::now_utc()) {
        Ok(HeartbeatOutcome::Renewed) => {
            println!("💓 心跳已续期: {stem}");
            0
        }
        Ok(HeartbeatOutcome::NoLock) => {
            println!("⚠️ 执行锁不存在（可能已被释放或接管）: {prd_argument}");
            1
        }
        Ok(HeartbeatOutcome::NotOwner(holder)) => {
            println!(
                "⚠️ 执行锁归属不符，当前持有方: {}",
                describe_worktree(&holder)
            );
            1
        }
        Err(error) => {
            println!("❌ 续期失败: {error:#}");
            1
        }
    }
}

/// `prd release`：释放执行锁。
pub fn prd_release(prd_argument: &str, force: bool) -> i32 {
    let caller = caller_for(&current_dir(), None);
    let stem = stem_of(prd_argument);
    match lock::release(&caller, &stem, force) {
        Ok(ReleaseOutcome::Released) => {
            println!("✅ 已释放执行锁: {stem}");
            0
        }
        Ok(ReleaseOutcome::NoLock) => {
            println!("ℹ️ 无执行锁可释放: {stem}");
            0
        }
        Ok(ReleaseOutcome::ForceReleased(previous)) => {
            println!("⚠️ 已强制释放 {} 持有的锁。", describe_worktree(&previous));
            0
        }
        Ok(ReleaseOutcome::NotOwner(holder)) => {
            println!("❌ 锁由 {} 持有，归属不符。", describe_worktree(&holder));
            println!("   确认对方已停止后加 --force 释放: just prd release {prd_argument} --force");
            1
        }
        Err(error) => {
            println!("❌ 释放失败: {error:#}");
            1
        }
    }
}

/// `prd check` 的选项。
#[derive(Debug, Default)]
pub struct CheckOptions {
    /// 要检查的 PRD；为空时由 `staged` / `archived` 决定检查哪些。
    pub paths: Vec<PathBuf>,
    /// 只检查结构（pending PRD 用）。
    pub structure_only: bool,
    /// 按「可归档」的标准检查。
    pub archive_ready: bool,
    /// 检查暂存区里新归档的 PRD。
    pub staged: bool,
    /// 检查 `tasks/archive` 下的全部 PRD。
    pub archived: bool,
}

/// 一个待检查的文件及其严格度。
#[derive(Debug)]
struct CheckTarget {
    path: PathBuf,
    label: String,
    gate: Gate,
}

/// 运行门禁并打印结果。返回退出码。
fn run_checks(targets: &[CheckTarget]) -> i32 {
    if targets.is_empty() {
        return 0;
    }
    println!("Checking PRD structure, acceptance checklists and validation oracles...\n");
    let mut has_errors = false;
    for target in targets {
        let content = match std::fs::read_to_string(&target.path) {
            Ok(content) => content,
            Err(error) => {
                println!("FAIL {}\n   - 无法读取：{error}\n", target.label);
                has_errors = true;
                continue;
            }
        };
        let issues = validate(&content, target.gate);
        if issues.is_empty() {
            println!("PASS {}", target.label);
            continue;
        }
        has_errors = true;
        println!("FAIL {}", target.label);
        for issue in issues {
            match issue.line {
                Some(line) => println!("   - L{line}: {}", issue.message),
                None => println!("   - {}", issue.message),
            }
        }
        println!();
    }
    if has_errors {
        println!("One or more PRDs have structural or delivery-readiness issues.");
        1
    } else {
        println!("\nAll checked PRDs are complete.");
        0
    }
}

fn relative_label(path: &Path, repo_root: &Path) -> String {
    let absolute = path.canonicalize().unwrap_or_else(|_| path.to_path_buf());
    let root = repo_root
        .canonicalize()
        .unwrap_or_else(|_| repo_root.to_path_buf());
    absolute
        .strip_prefix(&root)
        .unwrap_or(&absolute)
        .to_string_lossy()
        .replace('\\', "/")
}

fn staged_new_archived_prds(repo_root: &Path) -> Vec<String> {
    git::staged_files(repo_root, "ACR", &["tasks/archive"])
        .into_iter()
        .filter(|path| is_archived_prd(path))
        .collect()
}

/// `prd check`：PRD 门禁。
pub fn prd_check(options: &CheckOptions) -> i32 {
    let cwd = current_dir();
    let repo_root = repo_root(&cwd);
    let gate_for = |label: &str, default: Gate| {
        if options.structure_only {
            Gate::Structure
        } else if options.archive_ready || is_archived_prd(label) {
            Gate::ArchiveReady
        } else {
            default
        }
    };

    let mut targets = Vec::new();
    if !options.paths.is_empty() {
        let mut missing = Vec::new();
        for path in &options.paths {
            let absolute = if path.is_absolute() {
                path.clone()
            } else {
                cwd.join(path)
            };
            let absolute = if absolute.exists() {
                absolute
            } else {
                repo_root.join(path)
            };
            if !absolute.exists() {
                missing.push(path.clone());
                continue;
            }
            let label = relative_label(&absolute, &repo_root);
            let gate = gate_for(&label, Gate::Delivery);
            targets.push(CheckTarget {
                path: absolute,
                label,
                gate,
            });
        }
        if !missing.is_empty() {
            println!("Missing PRD path(s):");
            for path in missing {
                println!("   - {}", path.display());
            }
            return 1;
        }
    } else if options.staged {
        for label in staged_new_archived_prds(&repo_root) {
            targets.push(CheckTarget {
                path: repo_root.join(&label),
                gate: gate_for(&label, Gate::ArchiveReady),
                label,
            });
        }
    } else if options.archived {
        let mut names: Vec<_> = std::fs::read_dir(repo_root.join("tasks").join(ARCHIVE_DIR))
            .into_iter()
            .flatten()
            .flatten()
            .map(|entry| entry.file_name().to_string_lossy().into_owned())
            .filter(|name| looks_like_prd(name))
            .collect();
        names.sort();
        for name in names {
            let label = format!("tasks/{ARCHIVE_DIR}/{name}");
            targets.push(CheckTarget {
                path: repo_root.join(&label),
                gate: gate_for(&label, Gate::ArchiveReady),
                label,
            });
        }
    } else {
        println!(
            "用法：prd check <PRD 文件>... | --staged | --archived（可加 --structure-only 或 --archive-ready）"
        );
        return 2;
    }
    run_checks(&targets)
}

/// `prd review`：定位并打开人工审查清单。
pub fn prd_review(prd_argument: &str, print_only: bool) -> i32 {
    let cwd = current_dir();
    let repo_root = git::main_repo_root(&cwd);
    let Some(prd_path) = review::resolve_prd_path(prd_argument, &repo_root) else {
        println!("❌ 找不到 PRD: {prd_argument}（可以传路径、文件名或 slug）");
        return 1;
    };
    let stem = stem_of(&prd_path.to_string_lossy());
    let evidence_dir = repo_root.join("tasks").join("evidence").join(&stem);

    let Some(target) = review::find_review_target(&evidence_dir) else {
        println!("还没有可审查的材料: {stem}");
        let existing = review::list_evidence_files(&evidence_dir);
        if existing.is_empty() {
            println!("   证据目录不存在或为空: {}", evidence_dir.display());
        } else {
            println!("   证据目录现有文件（{}）：", evidence_dir.display());
            for name in existing {
                println!("   - {name}");
            }
        }
        println!(
            "   下一步：按已安装的 prd skill 生成 human-review-checklist.md（或证据报告）后再来。"
        );
        return 1;
    };
    println!("{}", target.display());
    if print_only {
        return 0;
    }
    let opener = if cfg!(target_os = "macos") {
        ("open", vec![])
    } else if cfg!(target_os = "windows") {
        ("cmd", vec!["/c", "start", ""])
    } else {
        ("xdg-open", vec![])
    };
    match Command::new(opener.0).args(&opener.1).arg(&target).status() {
        Ok(status) if status.success() => 0,
        _ => {
            println!("⚠️ 无法自动打开，请手动打开上面的路径。");
            1
        }
    }
}

/// 钩子：对暂存区里新归档的 PRD 运行归档门禁（pre-commit 把匹配的文件名传进来）。
pub fn hook_check_prd(files: &[String]) -> i32 {
    let cwd = current_dir();
    let repo_root = repo_root(&cwd);
    let newly_archived: std::collections::HashSet<String> =
        staged_new_archived_prds(&repo_root).into_iter().collect();

    let mut targets = Vec::new();
    for file in files {
        let label = file.replace('\\', "/");
        let gate = if is_archived_prd(&label) {
            // 只有「新归档」的 PRD 才过归档门禁；归档之后对已归档 PRD 的修改（例如人工验收的记录）不重新裁决。
            if !newly_archived.contains(&label) {
                continue;
            }
            Gate::ArchiveReady
        } else if label
            .strip_prefix("tasks/")
            .is_some_and(|name| !name.contains('/') && looks_like_prd(name))
        {
            Gate::Delivery
        } else {
            continue;
        };
        targets.push(CheckTarget {
            path: repo_root.join(&label),
            label,
            gate,
        });
    }
    run_checks(&targets)
}

/// 钩子：提交触及他人新鲜锁时警告，永不阻断。
pub fn hook_lock_conflict(files: &[String]) -> i32 {
    let cwd = current_dir();
    let main_repo_root = git::main_repo_root(&cwd);
    let label = git::worktree_label(&cwd, &main_repo_root);
    for warning in
        hooks::lock_conflict_warnings(files, &main_repo_root, &label, OffsetDateTime::now_utc())
    {
        println!("{warning}");
    }
    0
}

/// 钩子：文件非空行数上限。
pub fn hook_max_lines(paths: &[PathBuf], options: &MaxLinesOptions) -> i32 {
    let (code, output) = hooks::check_max_lines(paths, options, &current_dir());
    for line in output {
        println!("{line}");
    }
    code
}

/// 钩子：修改守卫测试需要显式确认。
pub fn hook_guard_tests() -> i32 {
    let repo_root = repo_root(&current_dir());
    let acknowledged =
        std::env::var("GUARD_UPDATE_ACK").is_ok_and(|value| !value.trim().is_empty());
    match hooks::decide_guard(hooks::staged_guard_tests(&repo_root), acknowledged) {
        GuardDecision::NothingGuarded => 0,
        GuardDecision::Acknowledged(_) => {
            println!(
                "GUARD_UPDATE_ACK 已设置：确认本次修改守卫测试是有意的规则更新。\n请确保已同步更新对应的约定文档。"
            );
            0
        }
        GuardDecision::Denied(files) => {
            eprintln!("⛔ 检测到对守卫测试的修改：\n");
            for file in files {
                eprintln!("  - {file}");
            }
            eprintln!(
                "\n守卫测试守护仓库约定，失败时应修复触发它的源代码或配置，而不是修改测试本身让失败消失。\n\
                 如果你确实要更新守卫规则本身（约定变更），请先同步对应约定文档，然后设置环境变量再提交：\n  \
                 GUARD_UPDATE_ACK=1 git commit ...\n\
                 AI 代理默认不应设置该变量；如因任务需要修改守卫测试，请先与人类确认。"
            );
            1
        }
    }
}

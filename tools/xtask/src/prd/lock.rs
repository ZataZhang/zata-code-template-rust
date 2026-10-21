//! PRD 执行锁：防止同一条 PRD 被多个会话重复开工。
//!
//! 锁是主仓库 `tasks/evidence/<prd-stem>/active.lock` 下的本机 JSON 文件，被 `.gitignore` 的
//! `tasks/evidence/**` 规则覆盖，天然不进版本库。在任意 linked worktree 内运行时，仓库根通过
//! `git rev-parse --git-common-dir` 反推主仓库，保证所有 worktree 共享同一份锁视图。
//!
//! 互斥语义：
//!
//! - **同一条 PRD 的领锁、续期、释放，都在一把操作系统的文件锁（`flock`）里串行执行**
//!   （`tasks/evidence/<stem>/.lock-mutex`）。「读现有锁 → 判断 → 写新锁」因此是原子的，竞争从构造上不存在；
//!   进程崩溃时操作系统自动释放这把锁，不会留下僵尸锁。锁文件本身仍用「写临时文件再改名」落盘，
//!   所以不拿互斥锁的读者（看板、提交钩子）看到的要么是完整 JSON、要么文件不存在。
//!
//!   这里**不**用「把疑似过期的锁改名挪走、事后核对、不对再放回去」的做法：挪走到放回之间锁路径是空的，
//!   第三方可以趁机创建自己的锁并宣称成功，随后「放回」又把它覆盖掉，造成两个会话同时认为自己持有锁。
//!   这个窗口在并发测试里实测复现过（40 次里 4 次），所以改成了串行化。
//! - 他人新鲜锁拒绝并给出持锁者信息；过期锁（心跳超 30 分钟）自动接管，旧锁改名留档为
//!   `active.lock.<时间戳>.<pid>.stale`；同归属（worktree 相对路径一致）重复领锁是幂等刷新。
//! - **过期判定看心跳 + worktree 活性，不做 pid 探测。** 锁工具自身是命令结束即退出的短命进程，且
//!   agent 工具调用通常给每条命令开新会话——无论记自身 pid 还是会话首领 pid，「pid 已死」都会在领锁
//!   动作返回后立刻成立，所有锁瞬间过期、互斥失效。`pid` / `hostname` 字段仍记录在锁 JSON 里，仅供
//!   排查展示，不参与判定。反过来，心跳过期但归属 worktree 在过期窗口内仍有文件改动时，视为存活会话
//!   拒绝接管——心跳只是无活性佐证时的兜底信号，避免误抢忘记续期但确实在执行的会话。
//! - 归属判定只看锁里的 `worktree` 字段；`ai_tool` / `branch` 是纯展示元数据。主仓库领取的锁被同一
//!   仓库 linked worktree 再次领取时，视为开工入口到执行器的**移交**：刷新心跳并把归属更新为当前 worktree。

use std::collections::HashSet;
use std::fs::{self, File, OpenOptions, TryLockError};
use std::io;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, SystemTime};

use anyhow::{Context, Result};
use serde_json::{Map, Value};
use time::OffsetDateTime;
use time::format_description::well_known::Rfc3339;
use time::macros::format_description;

/// 锁文件名。
pub const LOCK_FILENAME: &str = "active.lock";
/// 心跳超过这个时长即视为过期。
pub const STALE_AFTER: Duration = Duration::from_secs(30 * 60);
/// 没有自报工具名时的展示文本。
pub const UNKNOWN_TOOL_LABEL: &str = "unknown";
/// 串行化同一条 PRD 的锁操作用的互斥文件名。
const MUTEX_FILENAME: &str = ".lock-mutex";
/// 等互斥锁的最长时间。临界区只有几次小文件读写，等不到说明有进程卡死。
const MUTEX_WAIT_LIMIT: Duration = Duration::from_secs(10);
/// worktree 活性探测跳过依赖与构建产物目录。
const ACTIVITY_PRUNE_DIR_NAMES: [&str; 12] = [
    ".git",
    "target",
    "node_modules",
    ".next",
    ".turbo",
    "dist",
    "build",
    "site",
    ".venv",
    "venv",
    "__pycache__",
    ".cache",
];
/// 扫描文件数上限：超出后按已扫到的最新 mtime 给出结论，保证看板与领锁路径耗时可控。
const ACTIVITY_SCAN_FILE_LIMIT: usize = 200_000;

/// 锁 JSON 的内容。用通用的 map 而不是固定结构体：保留未知字段，与其他工具（含 Python 版）写出的锁互相兼容。
pub type Metadata = Map<String, Value>;

/// 锁的状态。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LockState {
    /// 没有锁。
    None,
    /// 有锁且心跳新鲜。
    Fresh,
    /// 有锁但已过期（或锁文件损坏、时间不可信）。
    Stale,
}

/// 某条 PRD 执行锁的查询快照。
#[derive(Debug, Clone)]
pub struct LockSnapshot {
    /// 锁状态。
    pub state: LockState,
    /// 锁 JSON 原文；无锁或损坏时为空。
    pub metadata: Metadata,
    /// 锁文件应在的路径。
    pub lock_path: PathBuf,
}

/// 调用方的身份：在哪个仓库、哪个 worktree、哪个分支。
#[derive(Debug, Clone)]
pub struct Caller {
    /// 主仓库根目录。
    pub main_repo_root: PathBuf,
    /// 归属标签：主仓库内为空串；linked worktree 内为相对主仓库根的路径。
    pub worktree_label: String,
    /// 分支名（仅展示）。
    pub branch: String,
}

/// 领锁的结果。
#[derive(Debug, Clone, PartialEq)]
pub enum ClaimOutcome {
    /// 新领取成功。`took_over` 是被接管的旧锁的留档路径。
    Claimed {
        /// 锁文件路径。
        lock_path: PathBuf,
        /// 接管了过期锁时，旧锁的留档路径。
        took_over: Option<PathBuf>,
    },
    /// 锁本就由当前会话持有，已刷新心跳。
    Refreshed,
    /// 锁由主仓库开工入口领取，已移交到当前 worktree。
    HandedOff {
        /// 移交到的 worktree。
        worktree: String,
    },
    /// 被他人持有，拒绝。
    Refused {
        /// 持锁者信息。
        holder: Metadata,
        /// 拒绝的原因。
        reason: RefusalReason,
    },
}

/// 拒绝领锁的原因。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RefusalReason {
    /// 他人持有新鲜的锁。
    FreshLock,
    /// 心跳已过期，但归属 worktree 近期仍有文件改动，视为仍在执行。
    HolderStillActive,
}

/// 续期的结果。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HeartbeatOutcome {
    /// 已续期。
    Renewed,
    /// 锁不存在（可能已被释放或接管）。
    NoLock,
    /// 归属不符；携带当前持有方的 worktree 标签。
    NotOwner(String),
}

/// 释放的结果。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ReleaseOutcome {
    /// 已释放。
    Released,
    /// 本就没有锁。
    NoLock,
    /// 归属不符而强制释放了；携带原持有方。
    ForceReleased(String),
    /// 归属不符且未强制，拒绝；携带当前持有方。
    NotOwner(String),
}

/// 某条 PRD 锁文件的路径。
pub fn lock_path_for(main_repo_root: &Path, prd_stem: &str) -> PathBuf {
    main_repo_root
        .join("tasks")
        .join("evidence")
        .join(prd_stem)
        .join(LOCK_FILENAME)
}

/// 把时间格式化成 `2026-10-16T02:30:00+00:00`（秒精度、显式 UTC 偏移），与 Python 版写出的格式一致。
pub fn format_timestamp(moment: OffsetDateTime) -> String {
    moment
        .to_offset(time::UtcOffset::UTC)
        .format(format_description!(
            "[year]-[month]-[day]T[hour]:[minute]:[second]+00:00"
        ))
        .unwrap_or_default()
}

/// 解析锁 JSON 里的 ISO 时间戳。无法解析、非字符串或不带时区时返回 `None`——调用方按「时间不可信即视为过期」处理。
pub fn parse_timestamp(value: Option<&Value>) -> Option<OffsetDateTime> {
    OffsetDateTime::parse(value?.as_str()?, &Rfc3339).ok()
}

/// 锁是否已过期：心跳无法解析，或距今超过 [`STALE_AFTER`]。不做 pid 存活探测（见模块文档）。
pub fn is_lock_stale(metadata: &Metadata, now: OffsetDateTime) -> bool {
    match parse_timestamp(metadata.get("heartbeat_at")) {
        None => true,
        Some(heartbeat) => {
            now - heartbeat > time::Duration::try_from(STALE_AFTER).unwrap_or_default()
        }
    }
}

fn read_text(path: &Path) -> Option<String> {
    fs::read_to_string(path).ok()
}

fn parse_metadata(text: &str) -> Metadata {
    match serde_json::from_str::<Value>(text) {
        Ok(Value::Object(map)) => map,
        _ => Metadata::new(),
    }
}

fn text_field(metadata: &Metadata, key: &str) -> String {
    metadata
        .get(key)
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

/// 查询某条 PRD 的执行锁状态，供看板与提交钩子复用。
pub fn inspect(main_repo_root: &Path, prd_stem: &str, now: OffsetDateTime) -> LockSnapshot {
    let lock_path = lock_path_for(main_repo_root, prd_stem);
    if !lock_path.is_file() {
        return LockSnapshot {
            state: LockState::None,
            metadata: Metadata::new(),
            lock_path,
        };
    }
    let metadata = read_text(&lock_path)
        .map(|text| parse_metadata(&text))
        .unwrap_or_default();
    let state = if is_lock_stale(&metadata, now) {
        LockState::Stale
    } else {
        LockState::Fresh
    };
    LockSnapshot {
        state,
        metadata,
        lock_path,
    }
}

/// 探测目录在最近 `window` 内是否有文件改动（跳过依赖与构建产物目录，不跟随符号链接）。
fn directory_has_recent_activity(root: &Path, window: Duration, now: SystemTime) -> bool {
    let prune: HashSet<&str> = ACTIVITY_PRUNE_DIR_NAMES.into_iter().collect();
    let mut stack = vec![root.to_path_buf()];
    let mut scanned = 0usize;
    let mut newest = SystemTime::UNIX_EPOCH;

    while let Some(directory) = stack.pop() {
        let Ok(entries) = fs::read_dir(&directory) else {
            continue;
        };
        for entry in entries.flatten() {
            let Ok(file_type) = entry.file_type() else {
                continue;
            };
            if file_type.is_symlink() {
                continue;
            }
            if file_type.is_dir() {
                let name = entry.file_name();
                if !prune.contains(name.to_string_lossy().as_ref()) {
                    stack.push(entry.path());
                }
                continue;
            }
            scanned += 1;
            if scanned > ACTIVITY_SCAN_FILE_LIMIT {
                stack.clear();
                break;
            }
            if let Ok(modified) = entry.metadata().and_then(|meta| meta.modified()) {
                newest = newest.max(modified);
            }
        }
    }
    now.duration_since(newest).is_ok_and(|age| age <= window)
}

/// 锁归属的 worktree 在过期窗口内是否仍有文件改动（心跳之外的存活佐证）。主仓库归属没有独立的工作目录，直接判否。
fn holder_has_recent_activity(main_repo_root: &Path, metadata: &Metadata, now: SystemTime) -> bool {
    let label = text_field(metadata, "worktree");
    if label.is_empty() {
        return false;
    }
    let worktree = main_repo_root.join(label);
    worktree.is_dir() && directory_has_recent_activity(&worktree, STALE_AFTER, now)
}

static STAGING_COUNTER: AtomicU64 = AtomicU64::new(0);

/// 把锁 JSON 原子地写入磁盘：先写同目录临时文件，再改名到位，读者看到的要么是旧内容、要么是新内容。
fn write_lock_file(lock_path: &Path, metadata: &Metadata) -> io::Result<()> {
    if let Some(parent) = lock_path.parent() {
        fs::create_dir_all(parent)?;
    }
    let staging = lock_path.with_file_name(format!(
        "{LOCK_FILENAME}.tmp.{}.{}",
        std::process::id(),
        STAGING_COUNTER.fetch_add(1, Ordering::Relaxed)
    ));
    let mut text = serde_json::to_string_pretty(metadata).map_err(io::Error::other)?;
    text.push('\n');
    fs::write(&staging, text)?;
    let result = fs::rename(&staging, lock_path);
    let _ = fs::remove_file(&staging);
    result
}

/// 持有期间独占同一条 PRD 的锁操作；离开作用域（含进程崩溃）时由操作系统释放。
struct MutexGuard(File);

impl MutexGuard {
    fn acquire(lock_dir: &Path) -> Result<Self> {
        fs::create_dir_all(lock_dir)
            .with_context(|| format!("无法创建目录 {}", lock_dir.display()))?;
        let mutex_path = lock_dir.join(MUTEX_FILENAME);
        let file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&mutex_path)
            .with_context(|| format!("无法打开互斥文件 {}", mutex_path.display()))?;
        let deadline = std::time::Instant::now() + MUTEX_WAIT_LIMIT;
        loop {
            match file.try_lock() {
                Ok(()) => return Ok(Self(file)),
                Err(TryLockError::WouldBlock) => {
                    if std::time::Instant::now() >= deadline {
                        anyhow::bail!(
                            "等待 PRD 锁的互斥文件超过 {} 秒（{}）：是否有进程卡死？",
                            MUTEX_WAIT_LIMIT.as_secs(),
                            mutex_path.display()
                        );
                    }
                    std::thread::sleep(Duration::from_millis(2));
                }
                Err(TryLockError::Error(error)) => {
                    return Err(error)
                        .with_context(|| format!("无法锁定 {}", mutex_path.display()));
                }
            }
        }
    }
}

impl Drop for MutexGuard {
    fn drop(&mut self) {
        let _ = self.0.unlock();
    }
}

fn lock_dir_for(main_repo_root: &Path, prd_stem: &str) -> PathBuf {
    main_repo_root.join("tasks").join("evidence").join(prd_stem)
}

fn new_metadata(caller: &Caller, tool: &str, now: OffsetDateTime) -> Metadata {
    let timestamp = format_timestamp(now);
    let mut metadata = Metadata::new();
    // pid 与 hostname 只供排查展示，不参与过期判定。
    metadata.insert("pid".into(), Value::from(std::process::id()));
    metadata.insert(
        "hostname".into(),
        Value::from(gethostname::gethostname().to_string_lossy().into_owned()),
    );
    metadata.insert(
        "worktree".into(),
        Value::from(caller.worktree_label.clone()),
    );
    metadata.insert("started_at".into(), Value::from(timestamp.clone()));
    metadata.insert("heartbeat_at".into(), Value::from(timestamp));
    metadata.insert("ai_tool".into(), Value::from(tool));
    metadata.insert("branch".into(), Value::from(caller.branch.clone()));
    metadata
}

/// 领取某条 PRD 的执行锁。
///
/// 整个「读现有锁 → 判断 → 写新锁」在互斥锁里串行执行（见模块文档），所以任何并发组合下恰一方成功，
/// 其余方看到的是已经写好的新锁，按「他人新鲜锁」被拒绝。
///
/// # Errors
///
/// 文件系统错误（无法创建目录、写入失败、等不到互斥锁等）。
pub fn claim(
    caller: &Caller,
    prd_stem: &str,
    tool: Option<&str>,
    now: OffsetDateTime,
) -> Result<ClaimOutcome> {
    let lock_path = lock_path_for(&caller.main_repo_root, prd_stem);
    let _guard = MutexGuard::acquire(&lock_dir_for(&caller.main_repo_root, prd_stem))?;
    let new_lock = new_metadata(caller, tool.unwrap_or(UNKNOWN_TOOL_LABEL), now);

    let Some(existing_text) = read_text(&lock_path) else {
        write_lock_file(&lock_path, &new_lock)
            .with_context(|| format!("无法写入锁文件 {}", lock_path.display()))?;
        return Ok(ClaimOutcome::Claimed {
            lock_path,
            took_over: None,
        });
    };
    let mut existing = parse_metadata(&existing_text);
    let existing_label = text_field(&existing, "worktree");

    // 同归属：幂等刷新心跳。
    if !existing.is_empty() && existing_label == caller.worktree_label {
        existing.insert("heartbeat_at".into(), Value::from(format_timestamp(now)));
        write_lock_file(&lock_path, &existing)?;
        return Ok(ClaimOutcome::Refreshed);
    }

    // 主仓库领取的锁被 linked worktree 再次领取：开工入口到执行器的移交。
    if !existing.is_empty() && existing_label.is_empty() && !caller.worktree_label.is_empty() {
        existing.insert(
            "worktree".into(),
            Value::from(caller.worktree_label.clone()),
        );
        existing.insert("branch".into(), Value::from(caller.branch.clone()));
        existing.insert("heartbeat_at".into(), Value::from(format_timestamp(now)));
        if let Some(tool) = tool {
            existing.insert("ai_tool".into(), Value::from(tool));
        }
        write_lock_file(&lock_path, &existing)?;
        return Ok(ClaimOutcome::HandedOff {
            worktree: caller.worktree_label.clone(),
        });
    }

    if !is_lock_stale(&existing, now) {
        return Ok(ClaimOutcome::Refused {
            holder: existing,
            reason: RefusalReason::FreshLock,
        });
    }
    if holder_has_recent_activity(&caller.main_repo_root, &existing, SystemTime::now()) {
        // 心跳过期但归属 worktree 仍在写入：视为存活会话，拒绝接管。
        return Ok(ClaimOutcome::Refused {
            holder: existing,
            reason: RefusalReason::HolderStillActive,
        });
    }

    // 接管过期锁：旧锁改名留档，再写新锁。互斥锁保证这两步之间没有别人能插进来。
    let archive_path = lock_path.with_file_name(format!(
        "{LOCK_FILENAME}.{}.{}.stale",
        now.format(format_description!(
            "[year][month][day]-[hour][minute][second]"
        ))
        .unwrap_or_default(),
        std::process::id()
    ));
    fs::rename(&lock_path, &archive_path)
        .with_context(|| format!("无法把过期锁留档到 {}", archive_path.display()))?;
    write_lock_file(&lock_path, &new_lock)
        .with_context(|| format!("无法写入锁文件 {}", lock_path.display()))?;
    Ok(ClaimOutcome::Claimed {
        lock_path,
        took_over: Some(archive_path),
    })
}

/// 续期当前会话持有的锁。
///
/// # Errors
///
/// 文件系统错误。
pub fn heartbeat(caller: &Caller, prd_stem: &str, now: OffsetDateTime) -> Result<HeartbeatOutcome> {
    let lock_path = lock_path_for(&caller.main_repo_root, prd_stem);
    if !lock_path.is_file() {
        return Ok(HeartbeatOutcome::NoLock);
    }
    let _guard = MutexGuard::acquire(&lock_dir_for(&caller.main_repo_root, prd_stem))?;
    // 拿到互斥锁之后重新读：等待期间锁可能已被释放或接管。
    let Some(text) = read_text(&lock_path) else {
        return Ok(HeartbeatOutcome::NoLock);
    };
    let mut existing = parse_metadata(&text);
    let holder = text_field(&existing, "worktree");
    if holder != caller.worktree_label {
        return Ok(HeartbeatOutcome::NotOwner(holder));
    }
    existing.insert("heartbeat_at".into(), Value::from(format_timestamp(now)));
    write_lock_file(&lock_path, &existing)?;
    Ok(HeartbeatOutcome::Renewed)
}

/// 释放某条 PRD 的执行锁。归属不符时需要 `force`。
///
/// # Errors
///
/// 文件系统错误。
pub fn release(caller: &Caller, prd_stem: &str, force: bool) -> Result<ReleaseOutcome> {
    let lock_path = lock_path_for(&caller.main_repo_root, prd_stem);
    if !lock_path.is_file() {
        return Ok(ReleaseOutcome::NoLock);
    }
    let _guard = MutexGuard::acquire(&lock_dir_for(&caller.main_repo_root, prd_stem))?;
    let Some(text) = read_text(&lock_path) else {
        return Ok(ReleaseOutcome::NoLock);
    };
    let holder = text_field(&parse_metadata(&text), "worktree");
    if holder != caller.worktree_label && !force {
        return Ok(ReleaseOutcome::NotOwner(holder));
    }
    fs::remove_file(&lock_path)
        .with_context(|| format!("无法删除锁文件 {}", lock_path.display()))?;
    Ok(if holder == caller.worktree_label {
        ReleaseOutcome::Released
    } else {
        ReleaseOutcome::ForceReleased(holder)
    })
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use std::sync::Barrier;

    use time::Duration as TimeDuration;

    use super::*;

    const STEM: &str = "P2-FEAT-20260915-112126-example";

    fn caller(root: &Path, label: &str) -> Caller {
        Caller {
            main_repo_root: root.to_path_buf(),
            worktree_label: label.to_string(),
            branch: format!("branch-of-{label}"),
        }
    }

    fn now() -> OffsetDateTime {
        OffsetDateTime::now_utc()
    }

    fn read_lock(root: &Path) -> Metadata {
        parse_metadata(&fs::read_to_string(lock_path_for(root, STEM)).unwrap())
    }

    /// 把锁里的心跳改成 `minutes_ago` 分钟前。
    fn age_lock(root: &Path, minutes_ago: i64) {
        let mut metadata = read_lock(root);
        metadata.insert(
            "heartbeat_at".into(),
            Value::from(format_timestamp(now() - TimeDuration::minutes(minutes_ago))),
        );
        fs::write(
            lock_path_for(root, STEM),
            serde_json::to_string(&metadata).unwrap(),
        )
        .unwrap();
    }

    #[test]
    fn claiming_a_free_prd_creates_a_complete_lock_file() {
        let root = tempfile::tempdir().unwrap();
        let outcome = claim(&caller(root.path(), "wt-a"), STEM, Some("claude"), now()).unwrap();

        assert!(matches!(
            outcome,
            ClaimOutcome::Claimed {
                took_over: None,
                ..
            }
        ));
        let metadata = read_lock(root.path());
        assert_eq!(text_field(&metadata, "worktree"), "wt-a");
        assert_eq!(text_field(&metadata, "ai_tool"), "claude");
        assert_eq!(text_field(&metadata, "branch"), "branch-of-wt-a");
        for key in ["pid", "hostname", "started_at", "heartbeat_at"] {
            assert!(metadata.contains_key(key), "锁 JSON 缺字段 {key}");
        }
        assert_eq!(inspect(root.path(), STEM, now()).state, LockState::Fresh);
        // 临时文件不能遗留。
        let leftovers: Vec<_> = fs::read_dir(lock_path_for(root.path(), STEM).parent().unwrap())
            .unwrap()
            .flatten()
            .filter(|entry| entry.file_name().to_string_lossy().contains(".tmp."))
            .collect();
        assert!(leftovers.is_empty(), "遗留了临时文件：{leftovers:?}");
    }

    #[test]
    fn another_session_is_refused_while_the_lock_is_fresh_and_sees_who_holds_it() {
        let root = tempfile::tempdir().unwrap();
        claim(&caller(root.path(), "wt-a"), STEM, Some("claude"), now()).unwrap();

        let outcome = claim(&caller(root.path(), "wt-b"), STEM, Some("codex"), now()).unwrap();

        let ClaimOutcome::Refused { holder, reason } = outcome else {
            panic!("应当被拒绝：{outcome:?}");
        };
        assert_eq!(reason, RefusalReason::FreshLock);
        assert_eq!(text_field(&holder, "ai_tool"), "claude");
        assert_eq!(
            text_field(&read_lock(root.path()), "worktree"),
            "wt-a",
            "不能改动持锁者的锁"
        );
    }

    #[test]
    fn claiming_again_from_the_same_worktree_is_idempotent_and_renews_the_heartbeat() {
        let root = tempfile::tempdir().unwrap();
        claim(
            &caller(root.path(), "wt-a"),
            STEM,
            Some("claude"),
            now() - TimeDuration::minutes(10),
        )
        .unwrap();
        let before = text_field(&read_lock(root.path()), "heartbeat_at");

        let outcome = claim(&caller(root.path(), "wt-a"), STEM, None, now()).unwrap();

        assert_eq!(outcome, ClaimOutcome::Refreshed);
        assert_ne!(text_field(&read_lock(root.path()), "heartbeat_at"), before);
    }

    #[test]
    fn a_lock_taken_at_the_main_repo_is_handed_off_to_the_worktree_that_picks_it_up() {
        let root = tempfile::tempdir().unwrap();
        claim(&caller(root.path(), ""), STEM, Some("just"), now()).unwrap();

        let outcome = claim(
            &caller(root.path(), "wt-exec"),
            STEM,
            Some("executor"),
            now(),
        )
        .unwrap();

        assert_eq!(
            outcome,
            ClaimOutcome::HandedOff {
                worktree: "wt-exec".into()
            }
        );
        let metadata = read_lock(root.path());
        assert_eq!(text_field(&metadata, "worktree"), "wt-exec");
        assert_eq!(text_field(&metadata, "branch"), "branch-of-wt-exec");
        assert_eq!(text_field(&metadata, "ai_tool"), "executor");
    }

    #[test]
    fn a_stale_lock_is_archived_and_taken_over() {
        let root = tempfile::tempdir().unwrap();
        claim(&caller(root.path(), "wt-a"), STEM, Some("claude"), now()).unwrap();
        age_lock(root.path(), 45);
        assert_eq!(inspect(root.path(), STEM, now()).state, LockState::Stale);

        let outcome = claim(&caller(root.path(), "wt-b"), STEM, Some("codex"), now()).unwrap();

        let ClaimOutcome::Claimed {
            took_over: Some(archive),
            ..
        } = outcome
        else {
            panic!("应当接管过期锁：{outcome:?}");
        };
        assert!(archive.to_string_lossy().ends_with(".stale"));
        assert_eq!(
            text_field(
                &parse_metadata(&fs::read_to_string(&archive).unwrap()),
                "worktree"
            ),
            "wt-a",
            "旧锁留档"
        );
        assert_eq!(text_field(&read_lock(root.path()), "worktree"), "wt-b");
    }

    #[test]
    fn a_lock_just_inside_the_window_is_still_fresh() {
        let root = tempfile::tempdir().unwrap();
        claim(&caller(root.path(), "wt-a"), STEM, None, now()).unwrap();
        age_lock(root.path(), 29);
        assert_eq!(inspect(root.path(), STEM, now()).state, LockState::Fresh);
    }

    #[test]
    fn a_stale_heartbeat_does_not_allow_takeover_while_the_holder_worktree_is_still_being_written()
    {
        let root = tempfile::tempdir().unwrap();
        let worktree = root.path().join("wt-a");
        fs::create_dir_all(&worktree).unwrap();
        fs::write(worktree.join("work.rs"), "just edited").unwrap(); // 刚写入
        claim(&caller(root.path(), "wt-a"), STEM, Some("claude"), now()).unwrap();
        age_lock(root.path(), 45);

        let outcome = claim(&caller(root.path(), "wt-b"), STEM, None, now()).unwrap();

        assert!(
            matches!(
                outcome,
                ClaimOutcome::Refused {
                    reason: RefusalReason::HolderStillActive,
                    ..
                }
            ),
            "{outcome:?}"
        );
        assert_eq!(text_field(&read_lock(root.path()), "worktree"), "wt-a");
    }

    #[test]
    fn activity_in_ignored_directories_does_not_count_as_the_holder_being_alive() {
        let root = tempfile::tempdir().unwrap();
        let worktree = root.path().join("wt-a");
        // 唯一的新文件在 target/ 里（构建产物），源码文件都很旧。
        fs::create_dir_all(worktree.join("target")).unwrap();
        fs::write(worktree.join("target").join("build.log"), "fresh").unwrap();
        let old_source = worktree.join("main.rs");
        fs::write(&old_source, "old").unwrap();
        let long_ago = SystemTime::now() - Duration::from_secs(3 * 3600);
        fs::File::options()
            .write(true)
            .open(&old_source)
            .unwrap()
            .set_modified(long_ago)
            .unwrap();
        claim(&caller(root.path(), "wt-a"), STEM, None, now()).unwrap();
        age_lock(root.path(), 45);

        let outcome = claim(&caller(root.path(), "wt-b"), STEM, None, now()).unwrap();

        assert!(
            matches!(
                outcome,
                ClaimOutcome::Claimed {
                    took_over: Some(_),
                    ..
                }
            ),
            "{outcome:?}"
        );
    }

    #[test]
    fn a_corrupt_lock_file_counts_as_stale_instead_of_blocking_forever() {
        let root = tempfile::tempdir().unwrap();
        let path = lock_path_for(root.path(), STEM);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, "{ not json").unwrap();
        assert_eq!(inspect(root.path(), STEM, now()).state, LockState::Stale);

        let outcome = claim(&caller(root.path(), "wt-a"), STEM, None, now()).unwrap();
        assert!(
            matches!(
                outcome,
                ClaimOutcome::Claimed {
                    took_over: Some(_),
                    ..
                }
            ),
            "{outcome:?}"
        );
    }

    #[test]
    fn timestamps_without_a_timezone_or_unparseable_are_untrusted_and_therefore_stale() {
        let mut metadata = Metadata::new();
        metadata.insert("heartbeat_at".into(), Value::from("2026-10-16T02:30:00"));
        assert!(is_lock_stale(&metadata, now()), "naive 时间戳不可信");
        metadata.insert("heartbeat_at".into(), Value::from("yesterday"));
        assert!(is_lock_stale(&metadata, now()));
        assert!(is_lock_stale(&Metadata::new(), now()), "缺心跳字段");

        // Python 版写出的格式（秒精度、+00:00）与我们写出的格式互相可读。
        metadata.insert(
            "heartbeat_at".into(),
            Value::from("2026-10-16T02:30:00+00:00"),
        );
        assert!(parse_timestamp(metadata.get("heartbeat_at")).is_some());
        let formatted =
            format_timestamp(OffsetDateTime::from_unix_timestamp(1_792_117_800).unwrap());
        assert_eq!(formatted, "2026-10-16T02:30:00+00:00");
    }

    #[test]
    fn heartbeat_renews_only_for_the_owner() {
        let root = tempfile::tempdir().unwrap();
        assert_eq!(
            heartbeat(&caller(root.path(), "wt-a"), STEM, now()).unwrap(),
            HeartbeatOutcome::NoLock
        );

        claim(
            &caller(root.path(), "wt-a"),
            STEM,
            None,
            now() - TimeDuration::minutes(20),
        )
        .unwrap();
        assert_eq!(
            heartbeat(&caller(root.path(), "wt-b"), STEM, now()).unwrap(),
            HeartbeatOutcome::NotOwner("wt-a".into())
        );
        assert_eq!(
            heartbeat(&caller(root.path(), "wt-a"), STEM, now()).unwrap(),
            HeartbeatOutcome::Renewed
        );
        assert_eq!(inspect(root.path(), STEM, now()).state, LockState::Fresh);
    }

    #[test]
    fn release_requires_ownership_unless_forced() {
        let root = tempfile::tempdir().unwrap();
        assert_eq!(
            release(&caller(root.path(), "wt-a"), STEM, false).unwrap(),
            ReleaseOutcome::NoLock
        );

        claim(&caller(root.path(), "wt-a"), STEM, None, now()).unwrap();
        assert_eq!(
            release(&caller(root.path(), "wt-b"), STEM, false).unwrap(),
            ReleaseOutcome::NotOwner("wt-a".into())
        );
        assert!(
            lock_path_for(root.path(), STEM).is_file(),
            "没有权限就不能动别人的锁"
        );
        assert_eq!(
            release(&caller(root.path(), "wt-b"), STEM, true).unwrap(),
            ReleaseOutcome::ForceReleased("wt-a".into())
        );
        assert!(!lock_path_for(root.path(), STEM).exists());

        claim(&caller(root.path(), "wt-a"), STEM, None, now()).unwrap();
        assert_eq!(
            release(&caller(root.path(), "wt-a"), STEM, false).unwrap(),
            ReleaseOutcome::Released
        );
    }

    #[test]
    fn exactly_one_of_many_simultaneous_claimants_wins() {
        let root = tempfile::tempdir().unwrap();
        let threads = 16;
        let barrier = Arc::new(Barrier::new(threads));
        let root_path = Arc::new(root.path().to_path_buf());

        let handles: Vec<_> = (0..threads)
            .map(|index| {
                let barrier = barrier.clone();
                let root_path = root_path.clone();
                std::thread::spawn(move || {
                    barrier.wait();
                    claim(
                        &caller(&root_path, &format!("wt-{index}")),
                        STEM,
                        None,
                        now(),
                    )
                    .unwrap()
                })
            })
            .collect();
        let outcomes: Vec<_> = handles
            .into_iter()
            .map(|handle| handle.join().unwrap())
            .collect();

        let winners = outcomes
            .iter()
            .filter(|o| matches!(o, ClaimOutcome::Claimed { .. }))
            .count();
        let refused = outcomes
            .iter()
            .filter(|o| matches!(o, ClaimOutcome::Refused { .. }))
            .count();
        assert_eq!(winners, 1, "必须恰好一个赢家：{outcomes:?}");
        assert_eq!(refused, threads - 1, "其余都应被拒绝：{outcomes:?}");
    }

    #[test]
    fn exactly_one_of_many_simultaneous_stale_takeovers_wins() {
        let root = tempfile::tempdir().unwrap();
        claim(&caller(root.path(), "wt-dead"), STEM, None, now()).unwrap();
        age_lock(root.path(), 45);

        let threads = 12;
        let barrier = Arc::new(Barrier::new(threads));
        let root_path = Arc::new(root.path().to_path_buf());
        let handles: Vec<_> = (0..threads)
            .map(|index| {
                let barrier = barrier.clone();
                let root_path = root_path.clone();
                std::thread::spawn(move || {
                    barrier.wait();
                    claim(
                        &caller(&root_path, &format!("wt-{index}")),
                        STEM,
                        None,
                        now(),
                    )
                    .unwrap()
                })
            })
            .collect();
        let outcomes: Vec<_> = handles
            .into_iter()
            .map(|handle| handle.join().unwrap())
            .collect();

        let winners = outcomes
            .iter()
            .filter(|o| matches!(o, ClaimOutcome::Claimed { .. }))
            .count();
        assert_eq!(winners, 1, "并发接管过期锁也必须恰好一个赢家：{outcomes:?}");
        let survivor = text_field(&read_lock(root.path()), "worktree");
        assert!(
            survivor.starts_with("wt-") && survivor != "wt-dead",
            "最终锁属于赢家：{survivor}"
        );
    }
}

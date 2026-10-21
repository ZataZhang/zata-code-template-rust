//! 端到端测试：真实的 `xtask` 二进制、真实的 git 仓库与 linked worktree。
//!
//! 单元测试证明了每条规则；这里证明**整条链路**成立——尤其是跨进程的互斥：多个进程分别在不同的 worktree 里
//! 同时领锁，必须恰好一个成功。

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::sync::{Arc, Barrier};

const XTASK: &str = env!("CARGO_BIN_EXE_xtask");
const VALID_PRD: &str = include_str!("fixtures/valid-prd.md");
const PRD_NAME: &str = "P2-FEAT-20260915-112126-example.md";
const PRD_STEM: &str = "P2-FEAT-20260915-112126-example";

fn git(cwd: &Path, args: &[&str]) {
    let output = Command::new("git")
        .args(args)
        .current_dir(cwd)
        .output()
        .expect("需要 git");
    assert!(
        output.status.success(),
        "git {args:?} 失败：{}",
        String::from_utf8_lossy(&output.stderr)
    );
}

fn xtask(cwd: &Path, args: &[&str]) -> Output {
    Command::new(XTASK)
        .args(args)
        .current_dir(cwd)
        .env_remove("GUARD_UPDATE_ACK")
        .env("NO_COLOR", "1")
        .output()
        .expect("无法运行 xtask")
}

fn stdout(output: &Output) -> String {
    String::from_utf8_lossy(&output.stdout).into_owned()
}

fn code(output: &Output) -> i32 {
    output.status.code().unwrap_or(-1)
}

/// 临时的 git 仓库：主仓库在 `<tmp>/main`，linked worktree 放在 `<tmp>/wt-N`。
struct Repo {
    _guard: tempfile::TempDir,
    main: PathBuf,
}

fn init_repo() -> Repo {
    let guard = tempfile::tempdir().unwrap();
    let main = guard.path().join("main");
    fs::create_dir_all(main.join("tasks/pending")).unwrap();
    git(guard.path(), &["init", "-q", "-b", "main", "main"]);
    git(&main, &["config", "user.email", "t@example.com"]);
    git(&main, &["config", "user.name", "tester"]);
    fs::write(main.join(".gitignore"), "tasks/evidence/**\n").unwrap();
    fs::write(main.join("tasks/pending").join(PRD_NAME), VALID_PRD).unwrap();
    git(&main, &["add", "-A"]);
    git(&main, &["commit", "-q", "-m", "chore: init"]);
    Repo {
        main: main.canonicalize().unwrap(),
        _guard: guard,
    }
}

fn add_worktree(repo: &Repo, name: &str) -> PathBuf {
    let path = repo.main.parent().unwrap().join(name);
    git(
        &repo.main,
        &["worktree", "add", "-q", path.to_str().unwrap(), "-b", name],
    );
    path.canonicalize().unwrap()
}

fn prd_arg() -> String {
    format!("tasks/pending/{PRD_NAME}")
}

#[test]
fn exactly_one_of_many_processes_in_different_worktrees_gets_the_lock() {
    let repo = init_repo();
    let worktrees: Vec<PathBuf> = (0..6)
        .map(|index| add_worktree(&repo, &format!("wt-{index}")))
        .collect();

    let barrier = Arc::new(Barrier::new(worktrees.len()));
    let handles: Vec<_> = worktrees
        .iter()
        .cloned()
        .map(|worktree| {
            let barrier = barrier.clone();
            std::thread::spawn(move || {
                barrier.wait();
                xtask(&worktree, &["prd", "start", &prd_arg(), "--tool", "test"])
            })
        })
        .collect();
    let outputs: Vec<Output> = handles
        .into_iter()
        .map(|handle| handle.join().unwrap())
        .collect();

    let winners: Vec<usize> = (0..outputs.len())
        .filter(|&index| code(&outputs[index]) == 0)
        .collect();
    assert_eq!(
        winners.len(),
        1,
        "跨进程必须恰好一个赢家：{:#?}",
        outputs.iter().map(stdout).collect::<Vec<_>>()
    );
    for (index, output) in outputs.iter().enumerate() {
        if winners.contains(&index) {
            assert!(
                stdout(output).contains("已领取 PRD 执行锁"),
                "{}",
                stdout(output)
            );
        } else {
            assert_eq!(code(output), 1);
            assert!(
                stdout(output).contains("正被其他会话执行"),
                "{}",
                stdout(output)
            );
        }
    }

    // 锁落在**主仓库**的证据目录，且归属是赢家的 worktree。
    let lock = fs::read_to_string(
        repo.main
            .join("tasks/evidence")
            .join(PRD_STEM)
            .join("active.lock"),
    )
    .unwrap();
    let winner_name = format!("wt-{}", winners[0]);
    assert!(
        lock.contains(&format!("{winner_name}\"")) && lock.contains("\"ai_tool\": \"test\""),
        "{lock}"
    );

    // 续期：只有持有者可以；释放：别人需要 --force。
    let winner = &worktrees[winners[0]];
    let other = &worktrees[(winners[0] + 1) % worktrees.len()];
    assert_eq!(code(&xtask(winner, &["prd", "heartbeat", &prd_arg()])), 0);
    let stranger = xtask(other, &["prd", "heartbeat", &prd_arg()]);
    assert_eq!(code(&stranger), 1, "{}", stdout(&stranger));
    let refused = xtask(other, &["prd", "release", &prd_arg()]);
    assert_eq!(code(&refused), 1);
    assert!(
        stdout(&refused).contains("归属不符"),
        "{}",
        stdout(&refused)
    );

    // 看板在主仓库里能看到持锁者。
    let board = xtask(&repo.main, &["prd", "status", "pending"]);
    assert!(stdout(&board).contains("🔒 test@wt-"), "{}", stdout(&board));

    let forced = xtask(other, &["prd", "release", &prd_arg(), "--force"]);
    assert_eq!(code(&forced), 0);
    assert!(
        stdout(&forced).contains("已强制释放"),
        "{}",
        stdout(&forced)
    );
    assert!(
        !repo
            .main
            .join("tasks/evidence")
            .join(PRD_STEM)
            .join("active.lock")
            .exists()
    );
}

#[test]
fn a_session_in_the_main_repo_hands_the_lock_to_the_worktree_that_continues_the_work() {
    let repo = init_repo();
    let worktree = add_worktree(&repo, "wt-exec");

    assert_eq!(
        code(&xtask(
            &repo.main,
            &["prd", "start", &prd_arg(), "--tool", "just"]
        )),
        0
    );
    let handoff = xtask(
        &worktree,
        &["prd", "start", &prd_arg(), "--tool", "executor"],
    );

    assert_eq!(code(&handoff), 0, "{}", stdout(&handoff));
    assert!(
        stdout(&handoff).contains("已移交到当前 worktree"),
        "{}",
        stdout(&handoff)
    );
    assert_eq!(
        code(&xtask(&worktree, &["prd", "heartbeat", &prd_arg()])),
        0,
        "移交之后 worktree 是持有者"
    );
}

#[test]
fn starting_an_unknown_prd_fails_clearly() {
    let repo = init_repo();
    let output = xtask(&repo.main, &["prd", "start", "tasks/pending/nope.md"]);
    assert_eq!(code(&output), 1);
    assert!(
        stdout(&output).contains("PRD 文件不存在"),
        "{}",
        stdout(&output)
    );
}

#[test]
fn a_prd_can_be_started_by_its_slug_and_progress_is_reported() {
    let repo = init_repo();
    let output = xtask(&repo.main, &["prd", "start", "example"]);
    assert_eq!(code(&output), 0, "{}", stdout(&output));
    assert!(
        stdout(&output).contains("清单进度: 1/2"),
        "{}",
        stdout(&output)
    );
}

#[test]
fn check_passes_a_valid_prd_and_names_the_problems_of_an_invalid_one() {
    let repo = init_repo();
    let valid = repo.main.join("tasks/pending").join(PRD_NAME);
    assert_eq!(
        code(&xtask(
            &repo.main,
            &["prd", "check", "--structure-only", valid.to_str().unwrap()]
        )),
        0
    );
    assert_eq!(
        code(&xtask(
            &repo.main,
            &["prd", "check", valid.to_str().unwrap()]
        )),
        0,
        "执行侧欠的项都已勾上"
    );

    let broken = repo
        .main
        .join("tasks/pending/P2-FIX-20260916-000000-broken.md");
    fs::write(
        &broken,
        VALID_PRD
            .replace("## 11. Non-Goals", "## 11. Goals")
            .replace("FR-2:", "FR-5:"),
    )
    .unwrap();
    let output = xtask(
        &repo.main,
        &["prd", "check", "--structure-only", broken.to_str().unwrap()],
    );
    assert_eq!(code(&output), 1);
    let text = stdout(&output);
    assert!(
        text.contains("FAIL tasks/pending/P2-FIX-20260916-000000-broken.md"),
        "{text}"
    );
    assert!(
        text.contains("Missing required PRD section: Non-Goals") && text.contains("found [1, 5]"),
        "{text}"
    );

    let missing = xtask(&repo.main, &["prd", "check", "tasks/pending/ghost.md"]);
    assert_eq!(code(&missing), 1);
    assert!(
        stdout(&missing).contains("Missing PRD path(s)"),
        "{}",
        stdout(&missing)
    );

    assert_eq!(
        code(&xtask(&repo.main, &["prd", "check"])),
        2,
        "没给要检查什么时是用法错误"
    );
}

#[test]
fn archive_hook_gates_only_newly_archived_prds() {
    let repo = init_repo();
    let archive_dir = repo.main.join("tasks/archive");
    fs::create_dir_all(&archive_dir).unwrap();

    // 新归档、但横幅还停在「未开工」：必须被拦。
    let not_ready = VALID_PRD.replace("🧍 **验收状态**：待人工验收。", "⬜ **验收状态**：未开工。");
    let archived_name = "P2-FEAT-20260917-000000-archived.md";
    fs::write(archive_dir.join(archived_name), &not_ready).unwrap();
    git(&repo.main, &["add", "-A"]);
    let rejected = xtask(
        &repo.main,
        &[
            "hook",
            "check-prd",
            &format!("tasks/archive/{archived_name}"),
        ],
    );
    assert_eq!(code(&rejected), 1);
    assert!(
        stdout(&rejected).contains("still says ⬜ 未开工"),
        "{}",
        stdout(&rejected)
    );

    // 修好后通过。
    fs::write(archive_dir.join(archived_name), VALID_PRD).unwrap();
    git(&repo.main, &["add", "-A"]);
    assert_eq!(
        code(&xtask(
            &repo.main,
            &[
                "hook",
                "check-prd",
                &format!("tasks/archive/{archived_name}")
            ]
        )),
        0
    );

    // 提交之后再改动已归档的 PRD（例如人工验收的记录）：不是「新归档」，不重新裁决。
    git(&repo.main, &["commit", "-q", "-m", "docs: archive prd"]);
    fs::write(archive_dir.join(archived_name), not_ready).unwrap();
    git(&repo.main, &["add", "-A"]);
    assert_eq!(
        code(&xtask(
            &repo.main,
            &[
                "hook",
                "check-prd",
                &format!("tasks/archive/{archived_name}")
            ]
        )),
        0
    );
}

#[test]
fn guard_test_hook_blocks_edits_to_marked_tests_unless_acknowledged() {
    let repo = init_repo();
    let tests_dir = repo.main.join("crates/app/tests");
    fs::create_dir_all(&tests_dir).unwrap();
    fs::write(
        tests_dir.join("architecture.rs"),
        "//! 守卫测试（guard test）：守护依赖方向。\n#[test] fn a() {}\n",
    )
    .unwrap();
    fs::write(
        tests_dir.join("ordinary.rs"),
        "//! 普通测试\n#[test] fn b() {}\n",
    )
    .unwrap();
    git(&repo.main, &["add", "-A"]);

    let denied = xtask(&repo.main, &["hook", "guard-tests"]);
    assert_eq!(code(&denied), 1);
    let message = String::from_utf8_lossy(&denied.stderr).into_owned();
    assert!(
        message.contains("crates/app/tests/architecture.rs") && !message.contains("ordinary.rs"),
        "{message}"
    );

    let acknowledged = Command::new(XTASK)
        .args(["hook", "guard-tests"])
        .current_dir(&repo.main)
        .env("GUARD_UPDATE_ACK", "1")
        .output()
        .unwrap();
    assert_eq!(code(&acknowledged), 0, "{}", stdout(&acknowledged));

    // 只改普通测试：放行。
    git(&repo.main, &["reset", "-q"]);
    git(&repo.main, &["add", "crates/app/tests/ordinary.rs"]);
    assert_eq!(code(&xtask(&repo.main, &["hook", "guard-tests"])), 0);
}

#[test]
fn max_lines_hook_fails_on_oversized_files_and_warns_when_asked() {
    let repo = init_repo();
    let big = repo.main.join("big.rs");
    fs::write(&big, "x\n".repeat(30)).unwrap();

    let failed = xtask(
        &repo.main,
        &[
            "hook",
            "max-lines",
            "--max-lines",
            "10",
            big.to_str().unwrap(),
        ],
    );
    assert_eq!(code(&failed), 1);
    assert!(stdout(&failed).contains("[ERROR]"), "{}", stdout(&failed));

    let warned = xtask(
        &repo.main,
        &[
            "hook",
            "max-lines",
            "--max-lines",
            "10",
            "--warn-only",
            big.to_str().unwrap(),
        ],
    );
    assert_eq!(code(&warned), 0);
    assert!(stdout(&warned).contains("[WARNING]"), "{}", stdout(&warned));
}

#[test]
fn lock_conflict_hook_warns_but_never_blocks() {
    let repo = init_repo();
    let worktree = add_worktree(&repo, "wt-other");
    assert_eq!(
        code(&xtask(
            &worktree,
            &["prd", "start", &prd_arg(), "--tool", "codex"]
        )),
        0
    );

    let output = xtask(
        &repo.main,
        &["hook", "lock-conflict", "--warn-only", &prd_arg()],
    );

    assert_eq!(code(&output), 0, "永不阻断");
    assert!(
        stdout(&output).contains("[WARNING]") && stdout(&output).contains("codex"),
        "{}",
        stdout(&output)
    );
}

#[test]
fn review_prints_the_best_available_target_or_explains_what_is_missing() {
    let repo = init_repo();
    let missing = xtask(&repo.main, &["prd", "review", "example", "--print"]);
    assert_eq!(code(&missing), 1);
    assert!(
        stdout(&missing).contains("还没有可审查的材料"),
        "{}",
        stdout(&missing)
    );

    let evidence = repo.main.join("tasks/evidence").join(PRD_STEM);
    fs::create_dir_all(&evidence).unwrap();
    fs::write(evidence.join("human-review-checklist.md"), "x").unwrap();
    let found = xtask(&repo.main, &["prd", "review", "example", "--print"]);
    assert_eq!(code(&found), 0);
    assert!(
        stdout(&found).trim().ends_with("human-review-checklist.md"),
        "{}",
        stdout(&found)
    );
}

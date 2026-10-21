//! 对 `git` 命令行的薄封装。
//!
//! 刻意用子进程调用 `git` 而不是链接 libgit2：这些工具只在开发机和 CI 上运行，那里一定有 git，
//! 而用子进程能保证行为和开发者手敲的命令完全一致（包括 worktree、`GIT_DIR` 等环境变量）。

use std::path::{Path, PathBuf};
use std::process::Command;

/// 运行 `git <args>`（可指定工作目录），成功时返回去掉首尾空白的标准输出；失败（含没有 git、
/// 不在仓库内）返回 `None`。
pub fn run(args: &[&str], cwd: Option<&Path>) -> Option<String> {
    let mut command = Command::new("git");
    command.args(args);
    if let Some(cwd) = cwd {
        command.current_dir(cwd);
    }
    let output = command.output().ok()?;
    output
        .status
        .success()
        .then(|| String::from_utf8_lossy(&output.stdout).trim().to_string())
}

/// 主仓库根目录（在 linked worktree 里也解析到**主**仓库，让所有 worktree 共享同一份锁视图）。
///
/// 依据 `git rev-parse --git-common-dir`；不在 git 仓库内时，向上查找含 `tasks/` 的目录；
/// 仍找不到时回退到 `cwd`。
pub fn main_repo_root(cwd: &Path) -> PathBuf {
    if let Some(common_dir) =
        run(&["rev-parse", "--git-common-dir"], Some(cwd)).filter(|text| !text.is_empty())
    {
        let common_dir = PathBuf::from(common_dir);
        let common_dir = if common_dir.is_absolute() {
            common_dir
        } else {
            cwd.join(common_dir)
        };
        let common_dir = common_dir.canonicalize().unwrap_or(common_dir);
        if common_dir.file_name().is_some_and(|name| name == ".git")
            && let Some(parent) = common_dir.parent()
        {
            return parent.to_path_buf();
        }
    }
    cwd.ancestors()
        .find(|candidate| candidate.join("tasks").is_dir())
        .unwrap_or(cwd)
        .to_path_buf()
}

/// 当前工作树相对主仓库根的归属标签：主仓库内为空串；linked worktree 内为相对主仓库根的路径。
pub fn worktree_label(cwd: &Path, main_repo_root: &Path) -> String {
    let Some(toplevel) =
        run(&["rev-parse", "--show-toplevel"], Some(cwd)).filter(|text| !text.is_empty())
    else {
        return String::new();
    };
    let current = PathBuf::from(toplevel);
    let current = current.canonicalize().unwrap_or(current);
    let main = main_repo_root
        .canonicalize()
        .unwrap_or_else(|_| main_repo_root.to_path_buf());
    if current == main {
        return String::new();
    }
    relative_path(&current, &main)
}

/// `target` 相对 `base` 的路径（可含 `..`），用正斜杠渲染。
fn relative_path(target: &Path, base: &Path) -> String {
    let target_parts: Vec<_> = target.components().collect();
    let base_parts: Vec<_> = base.components().collect();
    let common = target_parts
        .iter()
        .zip(&base_parts)
        .take_while(|(a, b)| a == b)
        .count();
    let mut parts: Vec<String> =
        std::iter::repeat_n("..".to_string(), base_parts.len() - common).collect();
    parts.extend(
        target_parts[common..]
            .iter()
            .map(|component| component.as_os_str().to_string_lossy().into_owned()),
    );
    parts.join("/")
}

/// 当前分支名；detached HEAD 或不在仓库内时为空串。
pub fn current_branch(cwd: &Path) -> String {
    run(&["branch", "--show-current"], Some(cwd)).unwrap_or_default()
}

/// 暂存区里发生变更的文件（仓库相对路径）。`diff_filter` 形如 `ACR`；`pathspec` 为空时不限路径。
pub fn staged_files(repo_root: &Path, diff_filter: &str, pathspec: &[&str]) -> Vec<String> {
    let filter = format!("--diff-filter={diff_filter}");
    let mut args = vec![
        "-c",
        "core.quotepath=false",
        "diff",
        "--cached",
        "--name-only",
        "--no-renames",
        &filter,
    ];
    if !pathspec.is_empty() {
        args.push("--");
        args.extend_from_slice(pathspec);
    }
    run(&args, Some(repo_root))
        .map(|output| {
            output
                .lines()
                .map(str::trim)
                .filter(|line| !line.is_empty())
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn relative_path_walks_up_and_down() {
        assert_eq!(relative_path(Path::new("/r/wt/a"), Path::new("/r")), "wt/a");
        assert_eq!(
            relative_path(Path::new("/other/x"), Path::new("/r/main")),
            "../../other/x"
        );
        assert_eq!(relative_path(Path::new("/r"), Path::new("/r")), "");
    }

    #[test]
    fn main_repo_root_falls_back_to_the_nearest_tasks_directory_outside_git() {
        let root = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(root.path().join("tasks")).unwrap();
        let nested = root.path().join("a/b");
        std::fs::create_dir_all(&nested).unwrap();

        let found = main_repo_root(&nested);

        // 临时目录不在任何 git 仓库里时才能断言回退行为；若碰巧在，就只断言结果是个真实目录。
        if run(&["rev-parse", "--git-common-dir"], Some(&nested)).is_none() {
            assert_eq!(
                found.canonicalize().unwrap(),
                root.path().canonicalize().unwrap()
            );
        } else {
            assert!(found.is_dir());
        }
    }
}

//! 安全复制 Rust 模板，并按显式清单更新上游拥有的文件。

use std::collections::BTreeSet;
use std::fs;
use std::io::{self, IsTerminal, Write};
use std::path::{Component, Path, PathBuf};
use std::process::Command;

use tempfile::TempDir;

/// `just sync-template` 未提供来源时使用的 Rust 模板仓库。
pub const DEFAULT_UPSTREAM: &str = "https://github.com/ZataZhang/zata-code-template-rust.git";

const SYNC_MANIFEST: &str = ".template-sync";

/// 把当前模板复制到目标目录。`--force` 只覆盖同名文件，不删除目标里的其他文件。
pub fn copy_template(destination: &Path, force: bool) -> i32 {
    match copy_template_inner(destination, force) {
        Ok(copied_files) => {
            println!("已复制 {copied_files} 个文件到 {}", destination.display());
            println!(
                "模板维护文件（just copy）已从目标 justfile 移除；未初始化 Git、未创建提交。\n"
            );
            0
        }
        Err(error) => {
            eprintln!("模板复制失败：{error:#}");
            1
        }
    }
}

fn copy_template_inner(destination: &Path, force: bool) -> anyhow::Result<usize> {
    let source_root = repository_root()?;
    let target_root = normalized_absolute(destination)?;
    if target_root == source_root
        || target_root.starts_with(&source_root)
        || source_root.starts_with(&target_root)
    {
        anyhow::bail!("目标不能是模板目录本身、模板的子目录或模板的父目录");
    }
    if target_root.exists() && !target_root.is_dir() {
        anyhow::bail!("目标已存在且不是目录：{}", target_root.display());
    }
    if target_root.is_dir() && fs::read_dir(&target_root)?.next().is_some() && !force {
        anyhow::bail!(
            "目标目录非空：{}；确认要覆盖同名文件时传 --force（不会删除其他文件）",
            target_root.display()
        );
    }

    fs::create_dir_all(&target_root)?;
    let mut copied_files = 0;
    copy_tree(&source_root, &source_root, &target_root, &mut copied_files)?;
    trim_copy_recipe(&target_root.join("justfile"))?;
    Ok(copied_files)
}

fn copy_tree(
    source_root: &Path,
    source_directory: &Path,
    target_root: &Path,
    copied_files: &mut usize,
) -> anyhow::Result<()> {
    let mut entries = fs::read_dir(source_directory)?.collect::<Result<Vec<_>, _>>()?;
    entries.sort_by_key(std::fs::DirEntry::file_name);
    for entry in entries {
        let source_path = entry.path();
        let relative_path = source_path.strip_prefix(source_root)?;
        if is_excluded_from_copy(relative_path) {
            continue;
        }
        if has_symlink_component(target_root, relative_path)? {
            anyhow::bail!(
                "复制目标经过符号链接，拒绝写入：{}",
                target_root.join(relative_path).display()
            );
        }
        let file_type = entry.file_type()?;
        if file_type.is_symlink() {
            continue;
        }
        let target_path = target_root.join(relative_path);
        if file_type.is_dir() {
            fs::create_dir_all(&target_path)?;
            copy_tree(source_root, &source_path, target_root, copied_files)?;
        } else if file_type.is_file() {
            if let Some(parent) = target_path.parent() {
                fs::create_dir_all(parent)?;
            }
            fs::copy(&source_path, &target_path)?;
            *copied_files += 1;
        }
    }
    Ok(())
}

fn is_excluded_from_copy(relative_path: &Path) -> bool {
    let components = relative_path
        .components()
        .filter_map(|component| component.as_os_str().to_str())
        .collect::<Vec<_>>();
    if components.iter().any(|component| {
        matches!(
            *component,
            ".git"
                | "target"
                | "node_modules"
                | ".next"
                | "dist"
                | "build"
                | ".venv"
                | ".pytest_cache"
                | ".ruff_cache"
                | "logs"
                | "backups"
                | ".artifacts"
                | ".claude"
                | ".codex"
                | ".agents"
                | ".vscode"
                | ".idea"
                | ".DS_Store"
                | "Thumbs.db"
        )
    }) {
        return true;
    }
    if components.first() == Some(&"tasks") {
        return true;
    }
    let file_name = relative_path.file_name().and_then(|name| name.to_str());
    file_name.is_some_and(|name| {
        (name.starts_with(".env") && !name.ends_with(".example"))
            || name.ends_with(".tgz")
            || name.ends_with(".log")
            || name.ends_with(".swp")
            || name.ends_with(".db")
            || name.ends_with(".db-journal")
    })
}

fn trim_copy_recipe(justfile_path: &Path) -> anyhow::Result<()> {
    if !justfile_path.is_file() {
        return Ok(());
    }
    const MARKER: &str = "\n# Template maintainer command: copy this repository to a new project.";
    let justfile = fs::read_to_string(justfile_path)?;
    if let Some(marker_index) = justfile.find(MARKER) {
        fs::write(
            justfile_path,
            format!("{}\n", justfile[..marker_index].trim_end()),
        )?;
    }
    Ok(())
}

/// 按目标仓库 `.template-sync` 清单展示差异；交互模式逐文件确认，`--apply` 才批量覆盖。
pub fn sync_template(upstream: &str, list_only: bool, apply_all: bool) -> i32 {
    match sync_template_inner(upstream, list_only, apply_all) {
        Ok(()) => 0,
        Err(error) => {
            eprintln!("模板同步失败：{error:#}");
            1
        }
    }
}

fn sync_template_inner(upstream: &str, list_only: bool, apply_all: bool) -> anyhow::Result<()> {
    let target_root = repository_root()?;
    let manifest_path = target_root.join(SYNC_MANIFEST);
    if has_symlink_component(&target_root, Path::new(SYNC_MANIFEST))? {
        anyhow::bail!("同步清单不能是符号链接：{}", manifest_path.display());
    }
    let sync_paths = read_sync_manifest(&manifest_path)?;
    let (upstream_root, _checkout) = checkout_upstream(upstream)?;
    let paths = collect_sync_paths(&target_root, &upstream_root, &sync_paths)?;
    let mut changes = Vec::new();

    for relative_path in paths {
        let upstream_path = upstream_root.join(&relative_path);
        let target_path = target_root.join(&relative_path);
        if has_symlink_component(&target_root, &relative_path)? {
            anyhow::bail!("同步路径经过符号链接，拒绝覆盖：{}", target_path.display());
        }
        if !upstream_path.is_file() {
            println!("保留（上游已无此文件）：{}", relative_path.display());
            continue;
        }
        let upstream_bytes = fs::read(&upstream_path)?;
        let target_bytes = fs::read(&target_path).ok();
        let status = match target_bytes.as_deref() {
            None => "新增",
            Some(bytes) if bytes == upstream_bytes => continue,
            Some(_) => "有差异",
        };
        println!("{status}：{}", relative_path.display());
        if !list_only {
            changes.push((relative_path, upstream_path, target_path));
        }
    }

    if list_only {
        return Ok(());
    }
    if changes.is_empty() {
        println!("没有需要同步的文件。");
        return Ok(());
    }
    if !apply_all && !io::stdin().is_terminal() {
        println!("当前不是交互终端；没有写入文件。查看差异：just sync-template --list");
        println!("确认覆盖清单内所有差异：just sync-template --apply");
        return Ok(());
    }

    let mut updated_files = 0;
    for (relative_path, upstream_path, target_path) in changes {
        if !apply_all {
            show_file_diff(&target_path, &upstream_path)?;
            print!("覆盖该文件？[y/N] ");
            io::stdout().flush()?;
            let mut answer = String::new();
            io::stdin().read_line(&mut answer)?;
            if !matches!(answer.trim().to_ascii_lowercase().as_str(), "y" | "yes") {
                println!("跳过：{}", relative_path.display());
                continue;
            }
        }
        if let Some(parent) = target_path.parent() {
            fs::create_dir_all(parent)?;
        }
        fs::copy(upstream_path, target_path)?;
        updated_files += 1;
    }
    println!("已更新 {updated_files} 个文件。");
    Ok(())
}

fn read_sync_manifest(manifest_path: &Path) -> anyhow::Result<Vec<PathBuf>> {
    let manifest = fs::read_to_string(manifest_path)
        .map_err(|error| anyhow::anyhow!("无法读取 {}：{error}", manifest_path.display()))?;
    let mut paths = Vec::new();
    for (line_number, line) in manifest.lines().enumerate() {
        let entry = line.trim();
        if entry.is_empty() || entry.starts_with('#') {
            continue;
        }
        let relative_path = PathBuf::from(entry);
        if relative_path.is_absolute()
            || relative_path
                .components()
                .any(|component| !matches!(component, Component::Normal(_)))
            || is_protected_sync_path(&relative_path)
        {
            anyhow::bail!(
                "{} 第 {} 行不是安全的仓库相对路径：{}",
                manifest_path.display(),
                line_number + 1,
                entry
            );
        }
        paths.push(relative_path);
    }
    if paths.is_empty() {
        anyhow::bail!("{} 没有同步路径", manifest_path.display());
    }
    Ok(paths)
}

fn is_protected_sync_path(relative_path: &Path) -> bool {
    let components = relative_path
        .components()
        .filter_map(|component| component.as_os_str().to_str())
        .collect::<Vec<_>>();
    if components.iter().any(|component| {
        matches!(
            *component,
            ".git" | "target" | "node_modules" | ".next" | "tasks" | "backups"
        )
    }) {
        return true;
    }
    relative_path
        .file_name()
        .and_then(|name| name.to_str())
        .is_some_and(|name| name.starts_with(".env") && !name.ends_with(".example"))
}

fn collect_sync_paths(
    target_root: &Path,
    upstream_root: &Path,
    sync_paths: &[PathBuf],
) -> anyhow::Result<BTreeSet<PathBuf>> {
    let mut paths = BTreeSet::new();
    for sync_path in sync_paths {
        collect_files(upstream_root, sync_path, &mut paths)?;
        collect_files(target_root, sync_path, &mut paths)?;
    }
    Ok(paths)
}

fn collect_files(
    root: &Path,
    relative_path: &Path,
    paths: &mut BTreeSet<PathBuf>,
) -> anyhow::Result<()> {
    if has_symlink_component(root, relative_path)? {
        return Ok(());
    }
    let absolute_path = root.join(relative_path);
    let Ok(file_type) = fs::symlink_metadata(&absolute_path).map(|metadata| metadata.file_type())
    else {
        return Ok(());
    };
    if file_type.is_symlink() {
        return Ok(());
    }
    if file_type.is_file() {
        paths.insert(relative_path.to_path_buf());
        return Ok(());
    }
    if file_type.is_dir() {
        let mut entries = fs::read_dir(&absolute_path)?.collect::<Result<Vec<_>, _>>()?;
        entries.sort_by_key(std::fs::DirEntry::file_name);
        for entry in entries {
            let child_relative_path = relative_path.join(entry.file_name());
            collect_files(root, &child_relative_path, paths)?;
        }
    }
    Ok(())
}

fn has_symlink_component(root: &Path, relative_path: &Path) -> anyhow::Result<bool> {
    let mut current_path = root.to_path_buf();
    for component in relative_path.components() {
        let Component::Normal(segment) = component else {
            anyhow::bail!("路径必须由普通相对路径段组成：{}", relative_path.display());
        };
        current_path.push(segment);
        match fs::symlink_metadata(&current_path) {
            Ok(metadata) if metadata.file_type().is_symlink() => return Ok(true),
            Ok(_) => {}
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(false),
            Err(error) => return Err(error.into()),
        }
    }
    Ok(false)
}

fn checkout_upstream(upstream: &str) -> anyhow::Result<(PathBuf, Option<TempDir>)> {
    let upstream_path = PathBuf::from(upstream);
    if upstream_path.is_dir() {
        return Ok((upstream_path.canonicalize()?, None));
    }
    let checkout = tempfile::tempdir()?;
    let output = Command::new("git")
        .args(["clone", "--depth", "1", "--quiet", "--", upstream])
        .arg(checkout.path())
        .output()?;
    if !output.status.success() {
        anyhow::bail!(
            "git clone 失败：{}",
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }
    Ok((checkout.path().to_path_buf(), Some(checkout)))
}

fn show_file_diff(target_path: &Path, upstream_path: &Path) -> anyhow::Result<()> {
    let empty_file = tempfile::NamedTempFile::new()?;
    let base_path = if target_path.is_file() {
        target_path
    } else {
        empty_file.path()
    };
    let output = Command::new("git")
        .args(["diff", "--no-index", "--no-color", "--"])
        .arg(base_path)
        .arg(upstream_path)
        .output()?;
    if output.status.code().is_some_and(|code| code > 1) {
        anyhow::bail!(
            "git diff 执行失败：{}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
    print!("{}", String::from_utf8_lossy(&output.stdout));
    Ok(())
}

fn repository_root() -> anyhow::Result<PathBuf> {
    let current_directory = std::env::current_dir()?;
    current_directory
        .ancestors()
        .find(|candidate| {
            candidate.join("Cargo.toml").is_file()
                && candidate.join("tools/xtask/Cargo.toml").is_file()
                && candidate.join("justfile").is_file()
        })
        .map(Path::to_path_buf)
        .ok_or_else(|| anyhow::anyhow!("无法定位 Rust 模板仓库根目录"))
}

fn normalized_absolute(path: &Path) -> anyhow::Result<PathBuf> {
    let absolute_path = if path.is_absolute() {
        path.to_path_buf()
    } else {
        std::env::current_dir()?.join(path)
    };
    let mut normalized = PathBuf::new();
    for component in absolute_path.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                normalized.pop();
            }
            other => normalized.push(other.as_os_str()),
        }
    }
    let mut existing_prefix = normalized.clone();
    let mut missing_suffix = Vec::new();
    while !existing_prefix.exists() {
        let Some(file_name) = existing_prefix.file_name() else {
            anyhow::bail!("无法解析目标路径：{}", path.display());
        };
        missing_suffix.push(file_name.to_os_string());
        existing_prefix.pop();
    }
    let mut resolved = existing_prefix.canonicalize()?;
    for component in missing_suffix.iter().rev() {
        resolved.push(component);
    }
    Ok(resolved)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn copy_excludes_local_state_and_secrets_but_keeps_example_env_files() {
        for excluded_path in [
            ".env.local",
            ".env.compose",
            ".claude/settings.local.json",
            ".codex/session.json",
            ".agents/settings.json",
            ".vscode/settings.json",
            ".idea/workspace.xml",
            "logs/app.log",
            "packages/accounts-ui/tpl-accounts-ui-0.1.0.tgz",
            ".artifacts/accounts-host/host-bundle.tgz",
            "state/test.db",
            "state/test.db-journal",
            "editor/file.swp",
            "Thumbs.db",
        ] {
            assert!(
                is_excluded_from_copy(Path::new(excluded_path)),
                "应排除 {excluded_path}"
            );
        }

        for retained_path in [".env.example", "frontends/admin/.env.example", "src/lib.rs"] {
            assert!(
                !is_excluded_from_copy(Path::new(retained_path)),
                "应保留 {retained_path}"
            );
        }
    }

    #[cfg(unix)]
    #[test]
    fn copy_refuses_to_follow_a_symlink_inside_the_destination() {
        let source_root = tempfile::tempdir().unwrap();
        let target_root = tempfile::tempdir().unwrap();
        let outside_root = tempfile::tempdir().unwrap();
        fs::create_dir(source_root.path().join("crates")).unwrap();
        fs::write(source_root.path().join("crates/app.rs"), "template").unwrap();
        std::os::unix::fs::symlink(outside_root.path(), target_root.path().join("crates")).unwrap();

        let mut copied_files = 0;
        let copy_result = copy_tree(
            source_root.path(),
            source_root.path(),
            target_root.path(),
            &mut copied_files,
        );

        assert!(copy_result.is_err());
        assert!(!outside_root.path().join("app.rs").exists());
    }
}

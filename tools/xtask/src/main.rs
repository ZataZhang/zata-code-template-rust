//! `cargo xtask`：本仓库的开发期工具入口。
//!
//! 常用：
//!
//! ```text
//! cargo xtask prd status [all|pending|archive] [--detail]   PRD 状态看板
//! cargo xtask prd start <prd> [--tool 名称]                  领取 PRD 执行锁
//! cargo xtask prd heartbeat <prd>                            续期
//! cargo xtask prd release <prd> [--force]                    释放
//! cargo xtask prd check <prd>... [--structure-only | --archive-ready]
//! cargo xtask prd review <prd> [--print]                     打开人工审查清单
//! ```
//!
//! 日常通过 `just prd ...` 调用；`hook ...` 子命令由 pre-commit 钩子调用。

use std::path::PathBuf;
use std::process::ExitCode;

use clap::{Parser, Subcommand, ValueEnum};
use xtask::commands::{self, CheckOptions};
use xtask::hooks::MaxLinesOptions;
use xtask::prd::status::Scope;
use xtask::template::{DEFAULT_UPSTREAM, copy_template, sync_template};

#[derive(Parser)]
#[command(
    name = "xtask",
    about = "本仓库的开发期工具：PRD 工具链与提交钩子的检查"
)]
enum Cli {
    /// PRD 工具链：状态看板、执行锁、门禁、审查入口
    Prd {
        #[command(subcommand)]
        command: PrdCommand,
    },
    /// 提交钩子的检查（由 pre-commit 调用）
    Hook {
        #[command(subcommand)]
        command: HookCommand,
    },
    /// 从本模板派生项目，或同步模板维护的通用文件
    Template {
        #[command(subcommand)]
        command: TemplateCommand,
    },
}

#[derive(Subcommand)]
enum TemplateCommand {
    /// 将当前模板复制到新目录；已有非空目录需传 --force（只覆盖同名文件）
    Copy {
        /// 目标目录
        destination: PathBuf,
        /// 允许把模板文件覆盖到已有目录；不会删除目标中的其他文件
        #[arg(long)]
        force: bool,
    },
    /// 按目标项目的 .template-sync 清单查看或同步模板文件
    Sync {
        /// 模板 Git 仓库 URL 或本地目录
        #[arg(long, default_value = DEFAULT_UPSTREAM)]
        upstream: String,
        /// 只列出差异，不显示 diff 或写入文件
        #[arg(long)]
        list: bool,
        /// 确认并覆盖所有有差异的上游所有文件
        #[arg(long, conflicts_with = "list")]
        apply: bool,
    },
}

#[derive(Clone, Copy, ValueEnum)]
enum ScopeArg {
    All,
    Pending,
    Archive,
}

#[derive(Subcommand)]
enum PrdCommand {
    /// 状态看板：汇总 pending / archive 的清单进度、依赖、执行锁与证据包状态
    Status {
        /// 看哪一部分
        #[arg(value_enum, default_value = "all")]
        scope: ScopeArg,
        /// 每个 PRD 下方追加标题与摘要
        #[arg(long)]
        detail: bool,
    },
    /// 领取执行锁：他人新鲜锁拒绝（退出 1），过期锁自动接管，同归属重复领取幂等
    Start {
        /// PRD 文件（路径、文件名或 slug）
        prd_file: String,
        /// 自报的 agent 工具名（自由文本，仅展示）
        #[arg(long)]
        tool: Option<String>,
        /// 分支展示字段，缺省取当前分支
        #[arg(long)]
        branch: Option<String>,
    },
    /// 续期自己持有的锁
    Heartbeat {
        /// PRD 文件
        prd_file: String,
    },
    /// 释放执行锁
    Release {
        /// PRD 文件
        prd_file: String,
        /// 归属不符时强制释放
        #[arg(long)]
        force: bool,
    },
    /// 门禁：检查 PRD 的结构、验收清单与验证 oracle
    Check {
        /// 要检查的 PRD 文件
        paths: Vec<PathBuf>,
        /// 只检查结构（新写出来的 pending PRD 用）
        #[arg(long, conflicts_with = "archive_ready")]
        structure_only: bool,
        /// 按「可归档」的标准检查（再加最终对账与横幅一致性）
        #[arg(long)]
        archive_ready: bool,
        /// 检查暂存区里新归档的 PRD
        #[arg(long, conflicts_with = "archived")]
        staged: bool,
        /// 检查 tasks/archive 下的全部 PRD
        #[arg(long)]
        archived: bool,
    },
    /// 打开 PRD 的人工审查清单（不存在时回退证据报告）
    Review {
        /// PRD 文件（路径、文件名或 slug）
        prd: String,
        /// 只打印路径，不调用系统打开器
        #[arg(long = "print")]
        print_only: bool,
    },
}

#[derive(Subcommand)]
enum HookCommand {
    /// 对暂存区里新归档的 PRD 运行归档门禁
    CheckPrd {
        /// pre-commit 传入的文件
        files: Vec<String>,
    },
    /// 提交触及他人新鲜锁持有的 PRD 时警告（永不阻断）
    LockConflict {
        /// 兼容 warn-only 钩子惯例的形式参数
        #[arg(long)]
        warn_only: bool,
        /// pre-commit 传入的文件
        files: Vec<String>,
    },
    /// 检查文件非空行数是否超过阈值
    MaxLines {
        /// 非空行数上限
        #[arg(long, default_value_t = 1000)]
        max_lines: usize,
        /// 仅警告，不返回非零退出码
        #[arg(long)]
        warn_only: bool,
        /// 目录递归时的 glob 过滤，例如 '*.rs'
        #[arg(long)]
        glob: Option<String>,
        /// 白名单文件：每行一个相对仓库根的路径，命中的超额文件只警告
        #[arg(long)]
        allow_list_file: Option<PathBuf>,
        /// 待检查的文件或目录
        #[arg(required = true)]
        paths: Vec<PathBuf>,
    },
    /// 修改守卫测试需要 GUARD_UPDATE_ACK=1 显式确认
    GuardTests,
}

fn main() -> ExitCode {
    let code = match Cli::parse() {
        Cli::Prd { command } => match command {
            PrdCommand::Status { scope, detail } => commands::prd_status(
                match scope {
                    ScopeArg::All => Scope::All,
                    ScopeArg::Pending => Scope::Pending,
                    ScopeArg::Archive => Scope::Archive,
                },
                detail,
            ),
            PrdCommand::Start {
                prd_file,
                tool,
                branch,
            } => commands::prd_start(&prd_file, tool, branch),
            PrdCommand::Heartbeat { prd_file } => commands::prd_heartbeat(&prd_file),
            PrdCommand::Release { prd_file, force } => commands::prd_release(&prd_file, force),
            PrdCommand::Check {
                paths,
                structure_only,
                archive_ready,
                staged,
                archived,
            } => commands::prd_check(&CheckOptions {
                paths,
                structure_only,
                archive_ready,
                staged,
                archived,
            }),
            PrdCommand::Review { prd, print_only } => commands::prd_review(&prd, print_only),
        },
        Cli::Hook { command } => match command {
            HookCommand::CheckPrd { files } => commands::hook_check_prd(&files),
            HookCommand::LockConflict {
                warn_only: _,
                files,
            } => commands::hook_lock_conflict(&files),
            HookCommand::MaxLines {
                max_lines,
                warn_only,
                glob,
                allow_list_file,
                paths,
            } => commands::hook_max_lines(
                &paths,
                &MaxLinesOptions {
                    max_lines,
                    warn_only,
                    glob,
                    allow_list_file,
                },
            ),
            HookCommand::GuardTests => commands::hook_guard_tests(),
        },
        Cli::Template { command } => match command {
            TemplateCommand::Copy { destination, force } => copy_template(&destination, force),
            TemplateCommand::Sync {
                upstream,
                list,
                apply,
            } => sync_template(&upstream, list, apply),
        },
    };
    ExitCode::from(u8::try_from(code).unwrap_or(1))
}

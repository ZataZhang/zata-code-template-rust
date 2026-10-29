//! 使用 Nacos 配置运行 Docker Compose 的开发期入口。

use std::collections::BTreeMap;
use std::env;
use std::ffi::{OsStr, OsString};
use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode};

use anyhow::{Context, Result, bail};
use tpl_infrastructure::config::load_compose_environment;

const RUNTIME_ENVIRONMENT_KEYS: &[&str] = &[
    "PATH",
    "HOME",
    "TMPDIR",
    "TMP",
    "TEMP",
    "TERM",
    "LANG",
    "LC_ALL",
    "DOCKER_CONFIG",
    "DOCKER_CONTEXT",
    "DOCKER_HOST",
    "DOCKER_TLS",
    "DOCKER_TLS_VERIFY",
    "DOCKER_CERT_PATH",
    "DOCKER_DEFAULT_PLATFORM",
    "SSH_AUTH_SOCK",
    "XDG_RUNTIME_DIR",
    "HTTP_PROXY",
    "HTTPS_PROXY",
    "ALL_PROXY",
    "NO_PROXY",
    "http_proxy",
    "https_proxy",
    "all_proxy",
    "no_proxy",
    "BACKUP_DIR",
    "RETENTION_DAYS",
];

const HOST_PORT_OVERRIDES: &[(&str, &str)] = &[
    ("COMPOSE_WEB_PORT", "WEB_PORT"),
    ("COMPOSE_ADMIN_PORT", "ADMIN_PORT"),
    ("COMPOSE_ACCOUNTS_PORT", "ACCOUNTS_PORT"),
];

#[tokio::main]
async fn main() -> ExitCode {
    match run().await {
        Ok(exit_code) => exit_code,
        Err(error) => {
            eprintln!("compose-config: {error:#}");
            ExitCode::FAILURE
        }
    }
}

async fn run() -> Result<ExitCode> {
    let repository_root = repository_root();
    env::set_current_dir(&repository_root).context("无法切换到仓库根目录")?;

    let invocation = Invocation::parse(env::args_os().skip(1))?;
    let Some(invocation) = invocation else {
        print_usage();
        return Ok(ExitCode::SUCCESS);
    };

    reject_legacy_env_file()?;
    let compose_environment = load_compose_environment().await?;
    reject_legacy_env_file()?;
    let mut child_environment: BTreeMap<String, OsString> = runtime_environment();
    for (key, value) in compose_environment.into_variables() {
        child_environment.insert(key, OsString::from(value));
    }

    let project_name = project_name_from_environment()?;
    child_environment.insert(
        "COMPOSE_PROJECT_NAME".to_string(),
        OsString::from(&project_name),
    );
    apply_host_port_overrides(&mut child_environment)?;
    validate_compose_ports(&child_environment)?;
    report_safe_settings(&project_name, &child_environment);

    match invocation {
        Invocation::Compose {
            stack_mode,
            arguments,
        } => run_compose(stack_mode, arguments, child_environment, &project_name),
        Invocation::Helper {
            script_name,
            arguments,
        } => run_helper(&script_name, arguments, child_environment),
    }
}

enum Invocation {
    Compose {
        stack_mode: StackMode,
        arguments: Vec<OsString>,
    },
    Helper {
        script_name: HelperScript,
        arguments: Vec<OsString>,
    },
}

impl Invocation {
    fn parse(mut arguments: impl Iterator<Item = OsString>) -> Result<Option<Self>> {
        let Some(command) = arguments.next() else {
            return Ok(None);
        };
        if command == OsStr::new("--help") || command == OsStr::new("-h") {
            return Ok(None);
        }

        match command.to_str() {
            Some("compose") => {
                let stack_mode = arguments
                    .next()
                    .ok_or_else(usage_error)?
                    .to_str()
                    .and_then(StackMode::parse)
                    .ok_or_else(usage_error)?;
                let compose_arguments: Vec<OsString> = arguments.collect();
                if compose_arguments.is_empty() {
                    bail!("缺少 Docker Compose 子命令；运行 `compose-config --help` 查看用法");
                }
                validate_compose_command(&compose_arguments)?;
                reject_reserved_compose_flags(&compose_arguments)?;
                Ok(Some(Self::Compose {
                    stack_mode,
                    arguments: compose_arguments,
                }))
            }
            Some("helper") => {
                let script_name = arguments
                    .next()
                    .ok_or_else(usage_error)?
                    .to_str()
                    .and_then(HelperScript::parse)
                    .ok_or_else(usage_error)?;
                Ok(Some(Self::Helper {
                    script_name,
                    arguments: arguments.collect(),
                }))
            }
            _ => bail!("未知命令；运行 `compose-config --help` 查看用法"),
        }
    }
}

#[derive(Clone, Copy)]
enum StackMode {
    Full,
    Accounts,
}

impl StackMode {
    fn parse(value: &str) -> Option<Self> {
        match value {
            "full" => Some(Self::Full),
            "accounts" => Some(Self::Accounts),
            _ => None,
        }
    }

    fn compose_files(self) -> &'static [&'static str] {
        match self {
            Self::Full => &["docker-compose.yml"],
            Self::Accounts => &["docker-compose.yml", "docker-compose.accounts.yml"],
        }
    }
}

#[derive(Clone, Copy)]
enum HelperScript {
    Backup,
    Restore,
    StackSmoke,
}

impl HelperScript {
    fn parse(value: &str) -> Option<Self> {
        match value {
            "db-backup" => Some(Self::Backup),
            "db-restore" => Some(Self::Restore),
            "stack-smoke" => Some(Self::StackSmoke),
            _ => None,
        }
    }

    fn path(self, repository_root: &Path) -> PathBuf {
        let script_name = match self {
            Self::Backup => "db-backup.sh",
            Self::Restore => "db-restore.sh",
            Self::StackSmoke => "stack-smoke.sh",
        };
        repository_root
            .join("scripts")
            .join("lib")
            .join(script_name)
    }
}

fn run_compose(
    stack_mode: StackMode,
    compose_arguments: Vec<OsString>,
    child_environment: BTreeMap<String, OsString>,
    project_name: &str,
) -> Result<ExitCode> {
    let mut command = Command::new("docker");
    command
        .current_dir(repository_root())
        .env_clear()
        .envs(child_environment)
        .arg("compose")
        .arg("--env-file")
        .arg("/dev/null")
        .arg("--project-name")
        .arg(project_name);
    for compose_file in stack_mode.compose_files() {
        command.arg("--file").arg(compose_file);
    }
    command.args(compose_arguments);
    execute(command)
}

fn run_helper(
    script_name: &HelperScript,
    script_arguments: Vec<OsString>,
    child_environment: BTreeMap<String, OsString>,
) -> Result<ExitCode> {
    let script_path = script_name.path(&repository_root());
    let mut command = Command::new("bash");
    command
        .current_dir(repository_root())
        .env_clear()
        .envs(child_environment)
        .arg(script_path)
        .args(script_arguments);
    execute(command)
}

fn execute(mut command: Command) -> Result<ExitCode> {
    let mut child = command
        .spawn()
        .context("无法启动 Docker Compose 运维命令")?;
    let status = child
        .wait()
        .context("等待 Docker Compose 运维命令结束失败")?;
    Ok(status
        .code()
        .and_then(|code| u8::try_from(code).ok())
        .map_or(ExitCode::FAILURE, ExitCode::from))
}

fn runtime_environment() -> BTreeMap<String, OsString> {
    RUNTIME_ENVIRONMENT_KEYS
        .iter()
        .filter_map(|key| env::var_os(key).map(|value| ((*key).to_string(), value)))
        .collect()
}

fn project_name_from_environment() -> Result<String> {
    let project_name = env::var("COMPOSE_PROJECT_NAME").unwrap_or_else(|_| "tpl".to_string());
    if project_name.is_empty()
        || project_name.len() > 63
        || !project_name.bytes().enumerate().all(|(index, value)| {
            value.is_ascii_lowercase()
                || value.is_ascii_digit()
                || (index > 0 && matches!(value, b'-' | b'_'))
        })
    {
        bail!("COMPOSE_PROJECT_NAME 只能使用小写字母、数字、连字符和下划线（最多 63 字符）");
    }
    Ok(project_name)
}

fn apply_host_port_overrides(child_environment: &mut BTreeMap<String, OsString>) -> Result<()> {
    for (source_name, compose_name) in HOST_PORT_OVERRIDES {
        let Some(value) = env::var_os(source_name) else {
            continue;
        };
        let value = value
            .into_string()
            .map_err(|_| anyhow::anyhow!("{source_name} 不是有效的 UTF-8"))?;
        child_environment.insert((*compose_name).to_string(), OsString::from(value));
    }
    Ok(())
}

fn validate_compose_ports(child_environment: &BTreeMap<String, OsString>) -> Result<()> {
    for port_name in ["WEB_PORT", "ADMIN_PORT", "ACCOUNTS_PORT"] {
        let Some(port_value) = child_environment.get(port_name) else {
            continue;
        };
        let port_value = port_value
            .to_str()
            .ok_or_else(|| anyhow::anyhow!("{port_name} 不是有效的 UTF-8"))?;
        let port_number = port_value
            .parse::<u16>()
            .with_context(|| format!("{port_name} 必须是 1 到 65535 的端口号"))?;
        if port_number == 0 {
            bail!("{port_name} 必须是 1 到 65535 的端口号");
        }
    }
    Ok(())
}

fn report_safe_settings(project_name: &str, child_environment: &BTreeMap<String, OsString>) {
    let port_summary = ["WEB_PORT", "ADMIN_PORT", "ACCOUNTS_PORT"]
        .into_iter()
        .filter_map(|key| {
            child_environment
                .get(key)
                .and_then(|value| value.to_str())
                .map(|value| format!("{key}={value}"))
        })
        .collect::<Vec<_>>()
        .join(" ");
    eprintln!("Compose project={project_name}; {port_summary}");
}

fn reject_reserved_compose_flags(arguments: &[OsString]) -> Result<()> {
    for argument in arguments {
        let argument = argument.to_string_lossy();
        let short_option_override = (argument.starts_with("-f") && argument.len() > 2)
            || (argument.starts_with("-p") && argument.len() > 2);
        if matches!(
            argument.as_ref(),
            "--env-file" | "--project-name" | "-p" | "--file" | "-f" | "--project-directory"
        ) || argument.starts_with("--env-file=")
            || argument.starts_with("--project-name=")
            || argument.starts_with("--file=")
            || argument.starts_with("--project-directory=")
            || short_option_override
        {
            bail!("Compose 的 env 文件、project 与配置文件参数由入口固定管理");
        }
    }
    Ok(())
}

fn validate_compose_command(arguments: &[OsString]) -> Result<()> {
    let Some(command_name) = arguments.first().and_then(|value| value.to_str()) else {
        bail!("Docker Compose 子命令无效");
    };
    let allowed_commands = [
        "up", "stop", "down", "logs", "ps", "start", "restart", "build", "pull", "push", "images",
        "rm", "create", "kill", "pause", "unpause", "top", "events", "wait",
    ];
    if allowed_commands.contains(&command_name) {
        return Ok(());
    }

    let config_arguments: Vec<&str> = arguments
        .iter()
        .filter_map(|argument| argument.to_str())
        .collect();
    if config_arguments.as_slice() == ["config", "--quiet"]
        || config_arguments.as_slice() == ["config", "--services"]
    {
        return Ok(());
    }

    bail!("Docker Compose 子命令不在允许列表中；不支持 exec、run 或导出完整 config")
}

fn reject_legacy_env_file() -> Result<()> {
    reject_legacy_env_file_value(env::var_os("ENV_FILE"))
}

fn reject_legacy_env_file_value(value: Option<OsString>) -> Result<()> {
    if value.is_some() {
        bail!("ENV_FILE 已停用；Compose 配置只从 Nacos 加载");
    }
    Ok(())
}

fn repository_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .unwrap_or_else(|_| Path::new(env!("CARGO_MANIFEST_DIR")).join("../.."))
}

fn usage_error() -> anyhow::Error {
    anyhow::anyhow!("命令参数无效；运行 `compose-config --help` 查看用法")
}

fn print_usage() {
    eprintln!(
        "用法:\n  compose-config compose <full|accounts> <允许的 Compose 运维子命令> [参数]\n  compose-config helper <db-backup|db-restore|stack-smoke> [args...]\n\n仅允许配置安全的运维子命令；配置验证只接受 `config --quiet` 或 `config --services`。\nCompose Data ID 从项目配置 `[nacos].compose_data_id` 读取，默认 rust-study-compose.properties。"
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reserved_compose_file_and_project_overrides_are_rejected() {
        for argument in [
            "--env-file",
            "--env-file=/tmp/local.env",
            "--project-name",
            "--project-name=other",
            "--file",
            "--file=docker-compose.override.yml",
            "-f",
            "-fother.yml",
            "-f=other.yml",
            "-p",
            "-ptpl-other",
            "-p=tpl-other",
            "--project-directory",
        ] {
            assert!(
                reject_reserved_compose_flags(&[OsString::from(argument)]).is_err(),
                "reserved option must be rejected: {argument}"
            );
        }
    }

    #[test]
    fn service_selection_and_non_reserved_flags_are_allowed() {
        assert!(
            validate_compose_command(&[
                OsString::from("up"),
                OsString::from("-d"),
                OsString::from("accounts"),
            ])
            .is_ok()
        );
        assert!(
            reject_reserved_compose_flags(&[
                OsString::from("up"),
                OsString::from("-d"),
                OsString::from("accounts"),
            ])
            .is_ok()
        );
    }

    #[test]
    fn config_output_and_arbitrary_command_execution_are_restricted() {
        assert!(
            validate_compose_command(&[OsString::from("config"), OsString::from("--quiet")])
                .is_ok()
        );
        assert!(
            validate_compose_command(&[OsString::from("config"), OsString::from("--services"),])
                .is_ok()
        );
        for arguments in [
            vec![OsString::from("config")],
            vec![OsString::from("config"), OsString::from("--environment")],
            vec![
                OsString::from("config"),
                OsString::from("--format"),
                OsString::from("json"),
            ],
            vec![
                OsString::from("exec"),
                OsString::from("accounts"),
                OsString::from("env"),
            ],
            vec![
                OsString::from("run"),
                OsString::from("--rm"),
                OsString::from("accounts"),
            ],
        ] {
            assert!(validate_compose_command(&arguments).is_err());
        }
    }

    #[test]
    fn legacy_env_file_is_rejected_when_set() {
        assert!(reject_legacy_env_file_value(None).is_ok());
        assert!(reject_legacy_env_file_value(Some(OsString::from("old.env"))).is_err());
    }
}

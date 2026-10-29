//! 本机文件与 Git 改动查看器的命令入口与常驻进程管理。

mod server;
mod workspace;

use std::fs::{self, OpenOptions};
use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result, bail};

const LOOPBACK_HOST: &str = "127.0.0.1";
const DEFAULT_PORT: u16 = 8791;
const DEFAULT_IDLE_TIMEOUT_SECONDS: f64 = 1800.0;
const REGISTRY_FILE_NAME: &str = ".env.view-state";
const SERVICE_LOG_RELATIVE_PATH: &str = "logs/view-viewer.log";

/// 一次 `just view` 调用的选项。
pub struct LaunchOptions {
    /// 打开后直达的仓库相对路径。
    pub path: String,
    /// 是否进入文件视图；缺省进入改动视图。
    pub files: bool,
    /// 显式指定的监听端口。
    pub port: Option<u16>,
    /// 空闲回收秒数；`None` 使用默认值，非正数关闭回收。
    pub idle_timeout_seconds: Option<f64>,
    /// 是否跳过实例复用并新起服务。
    pub no_reuse: bool,
    /// 是否只打印 URL。
    pub no_open: bool,
    /// 是否停止当前仓库的服务。
    pub stop: bool,
}

#[derive(Debug)]
struct InstanceRecord {
    process_id: u32,
    port: u16,
    repository_root: PathBuf,
}

/// 打开或启动查看器；错误以非零退出码返回给 CLI。
pub fn launch(options: LaunchOptions) -> i32 {
    match launch_inner(options) {
        Ok(code) => code,
        Err(error) => {
            eprintln!("查看器启动失败：{error:#}");
            1
        }
    }
}

/// 启动查看器 HTTP 服务。
pub fn run_server(repository_root: PathBuf, port: u16, idle_timeout_seconds: f64) -> i32 {
    if !idle_timeout_seconds.is_finite() {
        eprintln!("--idle-timeout 必须是有限数字");
        return 2;
    }
    let runtime = match tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
    {
        Ok(runtime) => runtime,
        Err(error) => {
            eprintln!("无法创建查看器运行时：{error}");
            return 1;
        }
    };
    runtime.block_on(server::run(repository_root, port, idle_timeout_seconds))
}

fn launch_inner(options: LaunchOptions) -> Result<i32> {
    let repository_root = resolve_repository_root()?;
    if options.stop {
        return stop_instance(&repository_root);
    }

    let previous_record = read_instance_record(&repository_root);
    if let Some(record) = previous_record.as_ref()
        && !options.no_reuse
        && is_reusable(record, &repository_root)
    {
        let url = build_viewer_url(record.port, &options.path, options.files);
        return open_viewer(
            &url,
            &format!(
                "复用查看器实例（pid {}，端口 {}）",
                record.process_id, record.port
            ),
            options.no_open,
        );
    }

    if let Some(record) = previous_record.as_ref() {
        if is_process_alive(record.process_id) {
            eprintln!(
                "已有实例不可复用（pid {}，端口 {}），启动新实例。",
                record.process_id, record.port
            );
        }
        clear_instance_record(&repository_root, Some(record.process_id));
    }

    let idle_timeout_seconds = options
        .idle_timeout_seconds
        .unwrap_or(DEFAULT_IDLE_TIMEOUT_SECONDS);
    if !idle_timeout_seconds.is_finite() {
        bail!("--idle-timeout 必须是有限数字");
    }
    let port = select_port(
        options.port,
        previous_record.as_ref().map(|record| record.port),
    )?;
    let process_id = start_server(&repository_root, port, idle_timeout_seconds)?;
    if !wait_until_ready(port, process_id) {
        stop_process(process_id);
        bail!(
            "查看器服务在 5 秒内未就绪（pid {process_id}，端口 {port}）；日志：{}",
            repository_root.join(SERVICE_LOG_RELATIVE_PATH).display()
        );
    }

    write_instance_record(&InstanceRecord {
        process_id,
        port,
        repository_root: repository_root.clone(),
    })?;
    let url = build_viewer_url(port, &options.path, options.files);
    open_viewer(
        &url,
        &format!("已启动查看器实例（pid {process_id}，端口 {port}）"),
        options.no_open,
    )
}

fn resolve_repository_root() -> Result<PathBuf> {
    let working_directory = std::env::current_dir().context("无法读取当前目录")?;
    let root = crate::git::run(&["rev-parse", "--show-toplevel"], Some(&working_directory))
        .filter(|path| !path.is_empty())
        .map(PathBuf::from)
        .context("当前目录不在 Git 仓库内")?;
    root.canonicalize()
        .with_context(|| format!("无法解析仓库根目录 {}", root.display()))
}

fn read_instance_record(repository_root: &Path) -> Option<InstanceRecord> {
    let content = fs::read_to_string(repository_root.join(REGISTRY_FILE_NAME)).ok()?;
    let mut process_id = None;
    let mut port = None;
    let mut recorded_root = None;
    for line in content.lines() {
        let Some((key, value)) = line.split_once('=') else {
            continue;
        };
        match key.trim() {
            "VIEW_PID" => process_id = value.trim().parse().ok(),
            "VIEW_PORT" => port = value.trim().parse().ok(),
            "VIEW_REPO_ROOT" => recorded_root = Some(PathBuf::from(value.trim())),
            _ => {}
        }
    }
    Some(InstanceRecord {
        process_id: process_id?,
        port: port?,
        repository_root: recorded_root?,
    })
}

fn write_instance_record(record: &InstanceRecord) -> Result<()> {
    let content = format!(
        "VIEW_PID={}\nVIEW_PORT={}\nVIEW_REPO_ROOT={}\nVIEW_STARTED_AT={}\n",
        record.process_id,
        record.port,
        record.repository_root.display(),
        SystemTime::now().duration_since(UNIX_EPOCH)?.as_secs()
    );
    fs::write(record.repository_root.join(REGISTRY_FILE_NAME), content)
        .context("无法写入查看器实例登记")
}

fn clear_instance_record(repository_root: &Path, expected_process_id: Option<u32>) {
    if let Some(expected_process_id) = expected_process_id
        && read_instance_record(repository_root)
            .is_none_or(|record| record.process_id != expected_process_id)
    {
        return;
    }
    let _ = fs::remove_file(repository_root.join(REGISTRY_FILE_NAME));
}

fn is_reusable(record: &InstanceRecord, repository_root: &Path) -> bool {
    record.repository_root == repository_root
        && is_process_alive(record.process_id)
        && TcpStream::connect((LOOPBACK_HOST, record.port)).is_ok()
}

fn is_process_alive(process_id: u32) -> bool {
    #[cfg(unix)]
    {
        Command::new("kill")
            .args(["-0", &process_id.to_string()])
            .status()
            .is_ok_and(|status| status.success())
    }
    #[cfg(windows)]
    {
        Command::new("tasklist")
            .args(["/FI", &format!("PID eq {process_id}")])
            .output()
            .is_ok_and(|output| {
                String::from_utf8_lossy(&output.stdout).contains(&process_id.to_string())
            })
    }
}

fn stop_process(process_id: u32) {
    #[cfg(unix)]
    let _ = Command::new("kill")
        .args(["-TERM", &process_id.to_string()])
        .status();
    #[cfg(windows)]
    let _ = Command::new("taskkill")
        .args(["/PID", &process_id.to_string(), "/T", "/F"])
        .status();
}

fn stop_instance(repository_root: &Path) -> Result<i32> {
    let Some(record) = read_instance_record(repository_root) else {
        println!("没有登记在案的查看器实例，无需回收。");
        return Ok(0);
    };
    if is_process_alive(record.process_id) {
        stop_process(record.process_id);
        let deadline = Instant::now() + Duration::from_secs(5);
        while Instant::now() < deadline && TcpStream::connect((LOOPBACK_HOST, record.port)).is_ok()
        {
            thread::sleep(Duration::from_millis(50));
        }
        println!(
            "已停止查看器实例（pid {}，端口 {}）。",
            record.process_id, record.port
        );
    } else {
        println!(
            "登记项已陈旧（pid {} 已退出），只清理登记。",
            record.process_id
        );
    }
    clear_instance_record(repository_root, Some(record.process_id));
    Ok(0)
}

fn select_port(requested_port: Option<u16>, remembered_port: Option<u16>) -> Result<u16> {
    if let Some(port) = requested_port {
        if port == 0 {
            bail!("--port 必须在 1 到 65535 之间");
        }
        if !is_port_available(port) {
            bail!("端口 {port} 已被占用，请换一个或省略 --port");
        }
        return Ok(port);
    }
    for port in [remembered_port, Some(DEFAULT_PORT)].into_iter().flatten() {
        if is_port_available(port) {
            return Ok(port);
        }
    }
    let listener = TcpListener::bind((LOOPBACK_HOST, 0)).context("无法分配本机空闲端口")?;
    Ok(listener.local_addr()?.port())
}

fn is_port_available(port: u16) -> bool {
    TcpListener::bind((LOOPBACK_HOST, port)).is_ok()
}

fn start_server(repository_root: &Path, port: u16, idle_timeout_seconds: f64) -> Result<u32> {
    let log_path = repository_root.join(SERVICE_LOG_RELATIVE_PATH);
    fs::create_dir_all(log_path.parent().context("日志路径没有父目录")?)
        .context("无法创建查看器日志目录")?;
    let log_file = OpenOptions::new()
        .create(true)
        .append(true)
        .open(&log_path)
        .with_context(|| format!("无法打开查看器日志 {}", log_path.display()))?;
    let stderr = Stdio::from(log_file.try_clone()?);
    let mut command = Command::new(std::env::current_exe().context("无法定位 xtask 可执行文件")?);
    command
        .arg("view-server")
        .arg("--repo-root")
        .arg(repository_root)
        .arg("--port")
        .arg(port.to_string())
        .arg("--idle-timeout")
        .arg(idle_timeout_seconds.to_string())
        .stdin(Stdio::null())
        .stdout(Stdio::from(log_file))
        .stderr(stderr);
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        command.process_group(0);
    }
    let child = command.spawn().context("无法启动查看器后台服务")?;
    Ok(child.id())
}

fn wait_until_ready(port: u16, process_id: u32) -> bool {
    let deadline = Instant::now() + Duration::from_secs(5);
    while Instant::now() < deadline {
        if !is_process_alive(process_id) {
            return false;
        }
        if let Ok(mut stream) = TcpStream::connect((LOOPBACK_HOST, port)) {
            let _ = stream.set_read_timeout(Some(Duration::from_secs(1)));
            let _ = stream.write_all(
                b"GET /api/info HTTP/1.1\r\nHost: 127.0.0.1\r\nConnection: close\r\n\r\n",
            );
            let mut response = [0_u8; 128];
            if let Ok(read_count) = stream.read(&mut response)
                && String::from_utf8_lossy(&response[..read_count]).starts_with("HTTP/1.1 200")
            {
                return true;
            }
        }
        thread::sleep(Duration::from_millis(20));
    }
    false
}

fn build_viewer_url(port: u16, requested_path: &str, files_view: bool) -> String {
    let view = if files_view { "files" } else { "diff" };
    let mut url = format!("http://{LOOPBACK_HOST}:{port}/?view={view}");
    if !requested_path.is_empty() {
        url.push_str("&path=");
        url.push_str(&percent_encode(requested_path));
    }
    url
}

fn open_viewer(url: &str, report: &str, no_open: bool) -> Result<i32> {
    println!("{report}");
    println!("{url}");
    if no_open {
        return Ok(0);
    }
    let mut command = if cfg!(target_os = "macos") {
        let mut command = Command::new("open");
        command.arg(url);
        command
    } else if cfg!(target_os = "windows") {
        let mut command = Command::new("cmd");
        command.args(["/C", "start", "", url]);
        command
    } else {
        let mut command = Command::new("xdg-open");
        command.arg(url);
        command
    };
    command
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .context("无法启动系统浏览器")?;
    Ok(0)
}

fn percent_encode(value: &str) -> String {
    let mut encoded = String::with_capacity(value.len());
    for byte in value.bytes() {
        if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.' | b'~') {
            encoded.push(char::from(byte));
        } else {
            encoded.push_str(&format!("%{byte:02X}"));
        }
    }
    encoded
}

pub(super) fn clear_server_record(repository_root: &Path, process_id: u32) {
    clear_instance_record(repository_root, Some(process_id));
}

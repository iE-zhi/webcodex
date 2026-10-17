//! 可选本机续跑适配：使用 Runner 自己的认证读取短租约，持有系统电源并有界打开固定 ChatGPT 会话。
mod platform;
use crate::webcodex_runner::config::RunnerConfig;
use crate::webcodex_runner::transport::http_client::read_bounded_response_body;
use serde::Deserialize;
use std::{
    path::PathBuf,
    process::{Child, Command, Stdio},
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
    thread::JoinHandle,
    time::{Duration, Instant},
};
use webcodex_core::continuation::{
    LocalLease, LocalLeaseRequest, LocalStatus, LOCAL_LEASE_MS, MAX_LOCAL_WAKES,
};

/// 轮询十五秒，单次 HTTP 最多五秒；停止信号每二百五十毫秒检查，不阻塞 Runner 关机。
const POLL_INTERVAL: Duration = Duration::from_secs(15);
/// HTTP 从建立连接到读取完整有界响应最多五秒。
const HTTP_TIMEOUT: Duration = Duration::from_secs(5);
/// 本机租约响应最多十六 KiB，覆盖四个固定会话 URL。
const MAX_RESPONSE: usize = 16 * 1024;
/// Runner 租约固定路径；Server 不接受浏览器 PAT 代替 agent 身份。
const LEASE_PATH: &str = "/api/shell/agent/continuation";
/// 待回收的浏览器启动句柄最多四个，不拥有或终止用户浏览器进程。
const MAX_LAUNCHERS: usize = 4;
/// 有界停止检查间隔，单位毫秒。
const STOP_INTERVAL: Duration = Duration::from_millis(250);

/// 启动时读取的本机配置；默认不运行，需要明确启用，修改后重启 Runner 程序生效。
#[derive(Clone, Debug, Default, Deserialize, PartialEq, Eq)]
#[serde(default, deny_unknown_fields)]
pub(crate) struct ContinuationConfig {
    /// 是否允许本机读取续跑租约；关闭时不轮询、不启动浏览器或持有电源。
    pub(crate) enabled: bool,
    /// 本机 Chrome/Edge 可执行文件绝对路径，只由受保护的 Runner 配置提供。
    pub(crate) browser_path: Option<PathBuf>,
    /// 确切浏览器 profile 目录名，如 Default 或 Profile 1；不能包含路径分隔符或参数。
    pub(crate) browser_profile: Option<String>,
}

impl ContinuationConfig {
    /// 检查本机可执行文件和 profile；不接受网络提供可执行程序、Shell 或任意启动参数。
    /// # Errors
    /// 启用但路径、文件名或 profile 不安全时拒绝启动 Runner，不执行配置中的程序。
    pub(crate) fn validate(&self) -> Result<(), String> {
        if !self.enabled {
            return Ok(());
        }
        if !cfg!(any(windows, target_os = "linux")) {
            return Err(
                "local continuation requires a Windows or Linux login-session Runner".into(),
            );
        }
        let path = self
            .browser_path
            .as_ref()
            .filter(|path| path.is_absolute() && path.is_file())
            .ok_or("continuation browser_path must name an installed browser by absolute path")?;
        let name = path
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("")
            .to_ascii_lowercase();
        if ![
            "chrome.exe",
            "msedge.exe",
            "chrome",
            "google-chrome",
            "google-chrome-stable",
            "chromium",
            "chromium-browser",
            "microsoft-edge",
            "google chrome",
            "microsoft edge",
        ]
        .contains(&name.as_str())
        {
            return Err(
                "continuation browser_path is not a supported Chrome/Edge executable".into(),
            );
        }
        let profile = self
            .browser_profile
            .as_deref()
            .ok_or("continuation browser_profile is required")?;
        if profile.is_empty()
            || profile.len() > 128
            || profile == "."
            || profile == ".."
            || !profile.bytes().all(|byte| {
                byte.is_ascii_alphanumeric() || matches!(byte, b' ' | b'.' | b'_' | b'-')
            })
        {
            return Err(
                "continuation browser_profile must be one safe profile directory name".into(),
            );
        }
        Ok(())
    }
}

/// 重新验证 Server 发来的固定 HTTPS 会话 URL；拒绝凭据、查询、片段、端口及任意其他站点。
/// # 参数
/// - `raw`：单个已领取的 URL，最多一千零二十四字节，不接受命令或参数。
/// # 返回
/// 合法 ChatGPT 会话返回 true；其他值不启动任何进程。
pub(crate) fn valid_chat_url(raw: &str) -> bool {
    if raw.len() > 1024 {
        return false;
    }
    let Ok(url) = url::Url::parse(raw) else {
        return false;
    };
    if !["https://chatgpt.com", "https://chat.openai.com"]
        .contains(&url.origin().ascii_serialization().as_str())
        || !url.username().is_empty()
        || url.password().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
    {
        return false;
    }
    let parts: Vec<_> = url.path().split('/').collect();
    let id = match parts.as_slice() {
        ["", "c", id] => id,
        ["", "g", prefix, "c", id]
            if !prefix.is_empty()
                && prefix.len() <= 200
                && prefix
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'-')) =>
        {
            id
        }
        _ => return false,
    };
    uuid::Uuid::parse_str(id).is_ok() && id.len() == 36
}

/// 同一工作线程请求短租约；HTTP 读取和解析均有上限，失败不返回或重放唤醒动作。
/// # 参数
/// - `client`：线程独占的复用 HTTP 连接池，超时五秒且禁止重定向。
/// - `cfg`：启动时的受保护 Runner 账户配置，凭据只用于 Authorization。
/// - `request`：本次新 UUID 及当前进程身份，返回必须逐项匹配。
/// # Errors
/// 网络、认证、大小、协议或 URL 不合法时返回固定码，不回显响应正文或令牌。
fn request_lease(
    client: &reqwest::blocking::Client,
    cfg: &RunnerConfig,
    request: &LocalLeaseRequest,
) -> Result<LocalLease, &'static str> {
    let mut response = client
        .post(format!(
            "{}{LEASE_PATH}",
            cfg.server_url.trim_end_matches('/')
        ))
        .bearer_auth(&cfg.token)
        .json(request)
        .send()
        .map_err(|_| "continuation_lease_unavailable")?;
    if !response.status().is_success() {
        return Err("continuation_lease_rejected");
    }
    let length = response.content_length();
    let body = read_bounded_response_body(&mut response, length, MAX_RESPONSE)
        .map_err(|_| "continuation_lease_invalid")?;
    if body.exceeded_limit {
        return Err("continuation_lease_invalid");
    }
    let lease: LocalLease =
        serde_json::from_slice(&body.bytes).map_err(|_| "continuation_lease_invalid")?;
    if lease.request_id != request.request_id
        || lease.valid_for_ms > LOCAL_LEASE_MS
        || lease.wake.len() > MAX_LOCAL_WAKES
        || lease
            .wake
            .iter()
            .any(|wake| uuid::Uuid::parse_str(&wake.task_id).is_err() || !valid_chat_url(&wake.url))
    {
        return Err("continuation_lease_invalid");
    }
    Ok(lease)
}

/// 启动一个受 Runner 关机标志控制的后台线程；禁用时不创建线程或进行网络 I/O。
/// # 参数
/// - `cfg`：启动配置快照，线程拥有副本；热更新不改变可执行文件或凭据。
/// - `instance`：当前 Runner 注册的进程 UUID，不能跨重启复用。
/// - `shutdown`：Runner 生命周期标志，只读取，归现有协调器控制。
/// # Errors
/// 本机配置或线程创建失败返回固定错误，调用方记录后继续原有工程服务。
pub(crate) fn start(
    cfg: RunnerConfig,
    instance: String,
    shutdown: Arc<AtomicBool>,
) -> Result<Option<JoinHandle<()>>, String> {
    if !cfg.continuation.enabled {
        return Ok(None);
    }
    cfg.continuation.validate()?;
    std::thread::Builder::new()
        .name("runner-continuation".into())
        .spawn(move || run(cfg, instance, shutdown))
        .map(Some)
        .map_err(|_| "continuation_thread_unavailable".into())
}

/// 读取租约、刷新电源和回收已退出启动句柄；用户浏览器不会因任务暂停或 Runner 停止而被杀掉。
/// # 参数
/// - `cfg`：本线程拥有的启动配置，错误只记录固定码，不输出凭据。
/// - `instance`：本进程注册身份。
/// - `shutdown`：共享停止标志；五秒 HTTP 上限及短等待保证有界退出。
fn run(cfg: RunnerConfig, instance: String, shutdown: Arc<AtomicBool>) {
    let client = match reqwest::blocking::Client::builder()
        .timeout(HTTP_TIMEOUT)
        .connect_timeout(HTTP_TIMEOUT)
        .redirect(reqwest::redirect::Policy::none())
        .build()
    {
        Ok(client) => client,
        Err(_) => {
            tracing::error!(
                code = "continuation_http_unavailable",
                "local continuation unavailable"
            );
            return;
        }
    };
    let mut power = platform::PowerLease::default();
    let mut launchers: Vec<Child> = Vec::new();
    let mut last_error = "";
    while !shutdown.load(Ordering::Acquire) {
        let started = Instant::now();
        let absent = cfg
            .continuation
            .browser_path
            .as_deref()
            .is_some_and(platform::browser_absent);
        let request = LocalLeaseRequest {
            status: local_status(last_error),
            client_id: cfg.client_id.clone(),
            agent_instance_id: instance.clone(),
            request_id: uuid::Uuid::new_v4().to_string(),
            browser_absent: absent,
        };
        let result = request_lease(&client, &cfg, &request);
        let mut lease_until = Instant::now();
        let status = match result {
            Ok(lease) => {
                lease_until = started + Duration::from_millis(u64::from(lease.valid_for_ms));
                if lease_until > Instant::now() && !shutdown.load(Ordering::Acquire) {
                    let power_result = power.update(lease.keep_awake);
                    let launch_result = if shutdown.load(Ordering::Acquire) {
                        Ok(())
                    } else {
                        launch(&cfg.continuation, &lease, &mut launchers)
                    };
                    power_result.and(launch_result)
                } else {
                    power.update(false)
                }
            }
            Err(code) => {
                if let Err(error) = power.update(false) {
                    tracing::error!(code = error, "local continuation power release failed");
                }
                Err(code)
            }
        };
        let code = status.err().unwrap_or("");
        if code != last_error {
            if !code.is_empty() {
                tracing::warn!(code, "local continuation action deferred");
            }
            last_error = code;
        }
        let next = started + POLL_INTERVAL;
        while Instant::now() < next && !shutdown.load(Ordering::Acquire) {
            if Instant::now() >= lease_until {
                if let Err(code) = power.update(false) {
                    tracing::error!(code, "local continuation power release failed");
                }
                lease_until = next;
            }
            std::thread::sleep(STOP_INTERVAL.min(next.saturating_duration_since(Instant::now())));
        }
    }
    // Drop 只释放本线程持有的电源资源；浏览器句柄销毁不会终止用户进程。
}

/// 将本模块固定错误转换成无正文的健康状态，不能把任意异常写进 Server 任务视图。
/// # 参数
/// - `code`：本模块固定码；空串表示没有已知失败，其他未知码按浏览器失败处理。
fn local_status(code: &str) -> LocalStatus {
    match code {
        "" => LocalStatus::Ready,
        "continuation_lease_invalid" => LocalStatus::ProtocolError,
        "continuation_lease_unavailable" | "continuation_lease_rejected" => {
            LocalStatus::LeaseUnavailable
        }
        "continuation_power_unavailable" | "continuation_power_release_failed" => {
            LocalStatus::PowerUnavailable
        }
        _ => LocalStatus::BrowserUnavailable,
    }
}

/// 回收已退出的启动器并在进程缺失时打开固定已授权 URL；不终止、接管或控制现有浏览器。
/// # 参数
/// - `cfg`：可信本机启动路径及安全 profile 名，不含远程参数。
/// - `lease`：已检查大小、nonce、期限和 URL 的本次租约。
/// - `launchers`：当前线程至多四个浏览器启动句柄，已退出者及时 wait 回收。
/// # Errors
/// 进程探测未知时不打开；句柄、文件或启动失败返回固定码，下次领取仍受 Server 原修复预算。
#[allow(
    clippy::zombie_processes,
    reason = "句柄交给 run 的有界集合，每轮 try_wait 回收；仍存活的用户浏览器不能阻塞 Runner 退出"
)]
fn launch(
    cfg: &ContinuationConfig,
    lease: &LocalLease,
    launchers: &mut Vec<Child>,
) -> Result<(), &'static str> {
    for at in (0..launchers.len()).rev() {
        match launchers[at].try_wait() {
            Ok(Some(_)) => {
                launchers.remove(at);
            }
            Ok(None) => {}
            Err(_) => return Err("continuation_launcher_unavailable"),
        }
    }
    if lease.wake.is_empty() {
        return Ok(());
    }
    let path = cfg
        .browser_path
        .as_deref()
        .ok_or("continuation_browser_unconfigured")?;
    if !platform::browser_absent(path) {
        return Ok(());
    }
    if launchers.len() >= MAX_LAUNCHERS {
        return Err("continuation_launcher_capacity");
    }
    let profile = cfg
        .browser_profile
        .as_deref()
        .ok_or("continuation_browser_unconfigured")?;
    let mut command = Command::new(path);
    command
        .arg(format!("--profile-directory={profile}"))
        .args(lease.wake.iter().map(|wake| wake.url.as_str()))
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    // 浏览器是用户交互窗口；此处明确允许显示，不使用隐藏服务进程或 Shell 启动。
    platform::detach_browser(&mut command);
    let child = command
        .spawn()
        .map_err(|_| "continuation_browser_start_failed")?;
    launchers.push(child);
    Ok(())
}

#[cfg(test)]
#[path = "../../../../../tests/chat-continuation/local_runner.rs"]
mod tests;

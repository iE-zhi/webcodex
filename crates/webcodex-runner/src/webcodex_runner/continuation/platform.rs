//! 本机浏览器存活与电源资源；探测不能证明退出时拒绝唤醒，电源只归续跑工作线程持有。
use std::{marker::PhantomData, path::Path, rc::Rc};

/// 进程探测最多检查六万五千五百三十六个进程，超限或未知均不判定浏览器退出。
#[cfg(any(windows, target_os = "linux"))]
const MAX_PROCESSES: usize = 65_536;

/// 只属于创建线程的电源租约；不能跨线程移动，销毁时撤销本线程或本进程组的防休眠资源。
#[derive(Default)]
pub(super) struct PowerLease {
    /// 禁止 Send/Sync，确保 Windows 执行状态在设置它的同一个线程清除。
    _thread: PhantomData<Rc<()>>,
    /// 当前线程已成功持有防休眠；默认 false，不影响其他线程。
    #[cfg(windows)]
    held: bool,
    /// Unix 防休眠进程组及发行时间；最多一个，暂停、失联和销毁会终止并回收。
    #[cfg(unix)]
    child: Option<(webcodex_process::ManagedChild, std::time::Instant)>,
}

impl PowerLease {
    /// 更新当前线程的系统防休眠，不保持显示器常亮，不修改系统电源方案。
    /// # 参数
    /// - `active`：真实未过期租约是否仍授权防休眠；false 立即释放本对象资源。
    /// # Errors
    /// 操作系统拒绝或固定平台工具不可用时返回固定错误，不重试执行未知操作。
    pub(super) fn update(&mut self, active: bool) -> Result<(), &'static str> {
        #[cfg(windows)]
        {
            use windows_sys::Win32::System::Power::{
                SetThreadExecutionState, ES_CONTINUOUS, ES_SYSTEM_REQUIRED,
            };
            if !active && !self.held {
                return Ok(());
            }
            let flags = ES_CONTINUOUS | if active { ES_SYSTEM_REQUIRED } else { 0 };
            // SAFETY: 只修改本线程状态，不借用外部内存；本类型不能跨线程移动，Drop 在同线程清除。
            if unsafe { SetThreadExecutionState(flags) } == 0 {
                return Err("continuation_power_unavailable");
            }
            self.held = active;
            Ok(())
        }
        #[cfg(unix)]
        {
            use std::{
                process::{Command, Stdio},
                time::{Duration, Instant},
            };
            /// 固定平台工具最多持有四十五秒；每次租约仍由外层更早截止时间控制。
            const INHIBIT_SECONDS: &str = "45";
            /// 剩余不足一个轮询周期时先建立新租约，再释放旧进程组，避免续期缺口。
            const RENEW_AFTER: Duration = Duration::from_secs(25);
            if !active {
                if let Some((mut child, _)) = self.child.take() {
                    child
                        .terminate_tree()
                        .map_err(|_| "continuation_power_release_failed")?;
                    // ManagedChild 的 Drop 在终止后有界 wait 回收，不留下后台资源。
                }
                return Ok(());
            }
            if let Some((child, since)) = self.child.as_mut() {
                match child.try_wait() {
                    Ok(None) if since.elapsed() < RENEW_AFTER => return Ok(()),
                    Ok(Some(status)) if !status.success() => {
                        return Err("continuation_power_unavailable")
                    }
                    Ok(_) => {}
                    Err(_) => return Err("continuation_power_unavailable"),
                }
            }
            #[cfg(target_os = "linux")]
            let mut command = {
                let mut command = Command::new("/usr/bin/systemd-inhibit");
                command.args([
                    "--what=idle:sleep",
                    "--mode=block",
                    "--who=WebCodex",
                    "--why=active-continuation",
                    "/bin/sleep",
                    INHIBIT_SECONDS,
                ]);
                command
            };
            #[cfg(not(target_os = "linux"))]
            let mut command = {
                let mut command = Command::new("/usr/bin/caffeinate");
                command.args(["-i", "-t", INHIBIT_SECONDS]);
                command
            };
            command
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null());
            let child = webcodex_process::ManagedChild::spawn(&mut command)
                .map_err(|_| "continuation_power_unavailable")?;
            if let Some((mut previous, _)) = self.child.replace((child, Instant::now())) {
                previous
                    .terminate_tree()
                    .map_err(|_| "continuation_power_release_failed")?;
            }
            Ok(())
        }
        #[cfg(not(any(unix, windows)))]
        {
            if active {
                Err("continuation_platform_unsupported")
            } else {
                Ok(())
            }
        }
    }
}

impl Drop for PowerLease {
    /// 释放仅归本对象持有的电源资源；销毁不能返回错误，因此只记录固定错误码。
    fn drop(&mut self) {
        if let Err(code) = self.update(false) {
            tracing::error!(code, "local continuation power cleanup failed");
        }
    }
}

/// 从本机系统进程表证明配置的浏览器已完全退出；任何不完整枚举均返回 false，不尝试启动。
/// # 参数
/// - `path`：已经过本机配置校验的 Chrome/Edge 路径；不用网页提供 PID 或进程名。
pub(super) fn browser_absent(path: &Path) -> bool {
    #[cfg(windows)]
    {
        use std::os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle};
        use windows_sys::Win32::{
            Foundation::{GetLastError, ERROR_NO_MORE_FILES, INVALID_HANDLE_VALUE},
            System::Diagnostics::ToolHelp::{
                CreateToolhelp32Snapshot, Process32FirstW, Process32NextW, PROCESSENTRY32W,
                TH32CS_SNAPPROCESS,
            },
        };
        let Some(name) = path.file_name().and_then(|name| name.to_str()) else {
            return false;
        };
        // SAFETY: 创建只读进程快照，不访问外部内存；有效句柄立即交给 OwnedHandle 唯一管理。
        let raw = unsafe { CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0) };
        if raw == INVALID_HANDLE_VALUE || raw.is_null() {
            return false;
        }
        // SAFETY: raw 是刚创建且尚未移交或关闭的有效内核句柄，Drop 只关闭一次。
        let snapshot = unsafe { OwnedHandle::from_raw_handle(raw) };
        let mut entry = PROCESSENTRY32W {
            dwSize: std::mem::size_of::<PROCESSENTRY32W>() as u32,
            ..Default::default()
        };
        // SAFETY: entry 为正确大小的初始化结构，快照在整个枚举期间存活，API 只写入本结构。
        if unsafe { Process32FirstW(snapshot.as_raw_handle(), &mut entry) } == 0 {
            return false;
        }
        for _ in 0..MAX_PROCESSES {
            let end = entry
                .szExeFile
                .iter()
                .position(|value| *value == 0)
                .unwrap_or(entry.szExeFile.len());
            let current = String::from_utf16_lossy(&entry.szExeFile[..end]);
            // 任一同名浏览器进程（含其他 profile 或渲染器）仍在时不重开，避免重复用户窗口。
            if current.eq_ignore_ascii_case(name) {
                return false;
            }
            // SAFETY: 同上，仍持有有效快照及本地结构，调用失败后立即读取线程本地错误码。
            if unsafe { Process32NextW(snapshot.as_raw_handle(), &mut entry) } == 0 {
                // SAFETY: GetLastError 无参数，读取刚结束的本线程枚举状态，不涉及外部资源。
                return unsafe { GetLastError() } == ERROR_NO_MORE_FILES;
            }
        }
        false
    }
    #[cfg(target_os = "linux")]
    {
        let Some(name) = path.file_name().and_then(|name| name.to_str()) else {
            return false;
        };
        let edge = name.contains("edge");
        let Ok(processes) = std::fs::read_dir("/proc") else {
            return false;
        };
        let mut count = 0;
        for process in processes {
            let Ok(process) = process else {
                return false;
            };
            if !process
                .file_name()
                .as_encoded_bytes()
                .iter()
                .all(u8::is_ascii_digit)
            {
                continue;
            }
            count += 1;
            if count > MAX_PROCESSES {
                return false;
            }
            let name = match std::fs::read_to_string(process.path().join("comm")) {
                Ok(name) => name,
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue, // 进程在枚举期间正常退出。
                Err(_) => return false, // 无权限或不完整读取不能作为退出证明。
            };
            let name = name.trim();
            let matches = if edge {
                ["msedge", "microsoft-edge"].contains(&name)
            } else {
                ["chrome", "chromium", "chromium-browse", "google-chrome"].contains(&name)
            };
            if matches {
                return false;
            }
        }
        true
    }
    #[cfg(not(any(windows, target_os = "linux")))]
    {
        let _ = path;
        false
    } // 未接入系统进程证明的平台不自动启动浏览器。
}

/// 将新用户浏览器与 Runner 的控制台及进程组分离；Windows 无法脱离父 Job 时让启动失败，不降级接管。
/// # 参数
/// - `command`：仅含本机配置浏览器和固定会话 URL 的待启动命令，不通过 Shell。
pub(super) fn detach_browser(command: &mut std::process::Command) {
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        use windows_sys::Win32::System::Threading::{
            CREATE_BREAKAWAY_FROM_JOB, CREATE_NEW_PROCESS_GROUP, DETACHED_PROCESS,
        };
        command.creation_flags(
            CREATE_BREAKAWAY_FROM_JOB | CREATE_NEW_PROCESS_GROUP | DETACHED_PROCESS,
        );
    }
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        command.process_group(0);
    }
    #[cfg(not(any(unix, windows)))]
    let _ = command;
}

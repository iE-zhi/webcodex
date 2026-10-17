//! 本机适配器的边界回归；不启动个人浏览器，不访问生产 Server、配置或凭据。
use super::*;
use std::io::{Read, Write};

/// 返回最小独立 Runner 配置，令牌是固定无效文本，HTTP 只访问本测试监听器。
/// # 参数
/// - `address`：测试独占的回环监听地址，不接受公网地址。
fn config(address: std::net::SocketAddr) -> RunnerConfig {
    toml::from_str(&format!("server_url = 'http://{address}'\nclient_id = 'continuation-test'\ntoken = 'invalid-test-token'\n")).unwrap()
}

/// 请求唯一 nonce 与进程身份；本测试不需要任何注册中的真实 Runner。
fn request() -> LocalLeaseRequest {
    LocalLeaseRequest {
        status: LocalStatus::Ready,
        client_id: "continuation-test".into(),
        agent_instance_id: uuid::Uuid::new_v4().to_string(),
        request_id: uuid::Uuid::new_v4().to_string(),
        browser_absent: false,
    }
}

/// 一次性 HTTP fake：断言认证头和路径，返回指定状态/正文；五秒读取上限，线程随本次测试 join。
/// # 参数
/// - `status`：固定 HTTP 状态行，不包含 CR/LF。
/// - `body`：测试拥有的响应正文，按真实 Content-Length 交给生产有界读取器。
fn server(status: &'static str, body: String) -> (RunnerConfig, JoinHandle<()>) {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let cfg = config(listener.local_addr().unwrap());
    // 无外部输入；请求由生产序列化器生成，正文规模由本测试确定。
    let thread = std::thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        stream.set_read_timeout(Some(HTTP_TIMEOUT)).unwrap();
        let mut input = Vec::new();
        let mut chunk = [0; 4096];
        loop {
            let count = stream.read(&mut chunk).unwrap();
            assert!(count > 0 && input.len() < MAX_RESPONSE);
            input.extend_from_slice(&chunk[..count]);
            if let Some(end) = input.windows(4).position(|part| part == b"\r\n\r\n") {
                let header = String::from_utf8_lossy(&input[..end]).to_ascii_lowercase();
                let length: usize = header
                    .lines()
                    .find_map(|line| line.strip_prefix("content-length:"))
                    .unwrap()
                    .trim()
                    .parse()
                    .unwrap();
                if input.len() >= end + 4 + length {
                    assert!(header.starts_with("post /api/shell/agent/continuation http/1.1"));
                    assert!(header.contains("authorization: bearer invalid-test-token"));
                    break;
                }
            }
        }
        write!(stream, "HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len()).unwrap();
    });
    (cfg, thread)
}

/// 唤醒 URL 只允许规范 HTTPS 聊天路由；拒绝重定向站点、参数、空段和编码路径等边界。
#[test]
fn canonical_chat_urls_only() {
    let id = uuid::Uuid::new_v4();
    for origin in ["https://chatgpt.com", "https://chat.openai.com"] {
        assert!(valid_chat_url(&format!("{origin}/c/{id}")));
        assert!(valid_chat_url(&format!("{origin}/g/g-example/c/{id}")));
    }
    for raw in [
        format!("http://chatgpt.com/c/{id}"),
        format!("https://chatgpt.com.evil/c/{id}"),
        format!("https://name@chatgpt.com/c/{id}"),
        format!("https://chatgpt.com/c/{id}?q=x"),
        format!("https://chatgpt.com/c/{id}#x"),
        format!("https://chatgpt.com//c/{id}"),
        format!("https://chatgpt.com/c/{id}/"),
        "https://chatgpt.com/c/not-a-uuid".into(),
    ] {
        assert!(!valid_chat_url(&raw), "{raw}");
    }
}

/// 真实 HTTP 读取须验证 nonce、TTL、最大动作数、URL 和大小；任何一项失败整笔拒绝，不启动进程。
#[test]
fn lease_protocol_rejects_replay_oversize_and_untrusted_targets() {
    let request = request();
    let valid = serde_json::json!({"request_id":request.request_id,"valid_for_ms":LOCAL_LEASE_MS,"keep_awake":true,"wake":[]});
    let mut cases = vec![(valid.clone(), true)];
    for (field, value) in [
        (
            "request_id",
            serde_json::json!(uuid::Uuid::new_v4().to_string()),
        ),
        ("valid_for_ms", serde_json::json!(LOCAL_LEASE_MS + 1)),
        (
            "wake",
            serde_json::json!([{"task_id":uuid::Uuid::new_v4().to_string(),"generation":1,"url":"https://example.com/"}]),
        ),
    ] {
        let mut invalid = valid.clone();
        invalid[field] = value;
        cases.push((invalid, false));
    }
    let mut oversized = valid.clone();
    oversized["extra"] = serde_json::json!("x".repeat(MAX_RESPONSE));
    cases.push((oversized, false));
    let client = reqwest::blocking::Client::builder()
        .timeout(HTTP_TIMEOUT)
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .unwrap();
    for (body, expected) in cases {
        let (cfg, thread) = server("200 OK", body.to_string());
        assert_eq!(request_lease(&client, &cfg, &request).is_ok(), expected);
        thread.join().unwrap();
    }
    let (cfg, thread) = server("302 Found", valid.to_string());
    assert_eq!(
        request_lease(&client, &cfg, &request).unwrap_err(),
        "continuation_lease_rejected"
    );
    thread.join().unwrap();
}

/// 默认关闭时无需浏览器，启用后拒绝相对/非浏览器路径及可注入 profile；不执行配置文件。
#[test]
fn local_configuration_is_opt_in_and_rejects_unsafe_profile() {
    assert!(ContinuationConfig::default().validate().is_ok());
    assert!(ContinuationConfig {
        enabled: true,
        browser_path: Some("chrome.exe".into()),
        browser_profile: Some("Default".into())
    }
    .validate()
    .is_err());
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join(if cfg!(windows) {
        "chrome.exe"
    } else {
        "chrome"
    });
    std::fs::write(&path, []).unwrap();
    for profile in ["../Default", "--user-data-dir=x", "", ".", "a/b"] {
        let cfg = ContinuationConfig {
            enabled: true,
            browser_path: Some(path.clone()),
            browser_profile: Some(profile.into()),
        };
        assert!(cfg.validate().is_err());
    }
    if cfg!(any(windows, target_os = "linux")) {
        assert!(ContinuationConfig {
            enabled: true,
            browser_path: Some(path),
            browser_profile: Some("Profile 1".into())
        }
        .validate()
        .is_ok());
    }
}

/// Windows 上仅在当前测试线程建立并释放真实电源请求；不修改系统方案或操作个人浏览器。
#[cfg(windows)]
#[test]
fn windows_power_request_is_released_on_owning_thread() {
    let mut power = platform::PowerLease::default();
    power.update(true).unwrap();
    power.update(false).unwrap();
    power.update(false).unwrap();
}

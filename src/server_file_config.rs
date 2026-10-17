//! Declarative Server boot contract. Parses TOML before async workers start.
//! Existing environment-based deployments remain supported when no TOML exists.
use serde::Deserialize;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct FileConfig {
    server: ServerSection,
    #[serde(default)]
    tunnel: Option<TunnelSection>,
    #[serde(default)]
    runners: Option<RunnersSection>,
    #[serde(default)]
    options: BTreeMap<String, toml::Value>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ServerSection {
    #[serde(default = "default_bind")]
    addr: String,
    #[serde(default = "default_data")]
    data_dir: PathBuf,
    token_file: Option<PathBuf>,
    public_url: Option<String>,
    #[serde(default)]
    allow_anonymous_loopback: bool,
}

fn default_bind() -> String {
    "127.0.0.1:8080".into()
}
fn default_data() -> PathBuf {
    PathBuf::from("./data")
}

/// 官方隧道的启动配置；旧 Environment 与直接绑定互斥，凭据仅引用私有文件，不进入健康输出。
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct TunnelSection {
    /// 旧 Environment 根目录；存在时不能同时填写直接绑定字段。
    environment_dir: Option<PathBuf>,
    /// 直接绑定的本地标识；缺省 main，只接受有界 ASCII 名称。
    profile_id: Option<String>,
    /// 官方控制面的固定 tunnel_ 身份，不能用于选择本地 MCP 账户。
    tunnel_id: Option<String>,
    /// 官方控制面密钥的受保护绝对路径，与本地 MCP 凭据分开。
    api_key_file: Option<PathBuf>,
    /// 本地 MCP 账户 PAT 的私有绝对路径；缺省沿用 Server token，但原生续跑要求账户 PAT。
    local_token_file: Option<PathBuf>,
    /// 默认随 Server 启动隧道；false 仅保留配置。
    #[serde(default = "enabled_by_default")]
    autostart: bool,
    /// 直接位于 Server 数据目录的健康文件；缺省由 profile_id 生成。
    readiness_path: Option<PathBuf>,
    /// 可选受保护代理地址，只用于控制面网络，不写入状态或日志。
    proxy: Option<String>,
}
fn enabled_by_default() -> bool {
    true
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct RunnersSection {
    bindings_file: PathBuf,
}

fn read_document(path: &Path) -> Result<FileConfig, String> {
    let meta = std::fs::symlink_metadata(path)
        .map_err(|_| "Server TOML config is unreadable".to_string())?;
    if !meta.is_file() || meta.file_type().is_symlink() || meta.len() > 128 * 1024 {
        return Err("Server TOML config must be a bounded regular non-symlink file".into());
    }
    let source = std::fs::read_to_string(path)
        .map_err(|_| "Server TOML config is unreadable".to_string())?;
    toml::from_str(&source)
        .map_err(|_| "Server TOML config has invalid fields or types".to_string())
}

/// Read a separate owner-private secret. No secret bytes are returned in errors.
pub(crate) fn read_private_secret(path: &Path) -> Result<String, String> {
    if !path.is_absolute()
        || path
            .components()
            .any(|component| matches!(component, std::path::Component::ParentDir))
    {
        return Err("secret path must be absolute without parent traversal".into());
    }
    let metadata =
        std::fs::symlink_metadata(path).map_err(|_| "secret file cannot be opened".to_string())?;
    if !metadata.is_file() || metadata.file_type().is_symlink() || metadata.len() > 4096 {
        return Err("secret must be a bounded, regular, non-symlink file".into());
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::{MetadataExt, PermissionsExt};
        if metadata.permissions().mode() & 0o077 != 0 || metadata.nlink() != 1 {
            return Err("secret file must have owner-only permissions".into());
        }
    }
    let source =
        std::fs::read_to_string(path).map_err(|_| "secret file cannot be read".to_string())?;
    let token = source.trim();
    if token.len() < 32 || token.len() > 2048 || token.chars().any(char::is_control) {
        return Err("secret must be a nonempty bounded bearer credential".into());
    }
    Ok(token.to_string())
}

fn valid_path(path: &Path) -> bool {
    path.is_absolute()
        && !path
            .components()
            .any(|part| matches!(part, std::path::Component::ParentDir))
}

/// 校验启动身份、路径和传输设置；不修改环境或绑定网络。
/// # 参数
/// - `config`：已解析的 Server TOML，只借用并读取其引用的受保护凭据文件。
/// # Errors
/// 不完整、越界或不安全的配置返回固定说明，失败不启用任何配置。
fn validate(config: &FileConfig) -> Result<(), String> {
    let addr = config
        .server
        .addr
        .parse::<std::net::SocketAddr>()
        .map_err(|_| "server.addr must be an IP:port socket address".to_string())?;
    if addr.port() == 0 {
        return Err("server.addr port cannot be zero".into());
    }
    if config.server.allow_anonymous_loopback && !addr.ip().is_loopback() {
        return Err("server.allow_anonymous_loopback requires a loopback address".into());
    }
    if config.server.token_file.is_none() && !config.server.allow_anonymous_loopback {
        return Err(
            "Server TOML requires server.token_file or explicit loopback anonymous opt-in".into(),
        );
    }
    if let Some(path) = &config.server.token_file {
        read_private_secret(path)?;
    }
    if let Some(url) = &config.server.public_url {
        let url =
            url::Url::parse(url).map_err(|_| "server.public_url is not an URL".to_string())?;
        if !matches!(url.scheme(), "https" | "http")
            || url.host_str().is_none()
            || url.username() != ""
            || url.password().is_some()
            || url.query().is_some()
            || url.fragment().is_some()
            || !matches!(url.path(), "" | "/")
        {
            return Err("server.public_url must be a credential-free HTTP(S) origin".into());
        }
    }
    if let Some(runners) = &config.runners {
        if !valid_path(&runners.bindings_file) {
            return Err("runners.bindings_file must be absolute without traversal".into());
        }
    }
    if let Some(tunnel) = &config.tunnel {
        if let Some(environment) = &tunnel.environment_dir {
            if tunnel.tunnel_id.is_some()
                || tunnel.api_key_file.is_some()
                || tunnel.local_token_file.is_some()
                || tunnel.profile_id.is_some()
                || tunnel.proxy.is_some()
                || tunnel.readiness_path.is_some()
            {
                return Err(
                    "tunnel.environment_dir cannot be mixed with a direct Tunnel binding".into(),
                );
            }
            if !valid_path(environment) {
                return Err("tunnel.environment_dir must be absolute".into());
            }
        } else {
            if !valid_path(&config.server.data_dir) {
                return Err("direct Tunnel requires absolute server.data_dir".into());
            }
            if config.server.token_file.is_none() {
                return Err("direct Tunnel requires server.token_file".into());
            }
            if !addr.ip().is_unspecified() && !addr.ip().is_loopback() {
                return Err("direct Tunnel requires a loopback-reachable Server bind".into());
            }
            let id = tunnel
                .tunnel_id
                .as_deref()
                .ok_or("tunnel.tunnel_id is required")?;
            if !id.starts_with("tunnel_")
                || id.len() != 39
                || !id[7..]
                    .bytes()
                    .all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase())
            {
                return Err("tunnel.tunnel_id must be a valid fixed tunnel_ identity".into());
            }
            let api_file = tunnel
                .api_key_file
                .as_deref()
                .ok_or("tunnel.api_key_file is required")?;
            read_private_secret(api_file)?;
            if let Some(path) = &tunnel.local_token_file {
                read_private_secret(path)?;
            }
            if let Some(path) = &tunnel.readiness_path {
                if !valid_path(path) || path.parent() != Some(config.server.data_dir.as_path()) {
                    return Err(
                        "tunnel.readiness_path must be directly inside absolute server.data_dir"
                            .into(),
                    );
                }
            }
            if let Some(proxy) = &tunnel.proxy {
                let parsed = url::Url::parse(proxy)
                    .map_err(|_| "tunnel.proxy has invalid URL".to_string())?;
                if !matches!(parsed.scheme(), "http" | "https")
                    || parsed.host_str().is_none()
                    || parsed.username() != ""
                    || parsed.password().is_some()
                {
                    return Err("tunnel.proxy must be an uncredentialed HTTP(S) proxy URL".into());
                }
            }
        }
    }
    // 这些身份和传输选项只能由显式 TOML 字段设置，不能通过通用 options 覆盖。
    const RESERVED: &[&str] = &[
        "WEBCODEX_SERVER_CONFIG",
        "WEBCODEX_ENV_FILE",
        "WEBCODEX_ADDR",
        "WEBCODEX_DATA",
        "WEBCODEX_TOKEN",
        "WEBCODEX_TUNNEL_ENVIRONMENT",
        "WEBCODEX_RUNNER_BINDINGS_FILE",
        "WEBCODEX_PUBLIC_URL",
    ];
    for (key, value) in &config.options {
        if !key.starts_with("WEBCODEX_")
            || key.len() > 110
            || !key
                .chars()
                .all(|c| c.is_ascii_uppercase() || c == '_' || c.is_ascii_digit())
            || RESERVED.contains(&key.as_str())
        {
            return Err("options contains an unsupported or reserved environment setting".into());
        }
        if !(value.is_str() || value.is_integer() || value.is_bool()) {
            return Err("options values must be TOML strings, booleans, or integers".into());
        }
    }
    Ok(())
}

pub(crate) fn selected_config_path() -> Result<Option<PathBuf>, String> {
    if let Some(path) = std::env::var_os("WEBCODEX_SERVER_CONFIG") {
        if path.is_empty() {
            return Err("WEBCODEX_SERVER_CONFIG cannot be empty".into());
        }
        return Ok(Some(PathBuf::from(path)));
    }
    let default = PathBuf::from("server.toml");
    if default.exists() {
        Ok(Some(default))
    } else {
        Ok(None)
    }
}

pub(crate) fn validate_selected() -> Result<(), String> {
    if let Some(path) = selected_config_path()? {
        let config = read_document(&path)?;
        validate(&config)?;
        if let Some(runners) = config.runners {
            crate::runner_bindings_file::validate_bindings_file(&runners.bindings_file)?;
        }
        Ok(())
    } else {
        Err("server.toml does not exist; use --config PATH".into())
    }
}

/// 应用显式 TOML 的启动身份和环境选项；身份与传输字段不从其他服务继承。
/// # Errors
/// 配置校验失败时在修改环境前返回错误；没有选定 TOML 则保留旧部署入口。
pub(crate) fn prepare() -> Result<(), String> {
    let Some(path) = selected_config_path()? else {
        return Ok(());
    };
    let config = read_document(&path)?;
    validate(&config)?;

    // Explicit TOML wins over inherited configuration for identity, transport
    // and authorization. In particular, a legacy Runner binding or Tunnel
    // location must not silently survive when omitted from the TOML.
    std::env::set_var("WEBCODEX_ADDR", &config.server.addr);
    std::env::set_var("WEBCODEX_DATA", &config.server.data_dir);
    match &config.server.token_file {
        Some(path) => std::env::set_var("WEBCODEX_TOKEN", read_private_secret(path)?),
        None => std::env::remove_var("WEBCODEX_TOKEN"),
    }
    std::env::set_var(
        "WEBCODEX_ALLOW_ANONYMOUS",
        config.server.allow_anonymous_loopback.to_string(),
    );
    match &config.server.public_url {
        Some(url) => std::env::set_var("WEBCODEX_PUBLIC_URL", url),
        None => std::env::remove_var("WEBCODEX_PUBLIC_URL"),
    }
    match &config.runners {
        Some(value) => std::env::set_var("WEBCODEX_RUNNER_BINDINGS_FILE", &value.bindings_file),
        None => std::env::remove_var("WEBCODEX_RUNNER_BINDINGS_FILE"),
    }
    match config
        .tunnel
        .as_ref()
        .and_then(|value| value.environment_dir.as_ref())
    {
        Some(path) => std::env::set_var("WEBCODEX_TUNNEL_ENVIRONMENT", path),
        None => std::env::remove_var("WEBCODEX_TUNNEL_ENVIRONMENT"),
    }
    for (key, value) in config.options {
        std::env::set_var(
            &key,
            match value {
                toml::Value::String(value) => value,
                value => value.to_string(),
            },
        );
    }
    Ok(())
}

/// 从直接配置建立官方隧道，优先使用独立账户 PAT；缺省沿用 Server 凭据，不修改 Server bootstrap 身份。
/// # Errors
/// 配置或私有凭据无效时拒绝启动；不输出凭据，不建立网络或修改现有运行状态。
pub(crate) fn direct_tunnel_profile(
) -> Result<Option<webcodex_environment::EmbeddedTunnelProfile>, String> {
    let Some(path) = selected_config_path()? else {
        return Ok(None);
    };
    let config = read_document(&path)?;
    validate(&config)?;
    let Some(tunnel) = config.tunnel else {
        return Ok(None);
    };
    if tunnel.environment_dir.is_some() {
        return Ok(None);
    }
    let local = if let Some(path) = tunnel
        .local_token_file
        .as_ref()
        .or(config.server.token_file.as_ref())
    {
        read_private_secret(path)?
    } else {
        return Err("embedded Tunnel requires a Server token_file".into());
    };
    use webcodex_environment::{EmbeddedTunnelProfile, Secret, TunnelCredentials};
    let id = tunnel.tunnel_id.ok_or("tunnel_id is missing")?;
    let profile_id = tunnel.profile_id.unwrap_or_else(|| "main".into());
    if profile_id.is_empty()
        || profile_id.len() > 64
        || !profile_id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
    {
        return Err("tunnel.profile_id has invalid characters".into());
    }
    let readiness_path = tunnel.readiness_path.unwrap_or_else(|| {
        config
            .server
            .data_dir
            .join(format!("{profile_id}-tunnel-readiness.json"))
    });
    Ok(Some(EmbeddedTunnelProfile {
        profile_id,
        autostart: tunnel.autostart,
        runtime_revision: 1,
        credentials: TunnelCredentials {
            tunnel_id: Secret::new(id),
            api_key: Secret::new(read_private_secret(
                tunnel
                    .api_key_file
                    .as_deref()
                    .ok_or("tunnel.api_key_file missing")?,
            )?),
        },
        local_token: Secret::new(local),
        proxy: tunnel.proxy.map(Secret::new),
        readiness_path,
    }))
}

#[cfg(test)]
#[path = "../tests/chat-continuation/tunnel_config.rs"]
mod continuation_tunnel_tests;

/// Materialize the private Tunnel health observation only for the directly
/// configured TOML profile. Never overwrite a saved observation at startup:
/// durable execution ownership remains guarded by the existing run fence.
pub(crate) fn ensure_direct_tunnel_readiness(data_dir: &Path) -> Result<(), String> {
    let Some(profile) = direct_tunnel_profile()? else {
        return Ok(());
    };
    if !valid_path(data_dir) || !valid_path(&profile.readiness_path) {
        return Err("direct Tunnel requires absolute data_dir and readiness_path".into());
    }
    let root = std::fs::symlink_metadata(data_dir)
        .map_err(|_| "Tunnel data directory cannot be inspected".to_string())?;
    if !root.is_dir() || root.file_type().is_symlink() {
        return Err("Tunnel data directory must be a real directory".into());
    }
    let parent = profile
        .readiness_path
        .parent()
        .ok_or("Tunnel readiness path must have a parent")?;
    if parent != data_dir || parent.canonicalize().ok() != data_dir.canonicalize().ok() {
        return Err("Tunnel readiness path must be directly inside Server data_dir".into());
    }
    let path = &profile.readiness_path;
    match std::fs::symlink_metadata(path) {
        Ok(_) => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            let mut options = std::fs::OpenOptions::new();
            options.create_new(true).write(true);
            #[cfg(unix)]
            {
                use std::os::unix::fs::OpenOptionsExt;
                options
                    .mode(0o600)
                    .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC);
            }
            match options.open(path) {
                Ok(file) => file
                    .sync_all()
                    .map_err(|_| "Tunnel readiness file cannot be synced")?,
                Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {}
                Err(_) => return Err("Tunnel readiness file cannot be created".into()),
            }
        }
        Err(_) => return Err("Tunnel readiness file cannot be inspected".into()),
    }
    let metadata = std::fs::symlink_metadata(path)
        .map_err(|_| "Tunnel readiness file cannot be inspected".to_string())?;
    if !metadata.is_file() || metadata.file_type().is_symlink() {
        return Err("Tunnel readiness file must be a regular non-symlink file".into());
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::{MetadataExt, PermissionsExt};
        if metadata.uid() != unsafe { libc::geteuid() }
            || metadata.permissions().mode() & 0o077 != 0
            || metadata.nlink() != 1
        {
            return Err("Tunnel readiness file must be owner-private".into());
        }
    }
    #[cfg(windows)]
    webcodex_environment::runtime_entry::validate_windows_env_acl(path)
        .map_err(|_| "Tunnel readiness file must be owner-private".to_string())?;
    Ok(())
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn rejects_unknown_and_unprotected_configurations() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("server.toml");
        std::fs::write(
            &path,
            "[server]\naddr = '0.0.0.0:8080'\nallow_anonymous_loopback = true\n",
        )
        .unwrap();
        assert!(validate(&read_document(&path).unwrap()).is_err());
        std::fs::write(&path, "[server]\naddr = '127.0.0.1:8080'\nallow_anonymous_loopback = true\nnot_known = true\n").unwrap();
        assert!(read_document(&path).is_err());
        std::fs::write(
            &path,
            "[server]\naddr = '127.0.0.1:8080'\nallow_anonymous_loopback = true\n",
        )
        .unwrap();
        assert!(validate(&read_document(&path).unwrap()).is_ok());
    }

    #[test]
    fn direct_tunnel_toml_creates_private_readiness_once() {
        let mut env = crate::test_support::TestEnvGuard::new();
        let temp = tempfile::tempdir().unwrap();
        let data = temp.path().join("data");
        std::fs::create_dir_all(&data).unwrap();
        let server_token = temp.path().join("server-token");
        let tunnel_key = temp.path().join("tunnel-api-key");
        std::fs::write(&server_token, format!("wc_pat_{}\n", "b".repeat(64))).unwrap();
        std::fs::write(
            &tunnel_key,
            "a-super-secret-api-key-with-at-least-32-bytes\n",
        )
        .unwrap();
        #[cfg(unix)]
        for file in [&server_token, &tunnel_key] {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(file, std::fs::Permissions::from_mode(0o600)).unwrap();
        }
        let file = temp.path().join("server.toml");
        std::fs::write(&file, format!(
            "[server]\naddr = '127.0.0.1:8080'\ndata_dir = '{}'\ntoken_file = '{}'\n[tunnel]\ntunnel_id = 'tunnel_{}'\napi_key_file = '{}'\n",
            data.display(), server_token.display(), "a".repeat(32), tunnel_key.display()
        )).unwrap();
        env.set("WEBCODEX_SERVER_CONFIG", &file);
        assert!(validate_selected().is_ok());
        ensure_direct_tunnel_readiness(&data).unwrap();
        let state = data.join("main-tunnel-readiness.json");
        assert!(state.is_file());
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                std::fs::metadata(&state).unwrap().permissions().mode() & 0o077,
                0
            );
        }
        webcodex_environment::write_embedded_tunnel_observation(
            &state,
            1,
            webcodex_environment::TunnelState::Stopped,
            false,
            false,
            None,
        )
        .unwrap();
        let before = std::fs::read(&state).unwrap();
        ensure_direct_tunnel_readiness(&data).unwrap();
        assert_eq!(std::fs::read(&state).unwrap(), before);
        #[cfg(unix)]
        {
            std::fs::remove_file(&state).unwrap();
            std::os::unix::fs::symlink(&server_token, &state).unwrap();
            assert!(ensure_direct_tunnel_readiness(&data).is_err());
        }
    }

    #[test]
    fn toml_authority_overrides_inherited_identity_and_clears_unlisted_bindings() {
        let mut env = crate::test_support::TestEnvGuard::new();
        let temp = tempfile::tempdir().unwrap();
        let secret = temp.path().join("server-token");
        let config = temp.path().join("server.toml");
        std::fs::write(&secret, format!("wc_pat_{}\n", "a".repeat(64))).unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&secret, std::fs::Permissions::from_mode(0o600)).unwrap();
        }
        std::fs::write(
            &config,
            format!(
                "[server]\naddr = '127.0.0.1:9090'\n token_file = '{}'\n",
                secret.display()
            ),
        )
        .unwrap();
        env.set("WEBCODEX_SERVER_CONFIG", &config);
        env.set("WEBCODEX_ADDR", "0.0.0.0:2020");
        env.set("WEBCODEX_TOKEN", "wrong-inherited-token");
        env.set("WEBCODEX_ALLOW_ANONYMOUS", "true");
        env.set("WEBCODEX_TUNNEL_ENVIRONMENT", "/tmp/legacy-tunnel");
        env.set("WEBCODEX_RUNNER_BINDINGS_FILE", "/tmp/legacy-runners");
        prepare().unwrap();
        assert_eq!(std::env::var("WEBCODEX_ADDR").unwrap(), "127.0.0.1:9090");
        assert_eq!(
            std::env::var("WEBCODEX_TOKEN").unwrap(),
            format!("wc_pat_{}", "a".repeat(64))
        );
        assert_eq!(std::env::var("WEBCODEX_ALLOW_ANONYMOUS").unwrap(), "false");
        assert!(std::env::var_os("WEBCODEX_TUNNEL_ENVIRONMENT").is_none());
        assert!(std::env::var_os("WEBCODEX_RUNNER_BINDINGS_FILE").is_none());
    }

    #[test]
    fn token_file_is_protected() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("secret");
        std::fs::write(&path, format!("wc_pat_{}\n", "a".repeat(64))).unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).unwrap();
            assert!(read_private_secret(&path).is_ok());
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644)).unwrap();
            assert!(read_private_secret(&path).is_err());
        }
    }
}

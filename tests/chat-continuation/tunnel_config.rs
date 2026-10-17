//! 官方隧道账户凭据的独立配置回归；不连接控制面，不创建真实账户或保存有效凭据。
use super::*;

/// 三个互不相同的无效测试凭据，分别模拟 Server bootstrap、控制面密钥和 MCP 账户 PAT。
const SERVER: &str = "wc_pat_bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";
/// 控制面假密钥，只用于验证读取来源，不发送到网络。
const CONTROL: &str = "test-control-plane-key-not-a-real-key-000000";
/// MCP 账户假 PAT，应只进入 EmbeddedTunnelProfile.local_token，不能替换 Server bootstrap。
const ACCOUNT: &str = "wc_pat_aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";

/// 创建当前测试独占、只读私有假密钥文件。
/// # 参数
/// - `root`：本测试临时目录，结束时统一释放。
/// - `name`：固定文件名，不接受路径分隔符或外部输入。
/// - `text`：不含真实秘密的测试文本。
fn secret(root: &Path, name: &str, text: &str) -> PathBuf {
    let path = root.join(name);
    std::fs::write(&path, text).unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).unwrap();
    }
    path
}

/// 独立 PAT 与 bootstrap 分开读取；未显式配置时保留旧绑定，且 prepare 不把账户 PAT 设为 bootstrap。
#[test]
fn direct_tunnel_account_pat_is_separate_and_legacy_fallback_is_preserved() {
    let mut env = crate::test_support::TestEnvGuard::new();
    let directory = tempfile::tempdir().unwrap();
    let server = secret(directory.path(), "server", SERVER);
    let control = secret(directory.path(), "control", CONTROL);
    let account = secret(directory.path(), "account", ACCOUNT);
    let config = directory.path().join("server.toml");
    let base = format!("[server]\naddr='127.0.0.1:8080'\ndata_dir='{}'\ntoken_file='{}'\n[tunnel]\ntunnel_id='tunnel_{}'\napi_key_file='{}'\nautostart=false\n", directory.path().display(), server.display(), "a".repeat(32), control.display());
    std::fs::write(&config, &base).unwrap();
    env.set("WEBCODEX_SERVER_CONFIG", &config);
    assert_eq!(
        direct_tunnel_profile()
            .unwrap()
            .unwrap()
            .local_token
            .expose(),
        SERVER
    );
    std::fs::write(
        &config,
        format!("{base}local_token_file='{}'\n", account.display()),
    )
    .unwrap();
    let profile = direct_tunnel_profile().unwrap().unwrap();
    assert_eq!(profile.local_token.expose(), ACCOUNT);
    assert_eq!(profile.credentials.api_key.expose(), CONTROL);
    prepare().unwrap();
    assert_eq!(std::env::var("WEBCODEX_TOKEN").unwrap(), SERVER);
}

/// 相对路径、缺失文件以及与旧 Environment 混用均被拒绝，不能悄悄回退到 bootstrap。
#[test]
fn invalid_explicit_mcp_credential_is_rejected_without_fallback() {
    let directory = tempfile::tempdir().unwrap();
    let server = secret(directory.path(), "server", SERVER);
    let control = secret(directory.path(), "control", CONTROL);
    let base = format!("[server]\naddr='127.0.0.1:8080'\ndata_dir='{}'\ntoken_file='{}'\n[tunnel]\ntunnel_id='tunnel_{}'\napi_key_file='{}'\n", directory.path().display(), server.display(), "a".repeat(32), control.display());
    for path in [
        "relative-pat".into(),
        directory.path().join("missing").display().to_string(),
    ] {
        let cfg: FileConfig =
            toml::from_str(&format!("{base}local_token_file='{path}'\n")).unwrap();
        assert!(validate(&cfg).is_err());
    }
    let cfg: FileConfig = toml::from_str(&format!(
        "[server]\ntoken_file='{}'\n[tunnel]\nenvironment_dir='{}'\nlocal_token_file='{}'\n",
        server.display(),
        directory.path().display(),
        server.display()
    ))
    .unwrap();
    assert!(validate(&cfg).is_err());
}

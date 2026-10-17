# WebCodex：纯 TOML、两进程部署

此版本只有 **webcodex-server** 与 **webcodex-runner** 两个可执行文件，不需要 `webcodex` CLI、Desktop、Console 或第三个常驻服务。配置文件统一使用 TOML。

## 文件布局

Server 主机：
- `/etc/webcodex/server.toml` — Server 地址、数据目录、嵌入式 Tunnel 和配置引用
- `/etc/webcodex/runner-bindings.toml` — Runner ID 和 SHA-256 token 哈希（**权限 0600**）
- `/etc/webcodex/secrets/server-token` — Server bootstrap 凭据（**权限 0600**）
- `/etc/webcodex/secrets/openai-control-plane-key` — OpenAI Tunnel 管理凭据（**权限 0600**，仅启用 Tunnel 时使用）

Runner 主机：
- `/etc/webcodex/runner.toml` — Server URL、Runner ID、允许的工作目录、Computer/ACP 等设置
- `/etc/webcodex/secrets/runner-token` — Runner 独立 token（**权限 0600**）
- `/etc/webcodex/projects/my-project.toml` — 项目的绝对路径和允许修改规则

模板位于 `config/server.example.toml`、`config/runner.example.toml`、`config/runner-bindings.example.toml`、`config/project.example.toml`。

普通 TOML 不应包含任何明文 token。Runner 使用 `token_file`，Server 使用 `server.token_file` 和 `tunnel.api_key_file`。只有受保护文件保存凭据正文。

## 编译与验证

```bash
cargo build --locked --release --bins -p webcodex -p webcodex-runner
target/release/webcodex-server --config /etc/webcodex/server.toml --check-config
target/release/webcodex-runner --config /etc/webcodex/runner.toml --check-config
```

编译产物只有 `webcodex-server` 和 `webcodex-runner`（Windows 加 `.exe`）。Server 也允许在当前目录读取默认 `server.toml`；Runner 默认查找已有的 `runner.toml`，也可设置 `WEBCODEX_RUNNER_CONFIG`。以前的 `webcodex.env` 启动方式仍作为旧安装兼容入口保留。

## Server 首次引导凭据

在 **Server 主机**上生成独立的随机 bootstrap 凭据，不需要 CLI。先创建 `/etc/webcodex/secrets` 并确保目录仅允许 Server 服务账户访问，再执行：

```bash
umask 077
python3 - <<'PY'
from pathlib import Path
import secrets
path = Path("/etc/webcodex/secrets/server-token")
path.write_text(secrets.token_hex(32) + "\n")
path.chmod(0o600)
PY
```

将该文件的绝对路径填写到 `server.toml` 的 `[server].token_file`。Server 的凭据与 Runner 的 `wc_agent_...` token **相互独立**，不要复用。
## 不用 CLI 配对跨机 Runner

1. 在 Runner 主机生成 **64 位十六进制随机串**，构成 `wc_agent_<64hex>` token，放入权限 0600 的独立文件。不要把 token 写进 TOML 或命令行历史。
2. 用 SHA-256 计算完整 token 的哈希；在 Server 的 `runner-bindings.toml` 填写相同的 `username`、`client_id`、`token_hash` 和 token 前 16 字符 `token_prefix`。只向 Server 传递哈希，不传递明文 Runner token。
3. Runner 的 `runner.toml` 中 `owner` 与 `username` 一致，`client_id` 与绑定记录一致，`server_url` 指向 Server 实际可达的 HTTPS origin。
4. 启动 Server。它在数据库打开后只创建明确列出的 Agent 身份，并限制为 `agent:register`、`agent:poll`、`agent:result`、`agent:job_update`，不会生成管理员或隐式扩大权限。已撤销或更换的 token 不会因重启而被悄悄恢复。
5. 在 Runner 主机启动 Runner。已创建的 `projects/*.toml` 会被 Runner 加载；项目路径必须落在 `policy.allowed_roots` 授权范围内。

一个安全的随机 token 生成示例（操作前先创建只对服务账户可读写的 secrets 目录）：

```bash
umask 077
python3 - <<'PY'
from pathlib import Path
import secrets
p = Path("/etc/webcodex/secrets/runner-token")
p.write_text("wc_agent_" + secrets.token_hex(32) + "\n")
p.chmod(0o600)
PY
```

生成给 Server 的哈希/前缀（不会打印 token 正文）：

```bash
python3 - <<'PY'
from pathlib import Path
from hashlib import sha256
token = Path("/etc/webcodex/secrets/runner-token").read_text().strip()
print("token_hash =", sha256(token.encode("utf-8")).hexdigest())
print("token_prefix =", token[:16])
PY
```

配置复制到对应主机后，分别前台启动：

```bash
webcodex-server --config /etc/webcodex/server.toml
webcodex-runner --config /etc/webcodex/runner.toml
```

常驻服务可直接由 systemd、launchd 或 Windows Service/Task Scheduler 启动对应二进制并传入 `--config`，无需安装额外管理程序。

## 内嵌 OpenAI Tunnel

将 `[tunnel]` 配置写在 `server.toml`，其中提供 OpenAI 已分配的 `tunnel_id` 和受保护的 `api_key_file`。Server 内部管理 Tunnel，不会启动第三个 WebCodex 守护进程。若不需要 Tunnel，删除整个 `[tunnel]` 表即可。

注意：**OpenAI Tunnel 只负责 ChatGPT → Server**。远端 Runner 通过自己的 `server_url` 直接访问 Server，通常应经过 HTTPS 反向代理。若 Server 绑定 `127.0.0.1`，反向代理需与其位于同一主机。Computer Use 在 Runner 本机执行，不经 Tunnel 直接转发鼠标键盘指令。

## 重要安全边界

- 配置哈希文件及明文 token 文件都需要系统账户独占访问（Unix 模式 0600）。不要将凭据存进 Git。
- 不要为了省配置而在公网开放匿名 Server。没有 Server token 时只能显式启用 `allow_anonymous_loopback = true` 并绑定 loopback。
- 首次增加/替换 Runner token 是明确的配置变更；Server 不允许静默替换活跃绑定或复活撤销的凭据。
- 已运行的服务不支持通过改文件自动热重载所有身份/传输设置；修改后按自己的服务管理器显式重启。旧会话清理和不确定执行恢复仍受原有安全机制约束。
- Server 保留受认证的管理 API（包括凭据管理），不需要 CLI。静态凭据配置不替代 OAuth/ChatGPT 客户端权限配置。

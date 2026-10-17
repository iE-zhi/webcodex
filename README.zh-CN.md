# WebCodex

无界面的 ChatGPT MCP 后端，采用 TOML 配置，**仅运行两个独立的可执行文件**。

- **`webcodex-server`**：MCP、认证、任务调度、内嵌 OpenAI Tunnel。
- **`webcodex-runner`**：本地/远端项目、文件、Git、命令、Browser 和 Computer Use。

不再提供独立 `webcodex` CLI、Desktop、Web Console 或纯展示类 MCP Apps。Computer 截图以及 Agent/Job 自动续跑所需的最小 MCP Host 载体仍然保留。

## 构建

```bash
cargo build --locked --release --bins -p webcodex -p webcodex-runner
```

发布归档只包含 `webcodex-server` 和 `webcodex-runner`。Server/Runner 在同机或跨机部署均可。

## 配置与启动

所有运行配置统一使用 TOML，凭据保存在各主机权限受限的独立文件中：

```bash
webcodex-server --config /etc/webcodex/server.toml --check-config
webcodex-runner --config /etc/webcodex/runner.toml --check-config

webcodex-server --config /etc/webcodex/server.toml
webcodex-runner --config /etc/webcodex/runner.toml
```

使用 `runner-bindings.toml` 在 Server 预登记 Runner token 哈希，使用 Runner 本地 `projects/*.toml` 注册项目，不需要 CLI 配对或注册命令。见 **[TOML 部署与凭据准备指南](docs/HEADLESS_TOML.zh-CN.md)** 及 `config/*.example.toml` 模板。

Python 发布验证需 Python 3.11+。历史 `webcodex.env` 服务环境兼容入口仍保留，新的配置应优先使用 TOML。

仅注册可信项目根目录，不要在普通 TOML 或 Git 中保存明文 token。Runner 以实际操作系统用户权限执行命令。详细安全说明见 [SECURITY.md](SECURITY.md)。

许可证：Apache-2.0。

# WebCodex 长任务续跑

Server 保存 Continue、Goal/Loop 的状态、预算和队列；当前 ChatGPT 判断进度，不调用额外模型 API。暂不提供 Compact & Resume。

## 构建与启动

```bash
cargo build --locked --release --bins -p webcodex -p webcodex-runner
webcodex-server --config /absolute/path/server.toml
```

## 浏览器接入

1. Chrome/Edge 120+ 打开扩展管理页，启用开发者模式，加载本目录。
2. 打开已有 ChatGPT 会话，或在新聊天预置后手动发出第一条请求。
3. 在扩展“后端连接”填写 Server origin 和本账户 PAT，仅授予 `session:collaborate`。可由现有 `POST /api/tokens/create` 账户接口创建；不要填 bootstrap 或 Runner token。HTTP 仅允许本机或 RFC 1918 私网 IPv4 字面地址，且会明文传输令牌；公网使用 HTTPS。扩展只请求该次填写地址的访问权限。
4. Continue 恢复中断；Goal 完成原目标；Loop 持续处理原任务。默认保持启用，正常结束后等待下一条人工请求；关闭“持续启用”则本次结束后停止。修改目标不重置累计轮数和授权期限。

面板显示连接结果；失败保留输入，成功保存 PAT，留空可验证原连接。未提交的连接草稿仅保留至浏览器关闭。

默认最多 100 轮、间隔至少 30 秒、授权 24 小时。当前主聊天按 `update_task_continuation` 返回输出单行来源回执并再次绑定；阶段结束由工具报告 continue/complete/blocked，后续工作由工具结果返回。子任务 Session 必须声明为依赖。工具不可用时使用当前聊天的 `WEBCODEX_GOAL` 标记。所有消息使用当前 ChatGPT 账户额度。

后端保留人工约束、当前公开回复与发送回执用于页面校验，不累计历史自动回合，不收集私有推理或原始工具参数/结果。任务状态在仅服务账户可访问的 Server 数据目录 `continuations.sqlite`；备份/恢复前停止 Server，再处理整个数据目录。

面板支持人工队列、工作流和当前聊天规划，人工队列优先。审批、关联 Session/Job、草稿及结果未知的操作会阻止推进。每回合最多三次刷新/重开；自适应静默窗口随模型和推理状态变化。发送结果未知不自动重发；暂停不能撤回已经点击发送的消息。恢复前核对聊天与草稿。

ChatGPT 通过官方隧道连接同一个 Server，使用同一账户凭据；等待范围仅包含已关联的工作，不等待账户内无关 Job。
在 Server 的 `[tunnel]` 设置 `local_token_file` 指向该账户供 MCP 使用的私有 PAT 文件；保留 `server.token_file` 作为独立 bootstrap。账户 PAT 须具备实际工程操作权限，扩展 PAT 只需 `session:collaborate`。

## 本机 Runner

普通续跑无需更改 Runner。浏览器完全退出后唤醒或系统防休眠，需要登录用户会话中的 Windows/Linux Runner：

```toml
[continuation]
enabled = true
browser_path = "C:\\Program Files\\Google\\Chrome\\Application\\chrome.exe"
browser_profile = "Default"
```

Linux 使用实际安装的 Chrome/Edge 绝对路径，防休眠需要 `/usr/bin/systemd-inhibit`。修改后仅重启 Runner 程序，再在扩展填写该账户的 Runner ID，并按需启用防休眠。profile 必须已登录 ChatGPT 并安装扩展。服务或非交互会话不能保证打开可见用户浏览器；不支持的系统拒绝启用。失联最多四十五秒释放防休眠，唤醒共享三次修复预算。

## 验证

Node.js 24+，仓库根目录：

```bash
cargo test --locked -p webcodex-continuation
cargo test --locked -p webcodex --lib tool_runtime::tests::continuation_runtime
cargo test --locked -p webcodex-runner continuation::tests
cargo build --locked -p webcodex --bin webcodex-server
node --test tests/chat-continuation/*.test.mjs
```

测试使用独立临时数据及浏览器 profile；真实 ChatGPT、官方隧道及本机电源/唤醒须在对应部署环境验收。

页面适配参考 [上游项目](https://github.com/totec448-spec/chat-on-steroids/tree/6090e7b0f0a422df23f45ffd53b128c68863d6c6)，许可见 `THIRD_PARTY_LICENSE.txt`。

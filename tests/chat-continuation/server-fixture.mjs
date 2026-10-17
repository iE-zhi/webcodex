/** 真实 Server 的隔离测试夹具：临时数据库和最小权限 PAT；不启动模型服务，结束后停止该进程。 */
import { spawn } from 'node:child_process';
import { access, mkdtemp, mkdir, writeFile, rm } from 'node:fs/promises';
import path from 'node:path';
import { tmpdir } from 'node:os';
import { createServer as tcpServer } from 'node:net';
import { once } from 'node:events';
import { randomBytes } from 'node:crypto';

/** 测试服务启动最多等待 20 秒；每个 HTTP 请求最多五秒。 */
const START_TIMEOUT_MS = 20000, HTTP_TIMEOUT_MS = 5000;
/** 测试 Server 正常停止及强制回收各最多五秒，避免清理无限等待。 */
const STOP_TIMEOUT_MS = 5000;

/** 取得当前未占用的 loopback 端口；服务只对本机开放。 */
async function port() {
  const listener = tcpServer(); listener.listen(0, '127.0.0.1'); await once(listener, 'listening');
  const number = listener.address().port;
  await new Promise(resolve => listener.close(resolve)); return number;
}

/** 启动测试实例；context 用于明确报告缺失二进制的跳过，不假报运行通过。 */
export async function serverFixture(context) {
  const binary = process.env.WEBCODEX_SERVER_BINARY || path.resolve('target/debug/webcodex-server' + (process.platform === 'win32' ? '.exe' : ''));
  try { await access(binary); } catch { context.skip('需先构建 webcodex-server；真实 HTTP 验证未执行'); return null; }
  const directory = await mkdtemp(path.join(tmpdir(), 'webcodex-continuation-server-'));
  const data = path.join(directory, 'data'); await mkdir(data, { mode: 0o700 });
  const bootstrap = 'wc_pat_' + randomBytes(32).toString('hex');
  const serverKeyPath = path.join(directory, 'bootstrap');
  await writeFile(serverKeyPath, bootstrap, { mode: 0o600 });
  const fixture = {};
  const serverPort = await port(), origin = `http://127.0.0.1:${serverPort}`;
  const config = path.join(directory, 'server.toml');
  await writeFile(config, `[server]\naddr = "127.0.0.1:${serverPort}"\ndata_dir = ${JSON.stringify(data)}\ntoken_file = ${JSON.stringify(serverKeyPath)}\n`);
  // 仅传本测试 TOML，不继承个人 WebCodex 部署、Tunnel 或项目凭据。
  const environment = Object.fromEntries(Object.entries(process.env).filter(([key]) => !key.startsWith('WEBCODEX_')));
  let processHandle = null, output = '';
  /** 停止本测试持有的唯一 Server 进程；没有 Runner 或其他服务被启动。 */
  async function stop() {
    if (!processHandle || processHandle.exitCode !== null) return;
    const process = processHandle;
    // 先订阅退出再发送停止信号，进程立即退出也不会漏掉清理事件。
    const stopped = once(process, 'exit', { signal: AbortSignal.timeout(STOP_TIMEOUT_MS) });
    process.kill();
    try { await stopped; }
    catch (error) {
      if (error.name !== 'AbortError') throw error;
      const forced = once(process, 'exit', { signal: AbortSignal.timeout(STOP_TIMEOUT_MS) });
      process.kill('SIGKILL');
      await forced;
    }
  }
  /** 从相同临时账本启动或重启，用于验证已提交状态的恢复。 */
  async function start() {
    processHandle = spawn(binary, ['--config', config], { env: environment, stdio: ['ignore', 'pipe', 'pipe'], windowsHide: true });
    processHandle.stdout.on('data', chunk => { output = (output + chunk.toString()).slice(-6000); });
    processHandle.stderr.on('data', chunk => { output = (output + chunk.toString()).slice(-6000); });
    let startError = null; processHandle.on('error', error => { startError = error; });
    const deadline = Date.now() + START_TIMEOUT_MS;
    while (Date.now() < deadline) {
      if (startError) throw startError;
      if (processHandle.exitCode !== null) throw new Error(`Isolated Server exited ${processHandle.exitCode}: ${output}`);
      try { if ((await fetch(`${origin}/healthz`, { signal: AbortSignal.timeout(1000) })).ok) return; }
      catch { /* 监听尚未就绪，继续有限启动检查。 */ }
      await new Promise(resolve => setTimeout(resolve, 50));
    }
    throw new Error('Isolated Server start timeout');
  }
  /** 向测试 Server 发送带指定身份的 JSON，返回状态与内容；不把 Token 写入错误。 */
  async function api(endpoint, body, token = fixture.pat) {
    const response = await fetch(origin + endpoint, { method: 'POST', signal: AbortSignal.timeout(HTTP_TIMEOUT_MS),
      headers: { 'Content-Type': 'application/json', ...(token ? { Authorization: `Bearer ${token}` } : {}) }, body: JSON.stringify(body) });
    const text = await response.text();
    return { status: response.status, body: text ? JSON.parse(text) : null };
  }
  /** 停止测试 Server，并删除已核对位于系统临时目录的测试数据。 */
  async function close() {
    await stop();
    const relative = path.relative(path.resolve(tmpdir()), path.resolve(directory));
    if (!relative.startsWith('..') && !path.isAbsolute(relative) && path.basename(directory).startsWith('webcodex-continuation-server-'))
      await rm(directory, { recursive: true, force: true, maxRetries: 3, retryDelay: 100 });
  }
  try {
    await start();
    const user = await api('/api/users/create', { username: 'continuation-test', role: 'user' }, bootstrap);
    if (user.status !== 200) throw new Error('Could not create isolated fixture user');
    const token = await api('/api/tokens/create', { username: 'continuation-test', name: 'continuation', scopes: ['session:collaborate'] }, bootstrap);
    fixture.pat = token.body?.token;
    if (token.status !== 200 || !fixture.pat) throw new Error('Could not create scoped fixture PAT');
    return Object.assign(fixture, { origin, api, close, stop, start, bootstrap });
  } catch (error) { await close(); throw error; }
}

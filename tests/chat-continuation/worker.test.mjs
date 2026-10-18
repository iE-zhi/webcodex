/** 真正 worker 配合受控 Server 协议 fake；只测试浏览器执行端，决策和持久化由 Rust 测试验证。 */
import test from 'node:test';
import assert from 'node:assert/strict';

/** 固定合成会话与服务端投递 ID，不含真实用户数据。 */
const CONVERSATION = '11111111-1111-4111-8111-111111111111';
const DELIVERY = '33333333-3333-4333-8333-333333333333';
/** 动态导入序号使每个测试获得独立 worker。 */
let moduleId = 0;

/** 生成指定 origin 的协议 fake；configured 控制是否已有连接，goalComplete、loseClaim 控制服务端返回。 */
async function harness(configured = true, origin = 'http://127.0.0.1:8080') {
  const local = configured ? { 'continuation.connection.v2': { url: origin, token: 'wc_pat_' + 'x'.repeat(64) } } : {};
  const session = {}, events = {}, requests = [], sends = [];
  const app = { local, session, requests, sends, task: null, goalComplete: false, loseClaim: false, failSend: false };
  app.page = { url: `https://chatgpt.com/c/${CONVERSATION}`, supported: true, userId: 'u1', userText: '原始任务', busy: false,
    blocked: false, draft: false, reply: { id: 'a1', userId: 'u1', text: '还有工作未完成', terminal: true } };
  /** 内存存储按 Chrome JSON 拷贝语义读写，不与生产对象共享引用。 */
  const storage = data => ({ async get(key) { return { [key]: structuredClone(data[key]) }; },
    async set(value) { Object.assign(data, structuredClone(value)); }, async setAccessLevel() {} });
  /** 记录事件监听器；tick 会 await 真实处理 Promise，不靠 sleep 模拟完成。 */
  const event = name => ({ addListener(listener) { events[name] = listener; } });
  globalThis.chrome = { storage: { local: storage(local), session: storage(session) },
    runtime: { id: 'test', getURL: path => `chrome-extension://test/${path}`, onMessage: event('message'), onStartup: event('startup'), onInstalled: event('installed') },
    alarms: { async create() {}, onAlarm: event('alarm') },
    scripting: { async executeScript(request) {
      if (request.args?.[0] === 'cancel') return [{ result: { cancelled: true } }];
      if (!request.args || request.args[0] === 'arm') return [{ documentId: 'doc-1', result: structuredClone(app.page) }];
      sends.push(request.args[1].text);
      assert.deepEqual(request.target.documentIds, ['doc-1']);
      assert.equal(app.task.state, 'dispatching', 'Server 必须先确认一次性领取');
      if (app.failSend) throw new Error('closed');
      if (!app.delayReceipt) { app.page.userId = 'receipt-user'; app.page.userText = request.args[1].text; }
      return [{ result: { sent: true } }];
    } } };
  /** 协议 fake 只返回受控后端投影，不在扩展测试中复制 Goal/Loop 决策算法。 */
  globalThis.fetch = async (url, input) => {
    assert.equal(url, `${origin}/api/continuations`);
    assert.equal(input.credentials, 'omit'); assert.equal(input.redirect, 'error');
    const request = JSON.parse(input.body); requests.push(request);
    let dispatch = null;
    if (request.action === 'start') app.task = { id: 'task-1', generation: 1, binding: request.binding, options: request.options,
      state: 'running', reason: '', rounds: 0, next_at: 0, delivery_id: null };
    if (request.action === 'observe' && app.task?.state === 'dispatching' && app.page.userId === 'receipt-user') {
      app.task.state = 'done';
    }
    if (request.action === 'observe' && app.task?.state === 'running' && !request.page.busy) {
      app.task.state = app.goalComplete ? 'done' : 'ready'; app.task.delivery_id = app.goalComplete ? null : DELIVERY;
    }
    if (request.action === 'prepare') {
      assert.equal(app.task.state, 'ready'); app.task.state = 'prepared';
      if (app.loseClaim) throw new Error('Response lost after commit');
      dispatch = { id: DELIVERY, text: `[[WC_SEND:${DELIVERY}]]\n这是后端预留的当前会话续跑指令。` };
    }
    if (request.action === 'authorize') { assert.equal(app.task.state, 'prepared'); app.task.state = 'dispatching'; app.task.rounds++; app.task.send_authorized = true; }
    if (request.action === 'pause') { app.task.state = 'paused'; app.task.reason = request.reason; app.task.generation++; }
    return new Response(JSON.stringify({ tasks: app.task ? [structuredClone(app.task)] : [], dispatch }), { status: 200 });
  };
  await import(`../../extensions/chat-continuation/worker.mjs?test=${++moduleId}`);
  /** 模拟唯一受信 popup，正文不会经过网页消息通道。 */
  app.command = (action, extra = {}) => new Promise(resolve => events.message({ action, tabId: 7, ...extra },
    { id: 'test', url: 'chrome-extension://test/popup.html' }, resolve));
  app.tick = () => events.alarm({ name: 'webcodex-continuation' });
  app.events = events;
  return app;
}

// 普通首页与项目首页的预置均需明确 popup 操作和同文档首发；取消晚于 Server 提交时暂停，不自动发送。
test('新聊天预置在真实首发后创建，取消阻止晚到启用和重复发送', async () => {
  for (const prefix of ['', '/g/g-p-' + 'a'.repeat(32)]) for (const cancel of [false, true]) {
    const app = await harness();
    app.page = { url: 'https://chatgpt.com' + prefix + (prefix ? '/project' : '/'), supported: true, busy: false, draft: true, blocked: false, userId: null, userText: '', reply: null };
    assert.equal((await app.command('start', { options: { objective: '预置目标', mode: 'goal', limit: 10, delay: 30 } })).ok, true);
    assert.equal(app.task, null);
    const pageMessage = message => new Promise(resolve => app.events.message(message, { id: 'test', tab: { id: 7 }, frameId: 0, documentId: 'doc-1', url: app.page.url }, resolve));
    assert.equal((await pageMessage({ event: 'webcodex_intent', text: '第一条请求' })).ok, true);
    const actualPrefix = prefix ? prefix + '-plotjuggler' : '';
    app.page = { ...app.page, url: `https://chatgpt.com${actualPrefix}/c/${CONVERSATION}`, userId: 'u1', userText: '第一条请求', userCount: 1, busy: true, draft: false };
    const original = globalThis.fetch;
    let release, reached;
    const held = new Promise(resolve => { release = resolve; }), arrived = new Promise(resolve => { reached = resolve; });
    globalThis.fetch = async (url, input) => {
      const result = await original(url, input);
      if (JSON.parse(input.body).action === 'start') { reached(); await held; }
      return result;
    };
    const tick = app.tick(); await arrived;
    if (cancel) assert.equal((await app.command('pause')).ok, true);
    release(); await tick;
    assert.equal(app.requests.filter(request => request.action === 'start').length, 1);
    assert.equal(app.task.state, cancel ? 'paused' : 'running');
    assert.equal(app.task.binding.prefix, actualPrefix);
    assert.equal(app.sends.length, 0);
    if (!cancel) assert.equal((await app.command('list')).prearm, null);
  }
});

test('无后端连接时禁止发送，不能降级到扩展自主续跑', async () => {
  const app = await harness(false);
  assert.equal((await app.command('continue')).error, 'connection_required');
  assert.equal(app.sends.length, 0); assert.equal(app.requests.length, 0);
});

// 已有会话的未发送草稿只报告明确原因；不得创建 Server 任务、清理文本或点击发送。
test('已有会话存在草稿时启用和Continue均明确拒绝，不创建任务或自动发送', async () => {
  for (const action of ['start', 'continue']) {
    const app = await harness(); app.page.draft = true;
    const result = await app.command(action, { options: { objective: '', mode: 'goal', limit: 10, delay: 30 } });
    assert.equal(result.error, 'page_draft');
    assert.equal(app.requests.some(request => request.action === 'start'), false);
    assert.equal(app.task, null); assert.equal(app.page.draft, true); assert.equal(app.sends.length, 0);
  }
});

// 页面模型未知不能先创建任务再交给 Server 返回笼统冲突，必须保留明确的页面失败状态。
test('已有会话页面不支持时启用明确拒绝，不创建任务', async () => {
  const app = await harness(); app.page.supported = false;
  const result = await app.command('start', { options: { objective: '', mode: 'goal', limit: 10, delay: 30 } });
  assert.equal(result.error, 'unsupported');
  assert.equal(app.requests.some(request => request.action === 'start'), false);
  assert.equal(app.task, null); assert.equal(app.sends.length, 0);
});

test('仅发送 Server 一次性领取正文，重复观测不重放 dispatching 消息', async () => {
  const app = await harness();
  assert.equal((await app.command('continue')).ok, true);
  assert.equal(app.sends[0], `[[WC_SEND:${DELIVERY}]]\n这是后端预留的当前会话续跑指令。`);
  await app.tick(); await app.tick();
  assert.equal(app.sends.length, 1);
  assert.equal(app.requests.filter(request => request.action === 'prepare').length, 1);
  assert.equal('continuation.v1' in app.local, false);
});

test('后端确认当前会话 Goal 完成标记后扩展不请求领取也不发送', async () => {
  const app = await harness(); app.goalComplete = true;
  await app.command('start', { options: { objective: '完成目标', mode: 'goal', limit: 2, delay: 30 } });
  assert.equal(app.sends.length, 0);
  assert.equal(app.requests.some(request => request.action === 'prepare'), false);
});

test('领取响应丢失后不重试，发送异常向后端报告未知投递', async () => {
  const app = await harness(); app.loseClaim = true;
  assert.equal((await app.command('continue')).ok, false);
  await app.tick();
  assert.equal(app.sends.length, 0);
  assert.equal(app.requests.filter(request => request.action === 'prepare').length, 1);
  const broken = await harness(); broken.failSend = true;
  await broken.command('continue');
  assert.equal(broken.task.state, 'dispatching');
  await broken.tick(); assert.equal(broken.sends.length, 1);
});

test('网页无控制权且配置拒绝公网明文 HTTP，PAT 不出现在响应', async () => {
  const app = await harness(false, 'http://10.28.221.1:18080');
  assert.equal((await app.command('configure', { connection: { url: 'http://example.com', token: 'wc_pat_' + 'x'.repeat(64) } })).error, 'connection_invalid');
  assert.equal(app.events.message({ action: 'continue', tabId: 7 }, { id: 'test', tab: { id: 7 }, url: app.page.url }, () => assert.fail('网页不得获得控制响应')), false);
  const result = await app.command('configure', { connection: { url: 'http://10.28.221.1:18080', token: 'wc_pat_' + 'x'.repeat(64) } });
  assert.equal(result.connected, true); assert.equal(JSON.stringify(result).includes('wc_pat_'), false);
});

// 可信 popup 只读凭据元数据；空输入只允许复用同 origin，失败不能覆盖旧凭据。
test('保存的 PAT 可在重开面板后复用，换地址和认证失败均不覆盖原连接', async () => {
  const app = await harness();
  const original = structuredClone(app.local['continuation.connection.v2']);
  const metadata = await app.command('connection');
  assert.deepEqual(metadata, { ok: true, url: original.url, tokenSaved: true });
  assert.equal(app.requests.length, 0, '读保存状态不依赖后端在线');
  assert.equal(JSON.stringify(metadata).includes(original.token), false);
  const reused = await app.command('configure', { connection: { url: original.url, token: '' } });
  assert.equal(reused.connected, true);
  assert.equal(reused.tokenSaved, true);
  assert.deepEqual(reused.tasks, []);
  assert.equal(JSON.stringify(reused).includes(original.token), false);
  assert.equal((await app.command('configure', { connection: { url: 'http://10.28.221.1:18080', token: '' } })).error, 'connection_invalid');
  assert.deepEqual(app.local['continuation.connection.v2'], original);
  globalThis.fetch = async () => new Response(JSON.stringify({ error: 'continuation_account_required' }), { status: 403 });
  assert.equal((await app.command('configure', { connection: { url: original.url, token: 'wc_pat_' + 'y'.repeat(64) } })).ok, false);
  assert.deepEqual(app.local['continuation.connection.v2'], original);
  globalThis.fetch = async () => new Response(JSON.stringify({ error: 'unauthorized' }), { status: 401 });
  assert.equal((await app.command('configure', { connection: { url: original.url, token: '' } })).error, 'pat_rejected');
  assert.deepEqual(app.local['continuation.connection.v2'], original);
  const empty = await harness(false);
  assert.deepEqual(await empty.command('connection'), { ok: true, url: '', tokenSaved: false });
});

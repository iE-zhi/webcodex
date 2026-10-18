/** 刷新后的真实 worker 回归；协议 fake 只授予显式投递，页面加载和文档切换由测试推进。 */
import test from 'node:test';
import assert from 'node:assert/strict';
import { randomUUID } from 'node:crypto';

/** 每个场景独立加载 worker，模拟 MV3 运行期内存被回收。 */
let sequence = 0;
/** 合成任务仍有一小时授权；模拟加载异常的两分钟超过生产九十秒诊断阈值。 */
const TASK_LIFETIME_MS = 3600000, STALE_PAGE_MS = 120000;

/** 合成已启用任务；rebind 模拟原标签关闭后在唯一新标签等待页面挂载。 */
async function fixture({ state = 'running', rebind = false } = {}) {
  const client = randomUUID(), browser = randomUUID(), events = {}, requests = [], sends = [];
  const binding = { client, browser, origin: 'https://chatgpt.com', prefix: '', conversation: randomUUID(), tab_id: 7 };
  const task = { id: randomUUID(), generation: 5, binding, state, reason: state === 'paused' ? 'user' : '', rounds: 3,
    options: { mode: 'goal', objective: '完成原任务', limit: 100, delay: 30 }, observed_at: Date.now(), next_at: 0, expires_at: Date.now() + TASK_LIFETIME_MS };
  const app = { task, requests, sends, events, document: 'before-refresh', ready: false, tabStatus: 'complete',
    page: { url: `${binding.origin}/c/${binding.conversation}`, supported: true, busy: false, draft: false, blocked: false,
      userId: 'human', userText: '原请求', reply: { id: 'reply', userId: 'human', terminal: true, text: '待继续' } } };
  /** 只持有合成安装身份；新 worker 使用相同会话存储，不访问个人 Chrome 数据。 */
  const local = { 'continuation.client.v2': client, 'continuation.connection.v2': { url: 'http://127.0.0.1:8080', token: 'wc_pat_' + 'x'.repeat(64) } };
  const session = { 'continuation.browser.v2': browser };
  const storage = data => ({ async get(key) { return { [key]: structuredClone(data[key]) }; },
    async set(value) { Object.assign(data, structuredClone(value)); }, async setAccessLevel() {} });
  /** 捕获事件入口；每次 tick 等待实际 worker 完成，不使用真实定时器。 */
  const event = name => ({ addListener(callback) { events[name] = callback; } });
  globalThis.chrome = { storage: { local: storage(local), session: storage(session) },
    runtime: { id: 'refresh', getURL: name => `chrome-extension://refresh/${name}`, onMessage: event('message'), onStartup: event('startup'), onInstalled: event('installed') },
    alarms: { async create() {}, onAlarm: event('alarm') },
    tabs: { async query() { return [{ id: rebind ? 99 : 7, url: app.page.url, status: app.tabStatus, pendingUrl: app.pendingUrl }]; },
      async get(id) { return { id, status: app.tabStatus, url: app.tabUrl || app.page.url, pendingUrl: app.pendingUrl }; },
      async reload() { assert.fail('页面挂载期间不能再次刷新'); }, async create() { assert.fail('已存在唯一会话，不能另开'); } },
    scripting: { async executeScript(request) {
      if (rebind && request.target.tabId === 7) throw new Error('old tab gone');
      if (request.args?.[0] === 'send') {
        assert.deepEqual(request.target.documentIds, [app.document]);
        assert.equal(request.args[1].url, app.page.url, '网页点击必须核对当前实际路径，不能使用服务端绑定别名');
        sends.push(request.args[1].id);
        return [{ result: { sent: true } }];
      }
      return [{ documentId: app.document, result: structuredClone(app.page) }];
    } } };
  /** 只在测试明确设置 ready 时返回一次发送权；未知投递始终不重新授权。 */
  const originalFetch = globalThis.fetch;
  globalThis.fetch = async (_url, input) => {
    const request = JSON.parse(input.body); requests.push(request);
    if (request.page && request.binding) assert.equal(request.page.url,
      `${request.binding.origin}${request.binding.prefix}/c/${request.binding.conversation}`, 'Server 始终收到原有绑定路径');
    let dispatch = null;
    if (request.action === 'rebind') task.binding = request.replacement;
    if (request.action === 'pause') { task.state = 'paused'; task.reason = request.reason; }
    if (request.action === 'observe' && app.ready && task.state === 'running') task.state = 'ready';
    if (request.action === 'prepare') { task.state = 'prepared'; dispatch = { id: 'one-delivery', text: 'Server 预留的续跑消息' }; }
    if (request.action === 'authorize') { task.state = 'dispatching'; task.send_authorized = true; }
    return Response.json({ tasks: [structuredClone(task)], dispatch });
  };
  /** 重载模块模拟 worker 休眠，不改变安装、任务或浏览器主文档。 */
  app.restartWorker = () => import(`../../extensions/chat-continuation/worker.mjs?refresh=${++sequence}`);
  await app.restartWorker();
  /** 同步推进一次生产 alarm；挂起的只读回执仍受生产总期限限制。 */
  app.tick = () => events.alarm({ name: 'webcodex-continuation' });
  /** 主文档真实停止通道；只验证确切用户操作的任务匹配，不用网页触发启用权。 */
  app.stop = () => new Promise(resolve => events.message({ event: 'webcodex_activity', stop: true },
    { id: 'refresh', frameId: 0, tab: { id: task.binding.tab_id }, documentId: app.document, url: app.page.url }, resolve));
  /** 受信 popup 的只读列表，不直接读取生产浏览器存储。 */
  app.list = () => new Promise(resolve => events.message({ action: 'list', tabId: task.binding.tab_id },
    { id: 'refresh', url: 'chrome-extension://refresh/popup.html' }, resolve));
  /** 释放本夹具的 fetch 替换，不影响任何真实服务或页面。 */
  app.close = () => { globalThis.fetch = originalFetch; };
  return app;
}

// 手动及自动刷新均可能先得到缺少输入框/React 的快照；原版本、期限、轮数和未知投递必须保留。
test('刷新挂载和worker休眠后自动恢复原任务，不暂停、不重置且只发送一次', async () => {
  const app = await fixture();
  try {
    const original = structuredClone(app.task);
    app.document = 'after-refresh'; app.page.supported = false;
    await app.tick();
    assert.equal((await app.list()).tasks[0].pageStatus, 'page_loading');
    app.task.observed_at -= STALE_PAGE_MS;
    await app.tick();
    assert.equal((await app.list()).tasks[0].pageStatus, 'page_unreadable');
    await app.restartWorker(); await app.tick();
    assert.equal(app.task.state, 'running'); assert.equal(app.task.generation, original.generation);
    assert.equal(app.task.rounds, original.rounds); assert.equal(app.task.expires_at, original.expires_at);
    assert.equal(app.requests.some(request => ['pause', 'observe', 'repair', 'prepare', 'authorize', 'start'].includes(request.action)), false);
    app.page.supported = true; app.ready = true;
    await app.tick(); await app.tick();
    assert.deepEqual(app.sends, ['one-delivery']);
    assert.equal(app.requests.filter(request => request.action === 'prepare').length, 1);
    assert.equal((await app.list()).tasks[0].pageStatus, '');
  } finally { app.close(); }
});

// 旧标签不可用时，新标签在完整读取前不能重绑定；就绪后自动重绑定并继续观察。
test('唯一恢复标签加载完成后自动重绑定，加载期间不消耗修复预算', async () => {
  const app = await fixture({ rebind: true });
  try {
    app.tabStatus = 'loading';
    await app.tick();
    assert.equal(app.task.binding.tab_id, 7);
    assert.equal(app.requests.some(request => ['repair', 'pause', 'rebind', 'observe'].includes(request.action)), false);
    app.tabStatus = 'complete';
    app.page.supported = false; app.document = 'restored-document';
    await app.tick(); assert.equal(app.task.binding.tab_id, 7);
    assert.equal(app.requests.some(request => ['repair', 'pause', 'rebind', 'observe'].includes(request.action)), false);
    app.page.supported = true; app.page.busy = true;
    await app.tick();
    assert.equal(app.task.binding.tab_id, 99);
    assert.equal(app.requests.filter(request => request.action === 'rebind').length, 1);
    assert.equal(app.requests.some(request => request.action === 'observe'), true);
    assert.equal(app.sends.length, 0);
  } finally { app.close(); }
});

// 未知投递只恢复观察；用户暂停或已停止的任务不能因为页面恢复而重新启用。
test('刷新不重放未知消息，也不越过手动暂停和停止', async () => {
  for (const state of ['dispatching', 'uncertain', 'paused', 'done']) {
    const app = await fixture({ state });
    try {
      app.page.supported = false; await app.tick();
      app.page.supported = true; await app.tick();
      assert.equal(app.task.state, state); assert.equal(app.sends.length, 0);
      assert.equal(app.requests.some(request => ['start', 'prepare', 'authorize', 'repair'].includes(request.action)), false);
      assert.equal(app.requests.some(request => request.action === 'observe'), !['paused', 'done'].includes(state));
    } finally { app.close(); }
  }
});

// 无法完整读取明确超限的正文属于固定协议边界，不能被当成暂时挂载而永久重试。
test('明确消息超限仍暂停，真实导航仍停止旧会话自动化', async () => {
  for (const reason of ['page_limit', 'navigation']) {
    const app = await fixture();
    try {
      if (reason === 'page_limit') { app.page.supported = false; app.page.unsupportedReason = reason; }
      else app.page.url = `https://chatgpt.com/c/${randomUUID()}`;
      await app.tick();
      assert.equal(app.task.state, 'paused'); assert.equal(app.task.reason, reason);
      assert.equal(app.sends.length, 0);
    } finally { app.close(); }
  }
});

// 项目固定 ID 和会话 UUID 未变时，名称后缀出现、消失或改名均不能撤销原任务。
test('活动项目名称别名变化仍观察原任务，Server绑定和网页发送地址分别核对', async () => {
  const project = '/g/g-p-' + 'a'.repeat(32);
  for (const [before, after] of [[project, project + '-plotjuggler'], [project + '-plotjuggler', project], [project + '-old', project + '-new']]) {
    const app = await fixture();
    try {
      app.task.binding.prefix = before;
      app.page.url = `${app.task.binding.origin}${after}/c/${app.task.binding.conversation}`;
      await app.tick();
      assert.equal(app.task.state, 'running'); assert.equal(app.task.binding.prefix, before);
      assert.equal(app.requests.filter(request => request.action === 'observe').length, 1);
      assert.equal(app.requests.some(request => ['pause', 'repair', 'start'].includes(request.action)), false);
      app.ready = true; await app.tick(); await app.tick();
      assert.deepEqual(app.sends, ['one-delivery']);
    } finally { app.close(); }
  }
});

// Chrome 标签仍在加载、存在 pendingUrl 或快照晚于导航时，不能据临时地址暂停任务。
test('刷新临时路由与晚到快照等待原会话加载，不误报会话切换', async () => {
  const app = await fixture();
  try {
    const originalUrl = app.page.url;
    app.page.url = 'https://chatgpt.com/'; app.page.supported = false;
    app.tabStatus = 'loading'; app.tabUrl = originalUrl;
    await app.tick();
    app.tabStatus = 'complete'; await app.tick();
    app.tabUrl = 'https://chatgpt.com/'; app.pendingUrl = originalUrl; await app.tick();
    assert.equal(app.task.state, 'running');
    assert.equal(app.requests.some(request => ['pause', 'observe', 'prepare', 'repair', 'start'].includes(request.action)), false);
    app.pendingUrl = undefined; app.tabUrl = originalUrl; app.page.url = originalUrl; app.page.supported = true;
    await app.tick();
    assert.equal(app.requests.filter(request => request.action === 'observe').length, 1);
    assert.equal(app.sends.length, 0);
  } finally { app.close(); }
});

// 相同会话 UUID 不能跨项目、origin、普通/GPT 路径或非法项目后缀继承授权。
test('稳定导航到其他项目或站点仍暂停，项目别名不能扩大范围', async () => {
  const project = '/g/g-p-' + 'a'.repeat(32);
  for (const [origin, changed] of [['https://chatgpt.com', project.replace(/a/g, 'b')], ['https://chatgpt.com', '/g/other-gpt'],
    ['https://chatgpt.com', project + '-bad.name'], ['https://chatgpt.com', ''], ['https://chat.openai.com', project]]) {
    const app = await fixture();
    try {
      app.task.binding.prefix = project;
      app.page.url = `${origin}${changed}/c/${app.task.binding.conversation}`;
      await app.tick();
      assert.equal(app.task.state, 'paused'); assert.equal(app.task.reason, 'navigation');
      assert.equal(app.requests.some(request => request.action === 'observe'), false);
      assert.equal(app.sends.length, 0);
    } finally { app.close(); }
  }
});

// 显示路径变化不削弱真实 Stop；当前文档发出的停止仍能立即暂停原绑定任务。
test('项目别名下的真实停止仍撤销原任务', async () => {
  const app = await fixture();
  try {
    app.task.binding.prefix = '/g/g-p-' + 'a'.repeat(32);
    app.page.url = `${app.task.binding.origin}${app.task.binding.prefix}-plotjuggler/c/${app.task.binding.conversation}`;
    assert.equal((await app.stop()).ok, true);
    assert.equal(app.task.state, 'paused'); assert.equal(app.task.reason, 'user_stopped');
    assert.equal(app.sends.length, 0);
  } finally { app.close(); }
});

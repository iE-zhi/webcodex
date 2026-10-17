/** 生产 worker 的页面心跳、真实停止和总回执期限回归；使用固定 Chrome/HTTP fake，不访问个人聊天。 */
import test from 'node:test';
import assert from 'node:assert/strict';
import { randomUUID } from 'node:crypto';

/** 每夹具使用独立模块实例，避免任务取消版本在场景间共享。 */
let sequence = 0;

/** options.stall 为 page/http 时只挂起首个任务发送后的只读回执，其他任务独立完成。 */
async function fixture({ stall = '', holdAuthorization = false } = {}) {
  const client = randomUUID(), browser = randomUUID(), events = {}, requests = [], sends = [], pages = new Map();
  const tasks = Array.from({ length: 2 }, (_, tab) => {
    const conversation = randomUUID();
    pages.set(tab, { url: `https://chatgpt.com/c/${conversation}`, supported: true, busy: false, draft: false, blocked: false,
      userId: 'human', userText: '原请求', reply: { id: 'reply', userId: 'human', text: '未完成', terminal: true } });
    return { id: randomUUID(), generation: 1, binding: { client, browser, conversation, origin: 'https://chatgpt.com', prefix: '', tab_id: tab },
      options: {}, state: 'running', next_at: 0, delivery_id: randomUUID() };
  });
  /** 存储异步接口使用值克隆；不读取生产 Chrome profile。 */
  const storage = data => ({ async get(key) { return { [key]: structuredClone(data[key]) }; }, async set(value) { Object.assign(data, structuredClone(value)); }, async setAccessLevel() {} });
  const event = key => ({ addListener(fn) { events[key] = fn; } });
  let release, reached;
  const held = new Promise(resolve => { release = resolve; }), authorized = new Promise(resolve => { reached = resolve; });
  globalThis.chrome = { storage: { local: storage({ 'continuation.client.v2': client, 'continuation.connection.v2': { url: 'http://127.0.0.1:8080', token: 'wc_pat_' + 'x'.repeat(64) } }), session: storage({ 'continuation.browser.v2': browser }) },
    alarms: { async create() {}, onAlarm: event('alarm') },
    runtime: { id: 'observer-test', getURL: name => `chrome-extension://observer-test/${name}`, onMessage: event('message'), onStartup: event('startup'), onInstalled: event('installed') },
    scripting: { async executeScript(request) {
      const tab = request.target.tabId, page = pages.get(tab);
      if (request.args?.[0] === 'cancel') return [{ result: { cancelled: true } }];
      if (request.args?.[0] === 'send') {
        sends.push(tab); page.userId = 'receipt'; page.userText = request.args[1].text;
        return [{ result: { sent: true } }];
      }
      if (stall === 'page' && tab === 0 && sends.includes(tab)) return new Promise(() => {});
      return [{ documentId: `document-${tab}`, result: structuredClone(page) }];
    } } };
  const originalFetch = globalThis.fetch;
  globalThis.fetch = async (_url, input) => {
    const request = JSON.parse(input.body); requests.push(request);
    const task = tasks.find(task => task.id === request.task_id);
    let dispatch = null;
    if (request.action === 'observe') {
      if (stall === 'http' && task.binding.tab_id === 0 && sends.includes(0)) return new Promise((_, reject) => {
        // fake 尊重生产 AbortSignal；不把 HTTP 超时误当成未点击证明。
        input.signal.addEventListener('abort', () => reject(input.signal.reason), { once: true });
      });
      if (task.state === 'running') task.state = 'ready';
      else if (task.state === 'dispatching' && request.page.user_id === 'receipt') task.state = 'running';
    }
    if (request.action === 'pause') { task.state = 'paused'; task.reason = request.reason; task.generation++; }
    if (request.action === 'prepare') { task.state = 'prepared'; dispatch = { id: task.delivery_id, text: `[[WC_SEND:${task.delivery_id}]]\n继续` }; }
    if (request.action === 'authorize') {
      task.state = 'dispatching'; task.send_authorized = true;
      if (holdAuthorization) { reached(); await held; }
    }
    if (request.action === 'not_sent') task.delivery_id = null;
    return Response.json({ tasks: request.action === 'list' ? tasks : [task], dispatch });
  };
  await import(`../../extensions/chat-continuation/worker.mjs?observer=${++sequence}`);
  return { tasks, pages, requests, sends, authorized, release,
    tick: () => events.alarm({ name: 'webcodex-continuation' }),
    /** 默认发送者为正确主文档；override 仅用于越权和旧文档边界测试。 */
    message: (message, override = {}) => new Promise(resolve => {
      const sender = { id: 'observer-test', frameId: 0, documentId: 'document-0', url: pages.get(0).url, tab: { id: 0 }, ...override };
      if (!events.message(message, sender, resolve)) resolve(null);
    }),
    close() { release(); globalThis.fetch = originalFetch; },
  };
}

// 只读页面或 HTTP 卡住均占用同一三秒预算；未知回执保留，第二任务不会重复点击。
test('页面和HTTP回执共用三秒总期限，其他任务照常完成', { timeout: 12000 }, async () => {
  for (const stall of ['page', 'http']) {
    const app = await fixture({ stall });
    try {
      const started = Date.now(); await app.tick();
      assert.ok(Date.now() - started < 4500, `${stall} 未遵守总回执期限`);
      assert.deepEqual(app.sends.slice().sort(), [0, 1]);
      assert.equal(app.tasks[0].state, 'dispatching');
      assert.equal(app.tasks[1].state, 'running');
    } finally { app.close(); }
  }
});

// 页面只有唤醒和真实 Stop 通道；跨站、子框架、无文档身份均无效，不能伪造 popup 操作。
test('页面事件严格验证来源并立即撤销晚到的发送授权', async () => {
  const app = await fixture({ holdAuthorization: true });
  try {
    assert.equal(await app.message({ action: 'start' }), null);
    await app.message({ event: 'webcodex_activity', stop: true }, { frameId: 1 });
    await app.message({ event: 'webcodex_activity', stop: true }, { url: 'https://example.com' });
    await app.message({ event: 'webcodex_activity', stop: true }, { documentId: undefined });
    assert.equal(app.requests.length, 0);
    const running = app.tick(); await app.authorized;
    assert.equal((await app.message({ event: 'webcodex_activity', stop: true })).ok, true);
    app.release(); await running;
    assert.equal(app.tasks[0].reason, 'user_stopped');
    assert.equal(app.sends.includes(0), false);
    assert.equal(app.sends.filter(tab => tab === 1).length, 1);
  } finally { app.close(); }
});

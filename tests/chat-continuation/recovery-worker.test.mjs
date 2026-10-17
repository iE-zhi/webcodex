/** 用真实 worker 和有界协议 fake 验证全量调度、即时回执与浏览器重新绑定；不发送真实聊天。 */
import test from 'node:test';
import assert from 'node:assert/strict';
import { randomUUID } from 'node:crypto';

/** 32 个任务为生产账户上限；fake 的每次点击立即发布公开回执，不伪造后端完成决定。 */
test('一轮覆盖32个任务并立即确认全部回执，不等待八轮alarm', async () => {
  const client = randomUUID(), browser = randomUUID(), events = {}, pages = new Map(), sends = [];
  const tasks = Array.from({ length: 32 }, (_, tab) => {
    const conversation = randomUUID();
    pages.set(tab, { url: `https://chatgpt.com/c/${conversation}`, supported: true, busy: false, draft: false,
      blocked: false, userId: 'u1', userText: '原任务', reply: { id: 'a1', userId: 'u1', text: '未完成', terminal: true } });
    return { id: randomUUID(), generation: 1, binding: { client, browser, conversation, origin: 'https://chatgpt.com', prefix: '', tab_id: tab },
      options: { mode: 'goal' }, state: 'running', next_at: 0, delivery_id: randomUUID() };
  });
  /** Chrome 存储保持原安装和进程身份；所有页面结果采用 JSON 拷贝。 */
  const storage = data => ({ async get(key) { return { [key]: data[key] }; }, async set(value) { Object.assign(data, value); }, async setAccessLevel() {} });
  /** 捕获 alarm 回调，测试显式推进而不依赖真实三十秒计时器。 */
  const event = key => ({ addListener(fn) { events[key] = fn; } });
  globalThis.chrome = { storage: { local: storage({ 'continuation.connection.v2': { url: 'http://127.0.0.1:8080', token: 'wc_pat_' + 'x'.repeat(64) }, 'continuation.client.v2': client }),
    session: storage({ 'continuation.browser.v2': browser }) },
    runtime: { id: 'test', getURL: name => name, onMessage: event('message'), onStartup: event('startup'), onInstalled: event('installed') },
    alarms: { async create() {}, onAlarm: event('alarm') },
    scripting: { async executeScript(request) {
      const id = request.target.tabId, page = pages.get(id);
      if (!request.args || request.args[0] === 'arm') return [{ documentId: `doc-${id}`, result: structuredClone(page) }];
      sends.push(id); page.userId = 'receipt'; page.userText = request.args[1].text;
      return [{ result: { sent: true } }];
    } } };
  /** Server fake 只实现预留与回执状态，不替代 Rust 的目标判断及安全校验。 */
  globalThis.fetch = async (_url, input) => {
    const request = JSON.parse(input.body), task = tasks.find(task => task.id === request.task_id);
    let dispatch = null;
    if (request.action === 'observe') {
      if (task.state === 'dispatching' && request.page.user_id === 'receipt') task.state = 'running';
      else if (task.state === 'running') task.state = 'ready';
    }
    if (request.action === 'prepare') { assert.equal(task.state, 'ready'); task.state = 'prepared'; dispatch = { id: task.delivery_id, text: `[[WC_SEND:${task.delivery_id}]]\n继续` }; }
    if (request.action === 'authorize') { assert.equal(task.state, 'prepared'); task.state = 'dispatching'; task.send_authorized = true; }
    return Response.json({ tasks: request.action === 'list' ? tasks : [task], dispatch });
  };
  await import('../../extensions/chat-continuation/worker.mjs?32-tasks');
  await events.alarm({ name: 'webcodex-continuation' });
  assert.equal(new Set(sends).size, 32); assert.equal(sends.length, 32);
  assert.equal(tasks.filter(task => task.state === 'dispatching').length, 0);
});

/** 已存在的原会话可在新浏览器绑定，不重开标签、不修改目标或重新发送消息。 */
test('浏览器重启后绑定唯一原会话，旧页面版本不能获得发送', async () => {
  const client = randomUUID(), browser = randomUUID(), oldBrowser = randomUUID(), conversation = randomUUID(), events = {}, requests = [];
  const page = { url: `https://chatgpt.com/c/${conversation}`, supported: true, userId: 'u1', userText: '原约束', busy: false, draft: false, blocked: false, reply: null };
  const task = { id: randomUUID(), generation: 3, state: 'running', binding: { client, browser: oldBrowser, conversation, origin: 'https://chatgpt.com', prefix: '', tab_id: 7 }, options: {}, repair_at: null };
  /** 存储只暴露测试安装和新浏览器身份，不访问个人扩展数据。 */
  const storage = data => ({ async get(key) { return { [key]: data[key] }; }, async set() {}, async setAccessLevel() {} });
  /** 捕获实际事件入口供测试同步等待。 */
  const event = key => ({ addListener(fn) { events[key] = fn; } });
  globalThis.chrome = { storage: { local: storage({ 'continuation.connection.v2': { url: 'http://127.0.0.1:8080', token: 'wc_pat_' + 'x'.repeat(64) }, 'continuation.client.v2': client }), session: storage({ 'continuation.browser.v2': browser }) },
    runtime: { id: 'test', getURL: name => name, onMessage: event('message'), onStartup: event('startup'), onInstalled: event('installed') },
    alarms: { async create() {}, onAlarm: event('alarm') }, tabs: { async query() { return [{ id: 99, url: page.url }]; }, async create() { assert.fail('已存在的会话不能重开'); } },
    scripting: { async executeScript(request) { if (request.target.tabId === 7) throw new Error('old tab gone'); return [{ documentId: 'new-document', result: page }]; } } };
  /** 重绑定响应保留控制版本，只有绑定从旧标签移动到新标签。 */
  globalThis.fetch = async (_url, input) => { const request = JSON.parse(input.body); requests.push(request);
    if (request.action === 'rebind') task.binding = request.replacement;
    return Response.json({ tasks: [task], dispatch: null }); };
  await import('../../extensions/chat-continuation/worker.mjs?restart-rebind');
  await events.alarm({ name: 'webcodex-continuation' });
  assert.equal(task.binding.browser, browser); assert.equal(task.binding.tab_id, 99); assert.equal(task.generation, 3);
  assert.deepEqual(requests.map(request => request.action), ['list', 'rebind', 'observe']);
});

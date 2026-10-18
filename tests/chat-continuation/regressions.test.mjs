/** 审计缺陷的真实 Server/SQLite 回归；Chrome 使用受控 fake，不访问个人聊天或生产部署。 */
import test from 'node:test';
import assert from 'node:assert/strict';
import { randomUUID } from 'node:crypto';
import { serverFixture } from './server-fixture.mjs';

/** 模块导入序号使每个场景获得自己的运行期取消版本及身份缓存。 */
let sequence = 0;
/** 真实后端最短续跑间隔为三十秒；额外一秒覆盖本机请求调度，不伪造服务端时钟。 */
const TRANSPORT_OBSERVATION_MS = 31000;

/** 运行一个合成浏览器；options 控制项目路径、重复标签、页面失效和响应交付屏障，server 为测试独占真实实例。 */
async function browser(server, { duplicate = false, unsupported = false, uninjectable = false, browserChanged = false, hold = '', prefix = '' } = {}) {
  const client = randomUUID(), process = randomUUID(), events = {}, requests = [], sends = [], reloads = [], retries = [];
  const binding = { client, browser: duplicate || browserChanged ? randomUUID() : process, conversation: randomUUID(), origin: 'https://chatgpt.com', prefix, tab_id: 7 };
  const page = { url: `https://chatgpt.com${prefix}/c/${binding.conversation}`, supported: !unsupported, userId: 'u1', userText: '原始人工请求', busy: false,
    draft: false, blocked: false, reply: { id: 'a1', userId: 'u1', text: '尚未完成', terminal: true } };
  let release, arrived;
  const blocked = new Promise(resolve => { release = resolve; }), reached = new Promise(resolve => { arrived = resolve; });
  /** 存储使用 JSON 克隆，测试事件回调仅由本场景显式推进。 */
  const storage = data => ({ async get(key) { return { [key]: structuredClone(data[key]) }; }, async set(value) { Object.assign(data, value); }, async setAccessLevel() {} });
  const event = key => ({ addListener(callback) { events[key] = callback; } });
  globalThis.chrome = { storage: { local: storage({ 'continuation.client.v2': client, 'continuation.connection.v2': { url: server.origin, token: server.pat } }),
    session: storage({ 'continuation.browser.v2': process }) }, alarms: { async create() {}, onAlarm: event('alarm') },
    runtime: { id: 'regression', getURL: name => `chrome-extension://regression/${name}`, onMessage: event('message'), onStartup: event('startup'), onInstalled: event('installed') },
    tabs: { async query() { return Array.from({ length: duplicate ? 2 : 1 }, (_, index) => ({ id: index + 90, url: page.url })); }, async reload(id) { reloads.push(id); } },
    scripting: { async executeScript(request) {
      if (uninjectable || duplicate && !browserChanged && request.target.tabId === 7) throw new Error('isolated unavailable document');
      if (request.files) return [{ documentId: 'isolated-document' }];
      if (request.args?.[0] === 'retry_load') {
        assert.deepEqual(request.target.documentIds, ['isolated-document']);
        retries.push(request.target.tabId);
        return [{ result: { retried: true } }];
      }
      if (request.args?.[0] === 'cancel') return [{ result: { cancelled: true } }];
      if (!request.args || request.args[0] === 'arm') return [{ documentId: 'isolated-document', result: structuredClone(page) }];
      sends.push(request.args[1].text); page.userId = 'receipt'; page.userText = request.args[1].text;
      return [{ result: { sent: true } }];
    } } };
  /** 屏障在真实事务提交且 HTTP 响应取得后才阻挡，不把超时当成未执行。 */
  const originalFetch = globalThis.fetch;
  globalThis.fetch = async (url, input) => {
    const request = JSON.parse(input.body); requests.push(request);
    const result = await originalFetch(url, input);
    if (request.action === hold) { arrived(); await blocked; }
    return result;
  };
  await import(`../../extensions/chat-continuation/worker.mjs?regression=${++sequence}`);
  /** 只模拟受信 popup，页面没有请求权限；extra 含本场景明确的选项或任务 ID。 */
  const command = (action, extra = {}) => new Promise(resolve => events.message({ action, tabId: 7, ...extra },
    { id: 'regression', url: 'chrome-extension://regression/popup.html' }, resolve));
  /** 直接建真实服务端任务，允许验证不能取得页面时的浏览器修复。 */
  const seed = async () => {
    const result = await server.api('/api/continuations', { action: 'start', request_id: randomUUID(), binding,
      options: { objective: '原始任务', mode: 'goal', limit: 10, delay: 30 },
      page: { url: page.url, supported: true, busy: false, draft: false, blocked: false, user_id: 'u1', user_text: page.userText, reply: null } });
    assert.equal(result.status, 200);
  };
  return { page, command, seed, requests, sends, reloads, retries, reached, release,
    list: async () => (await server.api('/api/continuations', { action: 'list', client })).body.tasks,
    tick: () => events.alarm({ name: 'webcodex-continuation' }),
    /** 还原全局 fetch，后续场景不能继承本场景的交付屏障。 */
    close() { globalThis.fetch = originalFetch; release(); } };
}

// 所有场景串行使用一个真实私有 Server；最终无论成功失败均释放响应屏障和进程。
test('真实后端修复 Continue 策略、重复标签、页面故障与暂停后晚到响应', { timeout: 60000 }, async context => {
  const server = await serverFixture(context); if (!server) return;
  let app;
  try {
    app = await browser(server); app.page.busy = true;
    assert.equal((await app.command('continue', { options: { limit: 9, delay: 120, policy: { recovery: false, silence_seconds: 120, lifetime_hours: 1 } } })).ok, true);
    const options = (await app.list())[0].options;
    assert.equal(options.limit, 9); assert.equal(options.delay, 120); assert.equal(options.policy.recovery, false); app.close();

    app = await browser(server, { duplicate: true }); await app.seed(); await app.tick();
    assert.equal((await app.list())[0].reason, 'ambiguous_tabs'); assert.equal((await app.list())[0].state, 'paused'); app.close();

    // 重载扩展后原标签仍可完整验证，真实 Server 允许原地更换临时身份，不被其他副本抢占。
    app = await browser(server, { duplicate: true, browserChanged: true }); await app.seed();
    const beforeReload = (await app.list())[0];
    app.page.busy = true; await app.tick();
    const afterReload = (await app.list())[0];
    assert.equal(afterReload.state, 'running'); assert.equal(afterReload.binding.tab_id, 7);
    assert.notEqual(afterReload.binding.browser, beforeReload.binding.browser);
    assert.equal(afterReload.generation, beforeReload.generation);
    assert.equal(app.requests.filter(request => request.action === 'rebind').length, 1);
    assert.equal(app.requests.some(request => ['pause', 'repair'].includes(request.action)), false); app.close();

    // 刷新期间页面模型不完整不能永久暂停；就绪后使用原任务继续，不重置控制版本、预算或期限。
    app = await browser(server, { unsupported: true }); await app.seed();
    const beforeRefresh = (await app.list())[0];
    await app.tick(); await app.tick();
    const waiting = (await app.list())[0];
    assert.equal(waiting.state, 'running'); assert.equal(waiting.reason, '');
    assert.equal(waiting.id, beforeRefresh.id); assert.equal(waiting.generation, beforeRefresh.generation);
    assert.equal(waiting.rounds, beforeRefresh.rounds);
    assert.equal(app.requests.some(request => ['pause', 'observe', 'repair', 'prepare', 'authorize'].includes(request.action)), false);
    assert.equal(app.sends.length, 0);
    app.page.supported = true; app.page.busy = true;
    await app.tick();
    assert.equal(app.requests.some(request => request.action === 'observe'), true);
    assert.equal((await app.list())[0].id, beforeRefresh.id); assert.equal(app.sends.length, 0); app.close();

    // 使用真实 Server 的严格绑定校验，证明项目显示路径变化不需要迁移任务或重新授予预算。
    const project = '/g/g-p-' + 'a'.repeat(32);
    app = await browser(server, { prefix: project }); await app.seed();
    const originalProjectTask = (await app.list())[0];
    const projectActionsAt = app.requests.length;
    app.page.url = `https://chatgpt.com${project}-plotjuggler/c/${originalProjectTask.binding.conversation}`;
    app.page.busy = true; await app.tick();
    const projectTask = (await app.list())[0];
    assert.equal(projectTask.state, 'running'); assert.equal(projectTask.reason, '');
    assert.equal(projectTask.id, originalProjectTask.id); assert.equal(projectTask.generation, originalProjectTask.generation);
    assert.equal(projectTask.binding.prefix, project); assert.equal(projectTask.rounds, originalProjectTask.rounds);
    assert.equal(app.requests.filter(request => request.action === 'observe').length, 1);
    assert.equal(app.requests.slice(projectActionsAt).some(request => ['pause', 'repair', 'start'].includes(request.action)), false); app.close();

    app = await browser(server, { uninjectable: true }); await app.seed(); await app.tick();
    assert.equal(app.requests.some(request => request.action === 'repair'), false);
    assert.deepEqual(app.reloads, []); app.close();

    // 明确加载失败共享真实 SQLite 修复预算，连续轮询受冷却约束；恢复后原任务继续且只投递一次。
    app = await browser(server); await app.seed();
    const beforeLoadFailure = (await app.list())[0];
    const recoveryActionsAt = app.requests.length;
    app.page.supported = false; app.page.unsupportedReason = 'conversation_load_failed';
    await app.tick(); await app.tick();
    assert.deepEqual(app.retries, [7]); assert.deepEqual(app.reloads, []);
    assert.equal((await app.list())[0].id, beforeLoadFailure.id);
    app.page.supported = true; await app.tick(); await app.tick();
    assert.equal(app.sends.length, 1); assert.match(app.sends[0], /WEBCODEX_GOAL/);
    assert.equal((await app.list())[0].generation, beforeLoadFailure.generation);
    assert.equal(app.requests.slice(recoveryActionsAt).some(request => request.action === 'start'), false); app.close();

    // 同一未完成回合明确传输中断且空闲时，真实 Server 按间隔投递一次，不先刷新或多等静默窗口。
    app = await browser(server); await app.seed();
    app.page.reply.terminal = false; app.page.interrupted = true;
    await app.tick();
    assert.equal(app.sends.length, 0);
    // resolve 仅结束这次有界等待，不写任务、推进假时钟或执行网页动作。
    await new Promise(resolve => setTimeout(resolve, TRANSPORT_OBSERVATION_MS));
    await app.tick(); await app.tick();
    assert.equal(app.sends.length, 1);
    assert.equal(app.requests.some(request => request.action === 'repair'), false);
    assert.equal((await app.list())[0].rounds, 1); app.close();

    // 暂停发生在已提交预算的响应返回前，晚到许可也不能点击 Retry 或发送消息。
    app = await browser(server, { hold: 'repair' }); await app.seed();
    app.page.supported = false; app.page.unsupportedReason = 'conversation_load_failed';
    const repairing = app.tick(); await app.reached;
    const repairingTask = (await app.list())[0];
    assert.equal((await app.command('pause', { taskId: repairingTask.id })).ok, true);
    app.release(); await repairing;
    assert.deepEqual(app.retries, []); assert.deepEqual(app.sends, []);
    assert.equal((await app.list())[0].state, 'paused'); app.close();

    for (const hold of ['prepare', 'authorize']) {
      app = await browser(server, { hold });
      const starting = app.command('start', { options: { objective: '完整实现原请求', mode: 'goal', limit: 10, delay: 30 } });
      await app.reached;
      const task = (await app.list())[0];
      assert.equal((await app.command('pause', { taskId: task.id })).ok, true);
      app.release(); await starting;
      assert.equal(app.sends.length, 0, `${hold} 响应晚到后不能点击`);
      assert.equal((await app.list())[0].state, 'paused');
      if (hold === 'authorize') assert.equal((await app.list())[0].delivery_id, null, '确切未点击证明清理暂停后的债务，不重新启用');
      app.close();
    }
  } finally { app?.close(); await server.close(); }
});

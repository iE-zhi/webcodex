/** 实际 Server + 真实认证/SQLite + 浏览器 worker 验证，页面动作使用受控 Chrome fake。 */
import test from 'node:test';
import assert from 'node:assert/strict';
import { randomUUID } from 'node:crypto';
import { serverFixture } from './server-fixture.mjs';

/** 合成会话 ID，所有状态属于临时测试 Server。 */
const CONVERSATION = '11111111-1111-4111-8111-111111111111';

test('真实 Server 认证、当前会话 Goal/Loop 标记、worker 投递和重启去重贯通', { timeout: 60000 }, async context => {
  const server = await serverFixture(context); if (!server) return;
  try {
    const client = randomUUID();
    assert.equal((await server.api('/api/continuations', { action: 'list', client }, null)).status, 401);
    assert.equal((await server.api('/api/continuations', { action: 'list', client }, server.bootstrap)).status, 403);
    const readOnly = await server.api('/api/tokens/create', { username: 'continuation-test', name: 'read-only', scopes: ['runtime:read'] }, server.bootstrap);
    assert.equal((await server.api('/api/continuations', { action: 'list', client }, readOnly.body.token)).status, 403);
    // 浏览器账户及 bootstrap 不可代发 Runner 租约；未认证请求先由统一鉴权拒绝。
    const lease = { client_id: 'fake-runner', agent_instance_id: randomUUID(), request_id: randomUUID(), browser_absent: true };
    assert.equal((await server.api('/api/shell/agent/continuation', lease, null)).status, 401);
    for (const token of [server.pat, server.bootstrap, readOnly.body.token]) {
      assert.equal((await server.api('/api/shell/agent/continuation', lease, token)).status, 403);
    }
    const local = { 'continuation.connection.v2': { url: server.origin, token: server.pat } }, session = {}, events = {}, sends = [];
    let capturedUser = 'u1', capturedText = '原始任务：完成整个功能。', capturedReply = { id: 'a1', userId: 'u1', terminal: true, text: '实现还没有完成。' };
    /** fake 存储按 Chrome JSON 拷贝，不包含任何模型密钥。 */
    const storage = data => ({ async get(key) { return { [key]: structuredClone(data[key]) }; }, async set(value) { Object.assign(data, structuredClone(value)); }, async setAccessLevel() {} });
    /** 记录生产 worker 的实际监听器，alarm 返回 Promise 供测试确定性等待。 */
    const event = name => ({ addListener(listener) { events[name] = listener; } });
    globalThis.chrome = {
      storage: { local: storage(local), session: storage(session) },
      runtime: { id: 'e2e', getURL: name => `chrome-extension://e2e/${name}`, onMessage: event('message'), onStartup: event('startup'), onInstalled: event('installed') },
      alarms: { async create() {}, onAlarm: event('alarm') },
      scripting: { async executeScript(request) {
        if (!request.args || request.args[0] === 'arm') return [{ documentId: 'fixture-document', result: { url: `https://chatgpt.com/c/${CONVERSATION}`, supported: true,
          busy: false, draft: false, blocked: false, userId: capturedUser, userText: capturedText, reply: capturedReply } }];
        assert.deepEqual(request.target.documentIds, ['fixture-document']);
        const text = request.args[1].text; sends.push(text);
        capturedUser = `u${sends.length + 1}`; capturedText = text;
        capturedReply = { id: `a${sends.length + 1}`, userId: capturedUser, terminal: true, text: '继续完成了本轮工作。' };
        return [{ result: { sent: true } }];
      } },
    };
    await import('../../extensions/chat-continuation/worker.mjs?real-backend');
    /** command 只走真实 popup 消息通道和生产 fetch，不替代 Server 决策。 */
    const command = (action, extra = {}) => new Promise(resolve => events.message({ action, tabId: 7, ...extra },
      { id: 'e2e', url: 'chrome-extension://e2e/popup.html' }, resolve));
    const result = await command('start', { options: { objective: '实现整个功能并验证', mode: 'goal', limit: 1, delay: 30, policy: { continuous: false } } });
    assert.equal(result.ok, true, result.error);
    assert.equal(sends.length, 1);
    assert.match(sends[0], /实现整个功能并验证/);
    assert.doesNotMatch(sends[0], /COS/);
    let listed = await command('list');
    const goal = listed.tasks[0];
    capturedReply.text = `已完成原请求和验证。\n[[WEBCODEX_GOAL:${goal.id}:${goal.generation}:COMPLETE]]`;
    await events.alarm({ name: 'webcodex-continuation' });
    assert.equal(sends.length, 1, '回执确认不重复发送');
    await events.alarm({ name: 'webcodex-continuation' });
    listed = await command('list');
    assert.equal(listed.tasks[0].state, 'done');
    assert.equal(sends.length, 1);

    // Loop 先在当前会话发送目标，再由同一会话明确报告 BLOCKED；不调用额外模型服务。
    const loop = await command('start', { options: { objective: '在原范围继续核验', mode: 'loop', limit: 3, delay: 30, policy: { continuous: false } } });
    assert.equal(loop.ok, true, loop.error);
    listed = await command('list');
    const loopTask = listed.tasks[0];
    capturedReply.text = `需要人工处理。\n[[WEBCODEX_GOAL:${loopTask.id}:${loopTask.generation}:BLOCKED]]`;
    await events.alarm({ name: 'webcodex-continuation' });
    await events.alarm({ name: 'webcodex-continuation' });
    listed = await command('list');
    assert.equal(listed.tasks[0].reason, 'chat_blocked'); assert.equal(sends.length, 2);

    // 已领取的 Continue 发送在 Server 重启后保留为未知，不能重放。
    const continued = await command('continue'); assert.equal(continued.ok, true, continued.error);
    assert.equal(sends.length, 2);
    await server.stop(); await server.start();
    listed = await command('list');
    assert.equal(listed.tasks[0].state, 'watching');
    await events.alarm({ name: 'webcodex-continuation' }); assert.equal(sends.length, 2);
    assert.equal(Object.keys(local).some(key => key === 'continuation.v1'), false);
  } finally { await server.close(); }
});

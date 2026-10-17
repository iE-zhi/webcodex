/** 新聊天预置与后台草稿持久化；独立内存存储模拟浏览器生命周期，不接触个人账户或页面。 */
import test from 'node:test';
import assert from 'node:assert/strict';
import { randomUUID } from 'node:crypto';
import { newChatLocation } from '../../extensions/chat-continuation/core.mjs';

/** 合成存储逐次复制值，夹具通过 gate 控制首次磁盘读取而非依赖随机延时。 */
function storage(data, gate = Promise.resolve()) {
  return { async get(key) { await gate; return { [key]: structuredClone(data[key]) }; },
    async set(values) { Object.assign(data, structuredClone(values)); } };
}

// 新聊天入口只能是明确根路由或 GPT 入口，搜索参数和历史/设置页面不能取得预置身份。
test('新聊天预置拒绝历史会话、搜索、凭据和其他站点', () => {
  assert.deepEqual(newChatLocation('https://chatgpt.com/'), { origin: 'https://chatgpt.com', prefix: '', conversation: null });
  assert.equal(newChatLocation('https://chatgpt.com/g/g-example').prefix, '/g/g-example');
  for (const url of ['https://chatgpt.com/settings', `https://chatgpt.com/c/${randomUUID()}`, 'https://chatgpt.com/?q=history', 'https://example.com/', 'https://name@chatgpt.com/']) assert.equal(newChatLocation(url), null);
});

// popup 输入在首次存储读取完成前关闭；worker 已接收的两次保存仍按顺序完成，最新内容不丢失。
test('后台串行草稿写入保留最新输入并限制字段与容量', async () => {
  let release;
  const gate = new Promise(resolve => { release = resolve; }), local = {};
  globalThis.chrome = { storage: { local: storage(local, gate) } };
  const { draftKey, readDraft, writeDraft } = await import('../../extensions/chat-continuation/drafts.mjs?draft-regression');
  const key = draftKey('http://127.0.0.1:8080', null, 0);
  const first = writeDraft(key, { objective: '最初输入', recovery: true });
  const last = writeDraft(key, { objective: '关闭前的最后输入', recovery: false });
  release(); await Promise.all([first, last]);
  assert.equal((await readDraft(key)).objective, '关闭前的最后输入');
  assert.equal((await readDraft(key)).recovery, false);
  assert.throws(() => writeDraft(key, { token: '不允许的凭据' }), /invalid_request/);
  assert.throws(() => writeDraft(key, { queueText: 'a'.repeat(8001) }), /invalid_request/);
  assert.throws(() => draftKey('https://example.com', 'not-a-conversation', 0), /invalid_request/);
  for (let tab = 1; tab <= 65; tab++) await writeDraft(draftKey('http://127.0.0.1:8080', null, tab), { objective: `${tab}` });
  assert.equal(Object.keys(local['continuation.form-drafts.v1']).length, 64);
  assert.equal(await readDraft(key), null);
});

// 只有明确预置+同文档真实首发意图+同一首条正文可关联；令牌只保存摘要，取消后不能消费旧授权。
test('预置按文档、首发正文、账户配置和取消身份隔离', async () => {
  const session = {};
  globalThis.chrome = { storage: { session: storage(session) } };
  const prearm = await import('../../extensions/chat-continuation/prearm.mjs?prearm-regression');
  const ids = { client: randomUUID(), browser: randomUUID() }, settings = { url: 'http://127.0.0.1:8080', token: 'wc_pat_' + 'x'.repeat(64) };
  const options = { objective: '', mode: 'goal', limit: 10, delay: 30 };
  const armed = await prearm.arm(7, 'same-document', 'https://chatgpt.com/', options, settings, ids);
  const snapshot = { documentId: 'same-document', page: { url: `https://chatgpt.com/c/${randomUUID()}`, supported: true, userId: 'first-user', userText: '首条真实请求', userCount: 1 } };
  assert.equal(await prearm.candidate(armed, snapshot, settings, ids), null);
  await prearm.intent(7, 'another-document', 'https://chatgpt.com/', '首条真实请求');
  assert.equal((await prearm.listPrearms())[0].intent, null);
  await prearm.intent(7, 'same-document', 'https://chatgpt.com/', '首条真实请求');
  const entry = (await prearm.listPrearms())[0];
  assert.equal((await prearm.candidate(entry, snapshot, settings, ids)).conversation, new URL(snapshot.page.url).pathname.split('/').at(-1));
  assert.equal(await prearm.candidate(entry, { ...snapshot, documentId: 'different-document' }, settings, ids), null);
  assert.equal(await prearm.candidate(entry, { ...snapshot, page: { ...snapshot.page, userText: '旧聊天的不同请求' } }, settings, ids), null);
  assert.equal(await prearm.candidate(entry, { ...snapshot, page: { ...snapshot.page, userCount: 2 } }, settings, ids), null);
  assert.equal(await prearm.candidate(entry, snapshot, { ...settings, token: 'new-account-token' }, ids), null);
  assert.equal(await prearm.candidate(entry, snapshot, settings, { ...ids, browser: randomUUID() }), null);
  assert.equal(await prearm.candidate({ ...entry, intent: { ...entry.intent, at: Date.now() - 120001 } }, snapshot, settings, ids), null);
  assert.doesNotMatch(JSON.stringify(session), /首条真实请求|wc_pat_/);
  await prearm.disarm(7);
  assert.equal(await prearm.consume(7, entry.id), false);
  assert.equal((await prearm.listPrearms()).length, 0);
});

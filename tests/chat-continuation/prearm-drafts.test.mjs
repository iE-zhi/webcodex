/** 新聊天预置与后台草稿持久化；独立内存存储模拟浏览器生命周期，不接触个人账户或页面。 */
import test from 'node:test';
import assert from 'node:assert/strict';
import { randomUUID } from 'node:crypto';
import { newChatLocation, sameChatScope } from '../../extensions/chat-continuation/core.mjs';

/** 合成存储逐次复制值，夹具通过 gate 控制首次磁盘读取而非依赖随机延时。 */
function storage(data, gate = Promise.resolve()) {
  return { async get(key) { await gate; return { [key]: structuredClone(data[key]) }; },
    async set(values) { Object.assign(data, structuredClone(values)); } };
}

// 新聊天入口包含明确项目首页，前缀与未来会话一致；搜索参数和历史/设置页面不能取得预置身份。
test('新聊天预置拒绝历史会话、搜索、凭据和其他站点', () => {
  assert.deepEqual(newChatLocation('https://chatgpt.com/'), { origin: 'https://chatgpt.com', prefix: '', conversation: null });
  assert.equal(newChatLocation('https://chatgpt.com/g/g-example').prefix, '/g/g-example');
  const prefix = '/g/g-p-' + 'a'.repeat(32);
  for (const ending of ['/project', '/project/']) assert.deepEqual(newChatLocation('https://chatgpt.com' + prefix + ending),
    { origin: 'https://chatgpt.com', prefix, conversation: null });
  assert.deepEqual(newChatLocation('https://chatgpt.com' + prefix + '-plotjuggler/project'),
    { origin: 'https://chatgpt.com', prefix: prefix + '-plotjuggler', conversation: null });
  for (const url of ['https://chatgpt.com/settings', `https://chatgpt.com/c/${randomUUID()}`, 'https://chatgpt.com/?q=history', 'https://example.com/', 'https://name@chatgpt.com/']) assert.equal(newChatLocation(url), null);
  for (const ending of ['/project/settings', '/project?q=history', '/project#history', '/project/extra']) assert.equal(newChatLocation('https://chatgpt.com' + prefix + ending), null);
  assert.equal(newChatLocation('https://chatgpt.com/g/g-example/project'), null);
});

// 只忽略同一固定项目 ID 的名称后缀；不同项目、根入口、普通 GPT 和嵌套路径不能被当成同一个范围。
test('固定项目ID允许名称后缀变化，其他聊天范围不放宽', () => {
  const prefix = '/g/g-p-' + 'a'.repeat(32);
  for (const suffix of ['', '-plotjuggler', '-renamed-project']) assert.equal(sameChatScope(prefix, prefix + suffix), true);
  for (const other of ['', '/g/g-p-' + 'b'.repeat(32) + '-plotjuggler', prefix + '-plotjuggler/extra', '/g/g-example']) assert.equal(sameChatScope(prefix, other), false);
  assert.equal(sameChatScope('/g/g-example', '/g/g-example-renamed'), false);
  assert.equal(sameChatScope(undefined, undefined), false);
});

// 项目首页预置只接管同文档中该项目的首条请求；根会话和其他项目即使正文相同也不能接管。
test('项目首页预置沿用项目绑定前缀并拒绝跨项目或根会话接管', async () => {
  const session = {};
  globalThis.chrome = { storage: { session: storage(session) } };
  const prearm = await import('../../extensions/chat-continuation/prearm.mjs?project-prearm');
  const ids = { client: randomUUID(), browser: randomUUID() }, settings = { url: 'http://127.0.0.1:8080', token: 'wc_pat_' + 'x'.repeat(64) };
  const prefix = '/g/g-p-' + 'a'.repeat(32), url = 'https://chatgpt.com' + prefix + '/project';
  await prearm.arm(7, 'project-document', url, { objective: '', mode: 'goal', limit: 10, delay: 30 }, settings, ids);
  await prearm.intent(7, 'project-document', url, '项目内首条任务');
  const entry = (await prearm.listPrearms())[0], conversation = randomUUID();
  assert.equal(entry.prefix, prefix);
  const snapshot = { documentId: 'project-document', page: { url: 'https://chatgpt.com' + prefix + '/c/' + conversation,
    supported: true, userId: 'first-user', userText: '项目内首条任务', userCount: 1 } };
  assert.equal((await prearm.candidate(entry, snapshot, settings, ids)).prefix, prefix);
  // 首页到会话时追加名称：接管后必须使用实际 URL 前缀，而不是把旧首页前缀交给 Server。
  const renamed = { ...snapshot, page: { ...snapshot.page, url: 'https://chatgpt.com' + prefix + '-plotjuggler/c/' + conversation } };
  assert.equal((await prearm.candidate(entry, renamed, settings, ids)).prefix, prefix + '-plotjuggler');
  assert.equal(await prearm.candidate(entry, { ...renamed, documentId: 'other-document' }, settings, ids), null);
  assert.equal(await prearm.candidate(entry, renamed, { ...settings, token: 'different-account' }, ids), null);
  for (const other of ['', '/g/g-p-' + 'b'.repeat(32)]) {
    assert.equal(await prearm.candidate(entry, { ...snapshot, page: { ...snapshot.page, url: 'https://chatgpt.com' + other + '/c/' + conversation } }, settings, ids), null);
  }
  // 预置后项目首页自身改名，真实首发仍须同文档、同固定项目身份和同一正文。
  await prearm.intent(7, 'project-document', 'https://chatgpt.com' + prefix + '-renamed/project', '改名后首条任务');
  // 新摘要必须已写入，不能仅靠旧意图仍然存在判定通过。
  const updated = (await prearm.listPrearms())[0];
  assert.notEqual(updated.intent.fingerprint, entry.intent.fingerprint);
  assert.ok(await prearm.candidate(updated, { ...renamed, page: { ...renamed.page, userText: '改名后首条任务' } }, settings, ids));
  await prearm.disarm(7);
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

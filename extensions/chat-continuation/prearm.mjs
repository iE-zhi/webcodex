/** 新聊天的显式预置授权；只在同文档真实首次发送后创建 Server 任务，不跟随历史会话导航。 */
import { newChatLocation, chatLocation, sameChatScope } from './core.mjs';

/** 预置只在当前浏览器会话存活，最多二十四小时；每安装最多十六个预置标签。 */
const KEY = 'continuation.prearms.v1', MAX_PREARMS = 16, ARM_MS = 86400000;
/** 人工首发意图最多两分钟，跨文档、导航或正文不符立即失效。 */
const INTENT_MS = 120000;
/** 串行存储更新由 worker 持有，popup 关闭不丢失已接收的启用或取消操作。 */
let writes = Promise.resolve();

/** 对公开首发正文生成不可逆比较摘要；不把人工正文写进预置存储。 */
async function hash(text) {
  return [...new Uint8Array(await crypto.subtle.digest('SHA-256', new TextEncoder().encode(text.trim())))].map(byte => byte.toString(16).padStart(2, '0')).join('');
}

/** 用单个 worker 写队列更新预置；失败传播给对应调用，下一次操作可重新读取。 */
function transaction(operation) {
  const pending = writes.then(async () => {
    const input = (await chrome.storage.session.get(KEY))[KEY];
    const entries = input && typeof input === 'object' && !Array.isArray(input) ? input : {};
    for (const [tab, value] of Object.entries(entries)) if (!value || Date.now() >= value.expiresAt) delete entries[tab];
    const result = await operation(entries);
    await chrome.storage.session.set({ [KEY]: entries });
    return result;
  });
  writes = pending.catch(() => { /* 原调用接收存储错误，后续操作仍能重新读取。 */ });
  return pending;
}

/** 保存明确授权的空白聊天；仅接收 popup 已提交的有界设置，不创建后台任务或自动输入。 */
export function arm(tab, documentId, url, options, settings, ids) {
  const route = newChatLocation(url);
  if (!route || typeof documentId !== 'string' || !documentId || documentId.length > 200
      || !options || !['continue', 'goal', 'loop'].includes(options.mode) || typeof options.objective !== 'string'
      || new TextEncoder().encode(options.objective).length > 8000 || !Number.isInteger(options.limit) || options.limit < 1 || options.limit > 10000
      || !Number.isInteger(options.delay) || options.delay < 30 || options.delay > 3600 || JSON.stringify(options).length > 16384) throw new Error('invalid_request');
  return transaction(async entries => {
    if (!entries[tab] && Object.keys(entries).length >= MAX_PREARMS) throw new Error('task_capacity');
    const entry = { id: crypto.randomUUID(), tab, documentId, ...route, options: structuredClone(options),
      server: settings.url, credential: await hash(settings.token), client: ids.client, browser: ids.browser,
      expiresAt: Date.now() + ARM_MS, intent: null };
    entries[tab] = entry; return entry;
  });
}

/** 取消精确标签的预置；已在途的 Server 启用还需由激活者核对 nonce 并暂停，不可继续投递。 */
export function disarm(tab) { return transaction(entries => { delete entries[tab]; }); }

/** 返回本浏览器的有界待处理预置，不返回凭据正文；账户令牌只保存摘要用于配置切换拒绝。 */
export function listPrearms() { return transaction(entries => structuredClone(Object.values(entries))); }

/** 真实首次发送只记录当前空白文档和正文摘要；没有显式预置时不产生任何授权。 */
export async function intent(tab, documentId, url, text) {
  const route = newChatLocation(url);
  if (!route || typeof text !== 'string' || new TextEncoder().encode(text).length > 131072) return;
  const fingerprint = await hash(text);
  return transaction(entries => {
    const entry = entries[tab];
    if (entry && entry.documentId === documentId && entry.origin === route.origin && sameChatScope(entry.prefix, route.prefix)) {
      entry.intent = { fingerprint, at: Date.now() };
    }
  });
}

/** 验证同文档、固定项目身份、账户配置及首条人工正文；允许项目名称后缀变化，返回实际会话前缀供 Server 精确绑定。 */
export async function candidate(entry, snapshot, settings, ids) {
  const route = chatLocation(snapshot.page.url);
  if (!entry.intent || !route || entry.expiresAt <= Date.now() || Date.now() - entry.intent.at > INTENT_MS
      || entry.documentId !== snapshot.documentId || entry.origin !== route.origin || !sameChatScope(entry.prefix, route.prefix)
      || entry.server !== settings.url || entry.client !== ids.client || entry.browser !== ids.browser
      || !snapshot.page.supported || !snapshot.page.userId || snapshot.page.userCount !== 1
      || await hash(settings.token) !== entry.credential || await hash(snapshot.page.userText) !== entry.intent.fingerprint) return null;
  return { ...ids, ...route, tab_id: entry.tab };
}

/** 激活返回后再次核对预置身份；用户已取消或替换预置时返回 false，调用方必须暂停刚创建的任务。 */
export function consume(tab, id) {
  return transaction(entries => {
    if (entries[tab]?.id !== id) return false;
    delete entries[tab]; return true;
  });
}

/** 记录固定激活失败供 popup 显示；身份改变才清除旧发送意图，网络未知结果保留幂等重试资格。 */
export function prearmFailure(tab, id, code, invalidate = false) {
  return transaction(entries => {
    if (entries[tab]?.id !== id) return;
    entries[tab].error = /^[a-z_]{1,64}$/.test(code) ? code : 'backend_unavailable';
    if (invalidate) entries[tab].intent = null;
  });
}

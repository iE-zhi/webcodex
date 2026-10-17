/** 受信扩展上下文的表单草稿仓库；worker 持有写入链，popup 关闭不会终止已接收的保存。 */
import { serverOrigin } from './core.mjs';

/** 本地只保存最近六十四个聊天的草稿，不包含 PAT、发送正文或任务授权。 */
const KEY = 'continuation.form-drafts.v1', MAX_DRAFTS = 64;
/** 已允许的表单值及最大字符串长度；不接受网页对象、原型字段或凭据字段。 */
const FIELDS = { objective: 2000, mode: 16, limit: 10, delay: 10, silenceSeconds: 10, lifetimeHours: 10,
  sessionId: 128, localRunner: 128, keepAwake: false, trigger: 16, recovery: false, continuous: false, adaptiveTiming: false, queueKind: 16, queueText: 8000 };
/** worker 生命周期内单个缓存及串行事务；失败由调用方显示，下次读写重新加载。 */
let loaded = null, writes = Promise.resolve();

/** 验证受信 popup 的草稿作用域；不允许任意网站、空标签或无界键。 */
export function draftKey(origin, conversation, tab) {
  if (!Number.isInteger(tab) || tab < 0 || conversation !== null && !/^[a-f\d]{8}(?:-[a-f\d]{4}){3}-[a-f\d]{12}$/i.test(conversation)) throw new Error('invalid_request');
  return `${serverOrigin(origin)}|${conversation || tab}`;
}

/** 只复制明确字段；非法值整次拒绝，不能悄悄裁剪人工输入。 */
function validate(values) {
  if (!values || typeof values !== 'object' || Array.isArray(values) || Object.keys(values).some(key => !Object.hasOwn(FIELDS, key))) throw new Error('invalid_request');
  const result = {};
  for (const [key, value] of Object.entries(values)) {
    const limit = FIELDS[key];
    if (limit === false ? typeof value !== 'boolean' : typeof value !== 'string' || value.length > limit) throw new Error('invalid_request');
    result[key] = value;
  }
  return result;
}

/** 加载一次有界旧格式仓库；无效历史值不授予权限，读取失败保留可见错误。 */
async function load() {
  if (!loaded) loaded = chrome.storage.local.get(KEY).then(result => {
    const entries = result[KEY];
    return entries && typeof entries === 'object' && !Array.isArray(entries) ? Object.fromEntries(Object.entries(entries).slice(-MAX_DRAFTS)) : {};
  }).catch(error => { loaded = null; throw error; });
  return loaded;
}

/** 读取本键已提交的草稿；等待此前保存完成，不访问后端或网页。 */
export async function readDraft(key) {
  await writes;
  const values = (await load())[key];
  return values ? validate(values) : null;
}

/** 入队时立刻捕获输入；worker 接收消息后保持通道到持久化完成，失败不会吞掉后续写入。 */
export function writeDraft(key, input) {
  const values = validate(input);
  const pending = writes.then(async () => {
    const stored = { ...await load() };
    delete stored[key]; stored[key] = values;
    const next = Object.fromEntries(Object.entries(stored).slice(-MAX_DRAFTS));
    await chrome.storage.local.set({ [KEY]: next });
    loaded = Promise.resolve(next);
  });
  // 当前请求仍接收原失败；这里只恢复串行链，下次操作重新读取最后一次成功的数据。
  writes = pending.catch(() => { loaded = null; });
  return pending;
}

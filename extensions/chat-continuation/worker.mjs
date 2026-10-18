/** 浏览器适配器：只上报公开回合和投递 Server 已决消息，不保存任务决策或执行本地续跑。 */
import { chatLocation, newChatLocation, sameChatScope, serverOrigin } from './core.mjs';
import { pageAction } from './page.mjs';
import { draftKey, readDraft, writeDraft } from './drafts.mjs';
import { arm, disarm, listPrearms, intent, candidate, consume, prearmFailure } from './prearm.mjs';

/** 续跑专用 Server origin 与 PAT；PAT 应仅授予 session:collaborate。 */
const CONNECTION = 'continuation.connection.v2';
/** 扩展安装 UUID，保留到用户清除扩展数据。 */
const CLIENT = 'continuation.client.v2';
/** 浏览器进程 UUID，重启后不能继承旧标签发送权。 */
const BROWSER = 'continuation.browser.v2';
/** 浏览器会话内真实 Stop 记录；按标签和主文档隔离，重启浏览器后清空。 */
const STOPS = 'continuation.stops.v1';
/** 浏览器观察周期为 30 秒，任务超时由 Server 独立维护。 */
const ALARM = 'webcodex-continuation';
/** 控制请求最多 25 秒；超时不重放已领取消息。 */
const HTTP_TIMEOUT_MS = 25000;
/** 只读页面快照最多等待五秒；超时的只读注入结果直接丢弃，不取得发送权。 */
const INSPECT_TIMEOUT_MS = 5000;
/** 后端 JSON 响应最多 2 MiB，覆盖三十二个任务的目标和有界队列摘要。 */
const MAX_RESPONSE_BYTES = 2 * 1024 * 1024;
/** 每批最多四个并发观察；同一轮覆盖所有任务，避免回执排在轮转队尾。 */
const MAX_TICK_TASKS = 4;
/** 运行期身份初始化、唤醒互斥、轮转游标和连接版本，不镜像后端任务状态。 */
let initialization = null, ticking = false, connectionEpoch = 0;
/** 仅缓存唤醒目标及更新时间，不缓存任务决策或发送权；最多三十二任务和十六预置标签。 */
let watchedTabs = new Set(), prearmedTabs = new Set(), inventoryAt = 0;
/** 唤醒目标缓存最多十六个预置标签；过期项只影响轮询机会，不授予启用或发送权限。 */
const MAX_PREARM_TARGETS = 16;
/** 最近六十四个已注入主文档；只减少重复注入开销，不缓存公开状态或发送权。 */
const observerDocuments = new Map(), MAX_OBSERVER_DOCUMENTS = 64;
/** 最多三十二个活动任务的页面读取诊断；只供面板显示，不保存任务状态或发送权。 */
const pageWaits = new Map(), MAX_PAGE_WAITS = 32;
/** 距最后一次有效页面观察超过九十秒后显示读取异常，不能无限显示正在加载。 */
const PAGE_LOADING_MS = 90000;

/** 记录有界的读取等待诊断；任务状态和授权截止时间仍由 Server 保持。 */
function waitForPage(task) {
  pageWaits.set(task.id, Date.now() - (task.observed_at || 0) > PAGE_LOADING_MS ? 'page_unreadable' : 'page_loading');
  if (pageWaits.size > MAX_PAGE_WAITS) pageWaits.delete(pageWaits.keys().next().value);
}

/** 页面模型未就绪时保留服务端授权并等待后续观察；完整正文超限仍须明确暂停。 */
async function pageReady(settings, task, snapshot) {
  if (snapshot.page.supported) { pageWaits.delete(task.id); return true; }
  if (snapshot.page.unsupportedReason === 'page_limit') {
    pageWaits.delete(task.id);
    await pause(settings, task, 'page_limit');
  } else {
    // 刷新后的 DOM/React 挂载没有固定时长；不观察残缺回合、不发送、不再次刷新。
    // 三十秒 alarm 和页面活动会自动复查，服务端原任务期限继续生效。
    waitForPage(task);
  }
  return false;
}

/** 记录一个有界预置唤醒目标；真正授权仍在会话存储和 Server 中重新检查。 */
function rememberPrearm(tab) {
  prearmedTabs.add(tab);
  if (prearmedTabs.size > MAX_PREARM_TARGETS) prearmedTabs.delete(prearmedTabs.values().next().value);
}

/** 幂等注入本扩展的隔离观察器；失败保留三十秒只读轮询，不向网页传递任何凭据。 */
async function enableObserver(tab, documentId = null) {
  if (documentId && observerDocuments.get(tab) === documentId) return;
  let timer;
  const expired = new Promise((_, reject) => { timer = setTimeout(() => reject(new Error('observer_unavailable')), INSPECT_TIMEOUT_MS); });
  try {
    const results = await Promise.race([chrome.scripting.executeScript({ target: { tabId: tab, ...(documentId ? { documentIds: [documentId] } : {}) }, world: 'ISOLATED', files: ['activity.js'] }), expired]);
    if (results.length === 1 && typeof results[0].documentId === 'string') {
      observerDocuments.set(tab, results[0].documentId);
      if (observerDocuments.size > MAX_OBSERVER_DOCUMENTS) observerDocuments.delete(observerDocuments.keys().next().value);
    }
  } catch { console.warn('Continuation live observer unavailable; polling retained.'); }
  finally { clearTimeout(timer); }
}

/** 更新已打开的前六十四个允许站点标签；其余活动任务在被观察前按需注入，不重载用户页面。 */
async function installObservers() {
  /** 单次安装最多扫描六十四个已打开标签，不对整个浏览器无限并发。 */
  const MAX_INSTALL_TABS = 64;
  const tabs = await chrome.tabs.query({ url: ['https://chatgpt.com/*', 'https://chat.openai.com/*'] });
  const available = tabs.slice(0, MAX_INSTALL_TABS).filter(tab => Number.isInteger(tab.id));
  for (let at = 0; at < available.length; at += MAX_TICK_TASKS) {
    await Promise.all(available.slice(at, at + MAX_TICK_TASKS).map(tab => enableObserver(tab.id)));
  }
}

/** 用刚读到的 Server 列表更新活动观察目标，所有真正动作仍重新查询服务端。 */
function rememberTargets(tasks) {
  watchedTabs = new Set(tasks.filter(task => !['paused', 'done'].includes(task.state)).map(task => task.binding.tab_id));
  const active = new Set(tasks.filter(task => !['paused', 'done'].includes(task.state)).map(task => task.id));
  for (const id of pageWaits.keys()) if (!active.has(id)) pageWaits.delete(id);
  inventoryAt = Date.now();
}
/** 当前安装内任务的取消版本；只保存最近 64 个 ID，阻止晚到准备或授权再次点击。 */
const cancellations = new Map();
/** 最近六十四个标签的停止记录和活动限流；不镜像任务状态或保存网页正文。 */
const stoppedTabs = new Map(), activityTimes = new Map();

/** 立即撤销本 worker 尚未点击的异步流程；不影响其他聊天任务。 */
function cancelLocal(id) {
  if (typeof id !== 'string') return;
  cancellations.set(id, (cancellations.get(id) || 0) + 1);
  if (cancellations.size > 64) cancellations.delete(cancellations.keys().next().value);
}

/** 撤销当前文档里尚在等待输入更新的投递；注入失败只记录固定诊断，Server 仍须撤销授权。 */
async function cancelPage(task) {
  if (!task?.delivery_id) return;
  await chrome.scripting.executeScript({ target: { tabId: task.binding.tab_id }, world: 'MAIN', func: pageAction,
    args: ['cancel', { id: task.delivery_id }] }).catch(() => console.warn('Continuation page cancellation unavailable.'));
}

/** 校验操作者指定的 Server origin；仅 loopback/私网 IPv4 允许 HTTP，不把 PAT 放进 URL。 */
function connectionInput(input) {
  if (!input || typeof input.url !== 'string' || typeof input.token !== 'string') throw new Error('connection_invalid');
  if (!/^wc_pat_[A-Za-z0-9_-]{32,2048}$/.test(input.token)) throw new Error('connection_invalid');
  return { url: serverOrigin(input.url), token: input.token };
}

/** 初始化可信存储和唯一身份；共享 Promise 防止并发创建两个安装 ID。 */
function identity() {
  if (!initialization) initialization = (async () => {
    await chrome.storage.local.setAccessLevel({ accessLevel: 'TRUSTED_CONTEXTS' });
    const stored = await chrome.storage.local.get(CLIENT), transient = await chrome.storage.session.get(BROWSER);
    const client = stored[CLIENT] || crypto.randomUUID(), browser = transient[BROWSER] || crypto.randomUUID();
    const stops = (await chrome.storage.session.get(STOPS))[STOPS] || {};
    for (const [tab, value] of Object.entries(stops).slice(-64)) {
      if (/^\d+$/.test(tab) && typeof value?.document === 'string' && value.document.length <= 200 && chatLocation(value.url) && Number.isFinite(value.at) && !stoppedTabs.has(Number(tab))) stoppedTabs.set(Number(tab), value);
    }
    if (!stored[CLIENT]) await chrome.storage.local.set({ [CLIENT]: client });
    if (!transient[BROWSER]) await chrome.storage.session.set({ [BROWSER]: browser });
    await chrome.alarms.create(ALARM, { periodInMinutes: 0.5 });
    return { client, browser };
  })().catch(error => { initialization = null; throw error; });
  return initialization;
}

/** 读取已验证连接；缺失时明确拒绝，不回退到浏览器自主续跑。 */
async function connection() {
  const value = (await chrome.storage.local.get(CONNECTION))[CONNECTION];
  if (!value) throw new Error('connection_required');
  return connectionInput(value);
}

/** 调用认证后端；流式限制响应大小，错误只提取安全代码。 */
async function api(settings, request, timeout = HTTP_TIMEOUT_MS) {
  const response = await fetch(`${settings.url}/api/continuations`, { method: 'POST', redirect: 'error', credentials: 'omit', cache: 'no-store',
    signal: AbortSignal.timeout(Math.max(1, Math.min(HTTP_TIMEOUT_MS, timeout))), headers: { 'Content-Type': 'application/json', Authorization: `Bearer ${settings.token}` }, body: JSON.stringify(request) });
  if (!response.body) throw new Error('backend_unavailable');
  const reader = response.body.getReader(), chunks = [];
  let total = 0;
  try {
    for (;;) {
      const { value, done } = await reader.read();
      if (done) break;
      total += value.byteLength;
      if (total > MAX_RESPONSE_BYTES) { await reader.cancel(); throw new Error('backend_response'); }
      chunks.push(value);
    }
  } finally { reader.releaseLock(); }
  const bytes = new Uint8Array(total); let offset = 0;
  for (const chunk of chunks) { bytes.set(chunk, offset); offset += chunk.byteLength; }
  const result = JSON.parse(new TextDecoder().decode(bytes));
  // 账户凭据失效与权限不足分别反馈，响应正文及令牌不进入错误信息。
  if (response.status === 401) throw new Error('pat_rejected');
  if (!response.ok) throw new Error(typeof result.error === 'string' && /^[a-z_]{1,64}$/.test(result.error) ? result.error : 'backend_unavailable');
  if (!Array.isArray(result.tasks) || result.tasks.length > 32) throw new Error('backend_response');
  return result;
}

/** 从确切主文档读取公开状态；documentId 只在本次发送使用。 */
async function inspect(tabId, action = 'snapshot', timeout = INSPECT_TIMEOUT_MS) {
  let timer;
  // Chrome 没有取消只读注入的 API；本地截止时间只丢弃晚到快照，不能用在发送脚本上。
  const expired = new Promise((_, reject) => { timer = setTimeout(() => reject(new Error('observer_unavailable')), Math.max(1, Math.min(INSPECT_TIMEOUT_MS, timeout))); });
  let results;
  try {
    results = await Promise.race([chrome.scripting.executeScript({ target: { tabId }, world: 'MAIN', func: pageAction, ...(action !== 'snapshot' ? { args: [action] } : {}) }), expired]);
  } finally { clearTimeout(timer); }
  if (results.length !== 1 || !results[0].documentId || !results[0].result) throw new Error('unsupported');
  return { page: { ...results[0].result, documentId: results[0].documentId }, documentId: results[0].documentId };
}

/** 只把相同 origin、会话 UUID 和固定项目 ID 视为同一会话；普通 GPT 路径仍须完全相同。 */
function matchesConversation(url, binding) {
  const route = chatLocation(url);
  return Boolean(route && route.conversation === binding.conversation && route.origin === binding.origin && sameChatScope(route.prefix, binding.prefix));
}

/** 转换公开观察字段；binding 可将同一项目的名称别名映射到服务端原绑定路径，其他身份变化一律拒绝。
 * 发送脚本仍用原始 page.url 和确切 documentId 核对，不用映射后的地址执行网页动作。 */
function observation(page, binding = null) {
  let url = page.url;
  if (binding) {
    if (!matchesConversation(url, binding)) throw new Error('page_changed');
    // Server 的持久化绑定不因项目名称变化而迁移；只转换已证实同一项目、同一会话的显示路径。
    if (chatLocation(url).prefix !== binding.prefix) url = `${binding.origin}${binding.prefix}/c/${binding.conversation}`;
  }
  return { url, supported: page.supported, busy: page.busy, draft: page.draft, blocked: page.blocked,
    document_id: page.documentId || '', interrupted: page.interrupted === true, tools_busy: page.toolsBusy === true, approval: page.approval === true, finish_id: null,
    activity: page.activity || {},
    source_proof: page.sourceProof || null,
    user_id: page.userId || '', user_text: page.userText || '', reply: page.reply ? {
      id: page.reply.id, user_id: page.reply.userId, terminal: page.reply.terminal, text: page.reply.text,
    } : null };
}

/** 从 Server 投影复制任务版本和绑定，网页文字不能替换它们。 */
function selector(task) {
  return { task_id: task.id, generation: task.generation, binding: task.binding };
}

/** 转换现有控件字段；pending 仅含投递 ID，不含未领取正文。 */
function view(task) {
  return { ...task.options, id: task.id, generation: task.generation, binding: task.binding, tabId: task.binding.tab_id,
    state: task.state, reason: task.reason, rounds: task.rounds, observedAt: task.observed_at || 0, localAt: task.local_at || 0, localStatus: task.local_status || 'ready',
    pageStatus: pageWaits.get(task.id) || '',
    sendAuthorized: task.send_authorized, nativeHeld: task.native_held, queue: task.queue || [], pending: task.delivery_id ? { id: task.delivery_id } : null };
}

/** 请求 Server 暂停任务；旧版本冲突交给后续查询，不重发浏览器消息。 */
async function pause(settings, task, reason) {
  return api(settings, { action: 'pause', ...selector(task), reason });
}

/** 查找确切会话；项目名称别名不改变身份，导航暂停须由当前标签地址确认，重开仍须领取预算。 */
async function locate(settings, task, browser) {
  let snapshot;
  try { snapshot = await inspect(task.binding.tab_id); } catch { /* 原标签关闭，继续查找同一会话。 */ }
  /** page 为只读快照或标签地址；身份按固定项目和会话判断，不按项目显示名称判断。 */
  const matches = page => matchesConversation(page.url, task.binding);
  if (snapshot && !matches(snapshot.page) && task.binding.browser === browser) {
    let tab;
    try { tab = await chrome.tabs.get(task.binding.tab_id); }
    catch { console.warn('Continuation navigation could not be confirmed; observation deferred.'); return null; }
    // 加载中的临时文档和晚到快照不能证明用户换了会话；等待下一次完整读取，不重建任务。
    const url = tab.pendingUrl || tab.url;
    if (tab.status !== 'complete' || !url || tab.pendingUrl && tab.pendingUrl !== tab.url || matches({ url }) || url !== snapshot.page.url) {
      waitForPage(task); return null;
    }
    await pause(settings, task, 'navigation'); return null;
  }
  if (!snapshot || !matches(snapshot.page) || task.binding.browser !== browser) {
    const url = `${task.binding.origin}${task.binding.prefix}/c/${task.binding.conversation}`;
    const tabs = await chrome.tabs.query({ url: `${task.binding.origin}/*` });
    const exact = tabs.filter(tab => matches({ url: tab.pendingUrl || tab.url || '' }));
    if (exact.length > 1) { await pause(settings, task, 'ambiguous_tabs'); return null; }
    if (!exact.length) {
      const permit = await api(settings, { action: 'repair', ...selector(task), kind: 'open', page: null });
      if (permit.repair === 'open') await chrome.tabs.create({ url, active: false });
      return null;
    }
    // 恢复后的唯一标签尚在加载时不再次注入或刷新；新浏览器身份也不能把正常加载计入修复次数。
    if (exact[0].status === 'loading' || exact[0].pendingUrl && exact[0].pendingUrl !== exact[0].url) {
      waitForPage(task); return null;
    }
    try { snapshot = await inspect(exact[0].id); }
    catch {
      const permit = await api(settings, { action: 'repair', ...selector(task), kind: 'wake', page: null });
      if (permit.repair === 'wake') await chrome.tabs.reload(exact[0].id);
      return null;
    }
    if (!matches(snapshot.page)) return null;
    if (!await pageReady(settings, task, snapshot)) return null;
    const replacement = { ...task.binding, browser, tab_id: exact[0].id };
    const rebound = await api(settings, { action: 'rebind', ...selector(task), replacement, page: observation(snapshot.page, task.binding) });
    task = rebound.tasks[0];
  }
  return { task, snapshot };
}

/** 点击后立即有界核对回执；失败保留未知投递，后续轮询继续对账，绝不再次点击。 */
async function receipt(settings, task) {
  /** 回执在前台最多等待三秒；后台观察不依赖此等待完成。 */
  const ATTEMPTS = 6, DELAY_MS = 500, BUDGET_MS = 3000;
  const deadline = Date.now() + BUDGET_MS;
  for (let attempt = 0; attempt < ATTEMPTS && Date.now() < deadline; attempt++) {
    const snapshot = await inspect(task.binding.tab_id, 'snapshot', deadline - Date.now());
    if (Date.now() >= deadline) return;
    const result = await api(settings, { action: 'observe', ...selector(task), page: observation(snapshot.page, task.binding) }, deadline - Date.now());
    if (!['dispatching', 'uncertain'].includes(result.tasks[0]?.state)) return;
    await new Promise(resolve => setTimeout(resolve, Math.min(DELAY_MS, Math.max(0, deadline - Date.now()))));
  }
}

/** 上报回合、领取并投递一次；刷新、重绑定和续跑均服从 Server 已提交状态。 */
async function processTask(settings, original, epoch) {
  const control = cancellations.get(original.id) || 0;
  const stopped = stoppedTabs.get(original.binding.tab_id);
  /** 所有可能点击的异步边界均检查连接和当前任务取消版本。 */
  const currentControl = () => epoch === connectionEpoch && control === (cancellations.get(original.id) || 0) && stoppedTabs.get(original.binding.tab_id) === stopped;
  const { browser } = await identity();
  const located = await locate(settings, original, browser);
  if (!located) return;
  let { task, snapshot } = located;
  await enableObserver(task.binding.tab_id, snapshot.documentId);
  const stop = stoppedTabs.get(task.binding.tab_id);
  if (stop?.document === snapshot.documentId && matchesConversation(stop.url, task.binding)) { await pause(settings, task, 'user_stopped'); return; }
  if (!await pageReady(settings, task, snapshot)) return;
  const observed = await api(settings, { action: 'observe', ...selector(task), page: observation(snapshot.page, task.binding) });
  const current = observed.tasks[0];
  if (!current || !currentControl()) return;
  if (current.state === 'running' && current.repair_at && current.repair_at <= Date.now() && !snapshot.page.reply?.terminal) {
    const permit = await api(settings, { action: 'repair', ...selector(current), kind: 'reload', page: observation(snapshot.page, current.binding) });
    if (permit.repair === 'reload') await chrome.tabs.reload(current.binding.tab_id);
    return;
  }
  if (current.state !== 'ready' || current.next_at > Date.now()) return;
  snapshot = await inspect(current.binding.tab_id);
  const claimed = await api(settings, { action: 'prepare', ...selector(current), delivery_id: current.delivery_id, page: observation(snapshot.page, current.binding) });
  if (!claimed.dispatch || !currentControl()) return;
  snapshot = await inspect(current.binding.tab_id);
  if (!currentControl()) return;
  const authorized = await api(settings, { action: 'authorize', ...selector(claimed.tasks[0]), delivery_id: claimed.dispatch.id, page: observation(snapshot.page, claimed.tasks[0].binding) });
  if (authorized.tasks[0]?.state !== 'dispatching' || !authorized.tasks[0]?.send_authorized) return;
  task = authorized.tasks[0];
  if (!currentControl()) {
    // 本流程尚未进入发送脚本，能够明确报告未点击；响应丢失或脚本异常不能走此证明。
    const cleared = await inspect(task.binding.tab_id);
    await api(settings, { action: 'not_sent', ...selector(task), delivery_id: claimed.dispatch.id, page: observation(cleared.page, task.binding) });
    return;
  }
  try {
    const result = await chrome.scripting.executeScript({ target: { tabId: current.binding.tab_id, documentIds: [snapshot.documentId] }, world: 'MAIN', func: pageAction,
      args: ['send', { url: snapshot.page.url, userId: snapshot.page.userId, userText: snapshot.page.userText,
        replyId: snapshot.page.reply?.id, replyText: snapshot.page.reply?.text, recovery: task.recovery_send,
        id: claimed.dispatch.id, text: claimed.dispatch.text }] });
    if (result.length !== 1 || result[0].result?.sent !== true) {
      if (result.length === 1 && result[0].result?.definite === true && result[0].result?.cleared === true) {
        const cleared = await inspect(task.binding.tab_id);
        await api(settings, { action: 'not_sent', ...selector(task), delivery_id: claimed.dispatch.id, page: observation(cleared.page, task.binding) });
      } else await pause(settings, task, 'send_failed');
      return;
    }
    await receipt(settings, task);
  } catch {
    // 点击可能已经成功，保持待回执，不撤销任务或重放消息；后续观察负责核对。
    console.warn('Continuation receipt unavailable; delivery retained without replay.');
  }
}

/** 每轮优先对账并覆盖全部任务；每批四个并发，单个异常不能阻塞其他会话。 */
async function tick(tabId = null) {
  if (ticking) return;
  ticking = true;
  try {
    const ids = await identity(), settings = await connection(), { client } = ids, epoch = connectionEpoch;
    const prearms = await listPrearms();
    prearmedTabs = new Set(prearms.map(entry => entry.tab));
    for (const entry of prearms.filter(entry => entry.intent && (tabId === null || entry.tab === tabId))) {
      await activatePrearm(entry, settings, ids, epoch);
    }
    const { tasks } = await api(settings, { action: 'list', client });
    rememberTargets(tasks);
    const active = tasks.filter(task => !['paused', 'done'].includes(task.state) && (tabId === null || task.binding.tab_id === tabId));
    active.sort((a, b) => Number(['dispatching', 'uncertain'].includes(b.state)) - Number(['dispatching', 'uncertain'].includes(a.state)));
    for (let offset = 0; offset < active.length; offset += MAX_TICK_TASKS) {
      await Promise.allSettled(active.slice(offset, offset + MAX_TICK_TASKS).map(async task => {
        try { await processTask(settings, task, epoch); }
        catch { console.warn('Continuation observation deferred; no message replayed.'); }
      }));
    }
  } finally { ticking = false; }
}

/** 同文档首发后幂等创建真正的 Server 任务；取消或配置切换后暂停晚到启用，不取得发送权。 */
async function activatePrearm(entry, settings, ids, epoch) {
  try {
    const snapshot = await inspect(entry.tab);
    if (newChatLocation(snapshot.page.url)) return;
    const binding = await candidate(entry, snapshot, settings, ids);
    if (!binding) { await prearmFailure(entry.tab, entry.id, 'prearm_navigation', true); return; }
    const started = await api(settings, { action: 'start', request_id: entry.id, task_id: null, generation: null,
      binding, options: entry.options, page: observation(snapshot.page) });
    const task = started.tasks[0];
    if (!task) throw new Error('backend_response');
    if (epoch !== connectionEpoch || !await consume(entry.tab, entry.id)) { await pause(settings, task, 'user'); return; }
    // 草稿跟随这次已验证的新会话；后端目标使用启用时明确提交的选项，不读取未提交草稿。
    const draft = await readDraft(draftKey(settings.url, null, entry.tab));
    if (draft) await writeDraft(draftKey(settings.url, binding.conversation, entry.tab), draft);
  } catch (error) { await prearmFailure(entry.tab, entry.id, error.message); }
}

/** 处理隔离脚本的心跳及真实 Stop；必须来自本扩展的顶层 ChatGPT 文档，不能传操作、目标或凭据。 */
async function pageActivity(message, sender) {
  if (!Number.isInteger(sender.tab?.id) || sender.frameId !== 0 || typeof sender.documentId !== 'string'
      || sender.documentId.length > 200 || !(chatLocation(sender.url) || newChatLocation(sender.url)) || typeof message.stop !== 'boolean') return;
  const tab = sender.tab.id;
  if (message.stop) {
    // 在第一次 await 前撤销该标签的晚到发送流程；存储失败仍保留内存屏障并返回固定错误。
    stoppedTabs.set(tab, { document: sender.documentId, url: sender.url, at: Date.now() });
    if (stoppedTabs.size > 64) stoppedTabs.delete(stoppedTabs.keys().next().value);
    await identity();
    await chrome.storage.session.set({ [STOPS]: Object.fromEntries(stoppedTabs) });
    const settings = await connection(), { client } = await identity();
    const { tasks } = await api(settings, { action: 'list', client });
    const route = chatLocation(sender.url);
    if (!route) return;
    for (const task of tasks.filter(task => task.binding.tab_id === tab && matchesConversation(sender.url, task.binding) && !['paused', 'done'].includes(task.state))) {
      cancelLocal(task.id); await cancelPage(task); await pause(settings, task, 'user_stopped');
    }
    return;
  }
  /** 每个标签最多每秒唤醒一次服务端读取，网页大量 DOM 变化不能无限增加请求。 */
  const GAP_MS = 1000;
  /** 未启用的聊天只在目标目录过期时触发读取，避免所有 ChatGPT 标签都每秒访问 SQLite。 */
  const INVENTORY_MS = 30000;
  if (Date.now() - inventoryAt < INVENTORY_MS && !watchedTabs.has(tab) && !prearmedTabs.has(tab)) return;
  if (Date.now() - (activityTimes.get(tab) || 0) < GAP_MS) return;
  activityTimes.set(tab, Date.now());
  if (activityTimes.size > 64) activityTimes.delete(activityTimes.keys().next().value);
  await tick(tab);
}

/** 处理 popup 的明确操作；Server 连接、账户或决策不可用时直接报告错误。 */
async function command(message) {
  if (['pause', 'delete', 'update', 'queue', 'dequeue'].includes(message.action)) cancelLocal(message.taskId);
  const ids = await identity();
  if (['form_read', 'form_write'].includes(message.action)) {
    const key = draftKey(message.origin, message.conversation, message.tabId);
    if (message.action === 'form_write') { await writeDraft(key, message.values); return {}; }
    return { draft: await readDraft(key) };
  }
  if (message.action === 'connection') {
    const saved = (await chrome.storage.local.get(CONNECTION))[CONNECTION];
    if (!saved) return { url: '', tokenSaved: false };
    const settings = connectionInput(saved);
    // popup 只获得保存状态和地址，不能读回完整 PAT。
    return { url: settings.url, tokenSaved: true };
  }
  if (message.action === 'configure') {
    const input = message.connection;
    const origin = serverOrigin(input?.url);
    let settings;
    if (input.token === '') {
      // 空白仅复用同一 origin 已保存的凭据，禁止把旧 PAT 带到新地址。
      settings = await connection();
      if (settings.url !== origin) throw new Error('connection_invalid');
    } else settings = connectionInput(input);
    const listed = await api(settings, { action: 'list', client: ids.client });
    const previous = (await chrome.storage.local.get(CONNECTION))[CONNECTION];
    if (previous?.url !== settings.url || previous?.token !== settings.token) {
      for (const entry of await listPrearms()) await disarm(entry.tab);
    }
    connectionEpoch++;
    pageWaits.clear();
    await chrome.storage.local.set({ [CONNECTION]: settings });
    rememberTargets(listed.tasks);
    return { tasks: listed.tasks.map(view), connected: true, url: settings.url, tokenSaved: true };
  }
  const settings = await connection(), listed = await api(settings, { action: 'list', client: ids.client });
  rememberTargets(listed.tasks);
  if (['pause', 'delete', 'update', 'queue', 'dequeue'].includes(message.action)) await cancelPage(listed.tasks.find(task => task.id === message.taskId));
  if (message.action === 'list') {
    const pending = (await listPrearms()).find(entry => entry.tab === message.tabId && entry.server === settings.url && entry.browser === ids.browser);
    if (pending) rememberPrearm(message.tabId);
    return { tasks: listed.tasks.map(view), prearm: pending ? { options: pending.options, error: pending.error || '' } : null, connected: true, url: settings.url };
  }
  if (message.action === 'delete') {
    const task = listed.tasks.find(task => task.id === message.taskId);
    if (task) await api(settings, { action: 'delete', task_id: task.id, client: ids.client });
    return {};
  }
  if (['queue', 'dequeue'].includes(message.action)) {
    const task = listed.tasks.find(task => task.id === message.taskId);
    if (!task) throw new Error('missing');
    const request = message.action === 'dequeue' ? { action: 'dequeue', ...selector(task), entry_id: message.entryId }
      : { action: 'queue', ...selector(task), kind: message.kind, text: message.text || '', stages: message.stages || [], trigger: message.trigger || 'after_turn' };
    await api(settings, request);
    return {};
  }
  if (!Number.isInteger(message.tabId) || message.tabId < 0) throw new Error('tab');
  if (message.action === 'pause') {
    const task = listed.tasks.find(task => task.id === message.taskId);
    if (!task) { await disarm(message.tabId); prearmedTabs.delete(message.tabId); return {}; }
    await pause(settings, task, 'user'); return {};
  }
  if (message.action === 'update') {
    const task = listed.tasks.find(task => task.id === message.taskId);
    if (!task) throw new Error('missing');
    const snapshot = await inspect(task.binding.tab_id);
    await api(settings, { action: 'update', ...selector(task), options: message.options, page: observation(snapshot.page, task.binding) });
    return {};
  }
  if (!['start', 'continue'].includes(message.action)) throw new Error('action');
  stoppedTabs.delete(message.tabId);
  await chrome.storage.session.set({ [STOPS]: Object.fromEntries(stoppedTabs) });
  const snapshot = await inspect(message.tabId, 'arm'), location = chatLocation(snapshot.page.url);
  const options = message.action === 'continue' ? { objective: '', limit: 100, delay: 30, ...message.options, mode: 'continue' } : message.options;
  if (!location) {
    if (!snapshot.page.supported || !newChatLocation(snapshot.page.url) || snapshot.page.busy || snapshot.page.userId) throw new Error('unsupported');
    await enableObserver(message.tabId, snapshot.documentId);
    await arm(message.tabId, snapshot.documentId, snapshot.page.url, options, settings, ids); rememberPrearm(message.tabId); return {};
  }
  // 已有聊天启用前先解释确定的本地阻挡；不清除草稿，也不把启用拒绝伪装成任务版本冲突。
  // 新聊天预置在上方单独处理，允许用户保留尚未首次发送的输入。
  if (snapshot.page.draft) throw new Error('page_draft');
  if (!snapshot.page.supported) throw new Error(snapshot.page.unsupportedReason === 'page_limit' ? 'page_limit' : 'unsupported');
  const matching = listed.tasks.filter(task => matchesConversation(snapshot.page.url, task.binding));
  // 同一会话可能保留旧的暂停记录；正在运行的唯一任务优先，不能误建第二个任务。
  const task = matching.find(task => !['paused', 'done'].includes(task.state)) || matching[0];
  const binding = { ...ids, ...location, prefix: task?.binding.prefix ?? location.prefix, tab_id: message.tabId };
  const started = await api(settings, { action: 'start', request_id: crypto.randomUUID(), task_id: task?.id || null,
    generation: task?.generation ?? null, binding, options, page: observation(snapshot.page, binding) });
  if (started.tasks[0]) { watchedTabs.add(message.tabId); await processTask(settings, started.tasks[0], connectionEpoch); }
  return {};
}

// 仅受信 popup 可以控制；网页及 content script 没有 PAT 或启停权。
chrome.runtime.onMessage.addListener((message, sender, respond) => {
  if (sender.id === chrome.runtime.id && sender.tab && ['webcodex_intent', 'webcodex_prearm_cancel'].includes(message?.event)) {
    if (sender.frameId !== 0 || !Number.isInteger(sender.tab.id) || typeof sender.documentId !== 'string' || sender.documentId.length > 200 || !(chatLocation(sender.url) || newChatLocation(sender.url))) return false;
    const pending = message.event === 'webcodex_intent' ? intent(sender.tab.id, sender.documentId, sender.url, message.text) : disarm(sender.tab.id);
    pending.then(() => respond({ ok: true }), () => respond({ ok: false, error: 'storage_error' })); return true;
  }
  if (sender.id === chrome.runtime.id && sender.tab && message?.event === 'webcodex_activity') {
    pageActivity(message, sender).then(() => respond({ ok: true }), () => respond({ ok: false, error: 'observer_unavailable' }));
    return true;
  }
  if (sender.id !== chrome.runtime.id || sender.tab || sender.url !== chrome.runtime.getURL('popup.html')) return false;
  command(message).then(result => respond({ ok: true, ...result }), error => respond({ ok: false, error: /^[a-z_]{1,64}$/.test(error.message) ? error.message : 'backend_unavailable' }));
  return true;
});
// alarm 只提供观察机会；任务完成、预算和失败恢复由 Server 决定。
chrome.alarms.onAlarm.addListener(alarm => {
  if (alarm.name === ALARM) return tick().catch(() => console.warn('Continuation backend unavailable; no local fallback.'));
});
// 重启和更新只恢复身份及观察；旧浏览器绑定在下次观察时暂停。
chrome.runtime.onStartup.addListener(() => { identity().then(installObservers).catch(() => console.error('Continuation observer initialization unavailable.')); });
chrome.runtime.onInstalled.addListener(() => { identity().then(installObservers).catch(() => console.error('Continuation observer initialization unavailable.')); });

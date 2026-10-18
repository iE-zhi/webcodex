/** 扩展单栏控制面板；中文默认，可切英文，所有正文通过 textContent 渲染。 */
import { chatLocation, newChatLocation, serverOrigin } from './core.mjs';

/** 完整的中英文界面词典；错误码不含网页、路径或凭据内容。 */
const COPY = {
  zh: {
    source_confirmation_required: '正在确认当前主聊天来源',
    localRunner: '本机 Runner ID（可选）', keepAwake: '活动任务期间防休眠', runnerHint: '需在本机 Runner 的 continuation 配置中启用浏览器恢复。',
    power_unavailable: '本机电源控制失败，请检查 Runner 日志', browser_unavailable: '本机浏览器恢复失败，请检查 Runner 日志', lease_unavailable: '本机租约不可用', protocol_error: '本机租约校验失败',
    runner_stale: '本机 Runner 尚未接管，请检查其配置', runner_access_denied: '无权关联此 Runner，请检查本账户的 Runner ID',
    prearmed: '已预置，等待你发送第一条消息', prearm_navigation: '预置未接管新会话，请检查页面后重新启用',
    adaptiveTiming: '按模型调整等待',
    native_running: '当前聊天内续跑', native_finish_required: '当前聊天尚未完成原生交接，请检查回复',
    native_receipt_unknown: '工具结果是否收到未知，请核对队列', after_turn_queue: '等待本轮结束后投递队列',
    loop_stop_refused: 'Loop 多次拒绝继续，已停止本次循环', queue_native: '已交给当前聊天', queue_uncertain: '接收未知，先核对',
    queueTitle: '排队输入与工作流', queueKind: '输入类型', queueMessage: '人工输入', queueWorkflow: '完整工作流（每行一个阶段）',
    queueText: '排队内容', queueAdd: '加入队列', queuePlan: '当前 ChatGPT 规划', queueEntries: '等待投递', queued_input: '等待投递排队输入', plan_invalid: '规划格式无效，已暂停，请检查当前回复',
    continuous: '保持本会话启用', update: '更新当前任务', watching: '本轮结束，等待新请求', prepared: '准备发送，尚未授权',
    user_stopped: '你已停止生成，自动续跑已暂停', page_limit: '公开消息无法完整读取，请检查会话', no_progress: '连续三轮答案相同，已暂停',
    page_loading: '等待页面加载，恢复后自动续跑',
    page_unreadable: '页面暂时无法读取，自动重试中',
    conversation_load_failed: '对话加载失败，按恢复预算自动重试',
    transport_interrupted: 'ChatGPT 传输已中断，等待恢复',
    rate_limited: '等待 ChatGPT 额度或访问限制解除', observer_stale: '页面观察已超时，请检查浏览器', turn_complete: '本轮已结束',
    not_sent_retry: '已确认未发送，等待重试', prepare_expired: '准备已过期，等待重新核对', observer_unavailable: '页面观察器不可用',
    recoverySettings: '恢复与触发', recovery: '自动恢复中断', silenceSeconds: '基础静默等待（秒）', lifetimeHours: '有效期（小时）',
    sessionId: '关联 Workflow Session（可选）', trigger: '触发时机', afterTurn: '每回合结束', afterFinish: 'finish 工具后',
    sessionHint: '可留空，由当前 ChatGPT 自动关联 Session；finish 在原生工具中交接后续工作。',
    recovery_exhausted: '恢复次数已用完，请检查会话后重新启用',
    uncertain: '正在核对发送回执', awaiting_receipt: '等待确切回执，不重复发送', recovering: '恢复中断中', approval_required: '等待人工批准或回答',
    ambiguous_tabs: '同一会话有多个标签，请保留一个后重新启用', runtime_busy: '后台任务仍在执行', runtime_unavailable: '无法确认后台任务状态',
    session_access_denied: '无权关联此 Session，请使用该任务的凭据', session_unavailable: '关联 Session 不可用',
    connection: '后端连接', serverUrl: 'Server 地址', serverToken: '会话协作 PAT', connect: '保存并连接',
    checking: '正在检查后端连接…', connected: '已连接后端', disconnected: '尚未连接后端', connection_failed: '连接失败',
    reconnect: '验证连接', connecting: '正在连接…', token_saved: 'PAT 已保存；留空可复用，输入新值可替换。',
    token_needed: '粘贴账户 PAT，连接成功后保存。', token_placeholder: '粘贴 wc_pat_…', token_saved_placeholder: '已保存 PAT',
    connection_required: '请连接后端，续跑由 Server 管理。', connection_invalid: '地址或 PAT 无效；HTTP 仅允许本机或私网 IPv4 地址。',
    permission_denied: '未允许访问该 Server 地址。', backend_unavailable: '后端不可用，未发送续跑消息。', backend_response: '后端响应无效。',
    continuation_account_required: '请使用具有 session:collaborate 权限的账户 PAT。',
    pat_rejected: 'PAT 无效或已失效，请重新粘贴。',
    ready: '等待投递', dispatching: '等待接收确认', goal_complete: '目标已完成',
    state_conflict: '任务状态已变化，请重新打开扩展检查。', invalid_request: '目标、页面或预算无效。', context_limit: '后端上下文达到上限，已暂停。',
    storage_error: '后端持久化失败，未投递新消息。', task_capacity: '后端任务数已满，请移除旧任务。', task_not_found: '后端任务已删除。', page_changed: '领取前页面发生变化，已暂停。',
    title: '长任务续跑', objective: '任务目标', mode: '续跑方式', goal: 'Goal · 完成后停止', loop: 'Loop · 持续推进',
    limit: '最多续跑轮数', delay: '间隔（秒）', start: '启用当前会话', pause: '暂停', continue: 'Continue · 自动恢复',
    tasks: '已保存任务', remove: '移除', empty: '暂无任务', loading: '读取中…', idle: '请打开 ChatGPT 聊天。', inactive: '当前会话尚未启用续跑。',
    boundary: '当前 ChatGPT 自行判断进度；自动发送占用当前账户额度，人工审批保持等待，需要补充信息时停止。',
    goal_marker_missing: '未识别到本轮状态标记，已暂停；请检查回复后重新启用。', protocol_updated: '续跑协议已更新，请检查会话后重新启用。', control_busy: '后端繁忙，请稍后重试。',
    running: '续跑已启用', paused: '已暂停', done: '已停止',
    user: '手动暂停', expired: '有效期已到', navigation: '会话已切换', unsupported: '页面结构无法确认，请刷新页面或更新扩展',
    delivery_unknown: '发送结果不确定，请核对会话后重新启用', user_changed: '收到新消息，请核对目标后重新启用',
    page_draft: '聊天输入框有未发送内容，请先发送后再启用续跑。',
    page_blocked: '页面提示错误或需要操作', chat_blocked: '当前会话需要人工处理',
    round_limit: '已到轮数上限', send_failed: '发送未完成，请检查草稿', unavailable: '原标签页不可用',
    browser_restart: '浏览器已重启，请检查后重新启用',
    missing: '当前会话没有任务', tab: '无法读取当前标签', action: '不支持的操作', generic: '操作失败，请重新打开扩展检查状态',
  },
  en: {
    source_confirmation_required: 'Confirming the current main chat source',
    localRunner: 'Local Runner ID (optional)', keepAwake: 'Prevent sleep during active tasks', runnerHint: 'Enable browser recovery in the local Runner continuation config.',
    power_unavailable: 'Local power control failed; review Runner logs', browser_unavailable: 'Local browser recovery failed; review Runner logs', lease_unavailable: 'Local lease unavailable', protocol_error: 'Local lease validation failed',
    runner_stale: 'Local Runner has not taken over; review its config', runner_access_denied: 'Runner association denied; check the ID owned by this account',
    prearmed: 'Armed; awaiting your first message', prearm_navigation: 'New-chat association failed; review the page and enable again',
    adaptiveTiming: 'Adjust wait by model',
    native_running: 'Continuing inside current chat', native_finish_required: 'Native handoff is unfinished; review the reply',
    native_receipt_unknown: 'Tool result receipt unknown; review the queue', after_turn_queue: 'Queue delivery awaits the end of this turn',
    loop_stop_refused: 'Loop repeatedly refused to continue; this run stopped', queue_native: 'Handed to current chat', queue_uncertain: 'Receipt unknown; review first',
    queueTitle: 'Queued input and workflow', queueKind: 'Input type', queueMessage: 'Your message', queueWorkflow: 'Full workflow (one stage per line)',
    queueText: 'Queued content', queueAdd: 'Add to queue', queuePlan: 'Plan in current ChatGPT', queueEntries: 'Pending delivery', queued_input: 'Queued input ready', plan_invalid: 'Invalid plan format; paused, review the reply',
    continuous: 'Keep enabled for this chat', update: 'Update current task', watching: 'Turn ended; awaiting your next request', prepared: 'Prepared; not authorized',
    user_stopped: 'You stopped generation; automation paused', page_limit: 'Cannot read the full public message; review the chat', no_progress: 'Same answer for three rounds; paused',
    page_loading: 'Waiting for the page; continuation resumes automatically',
    page_unreadable: 'Page unreadable; retrying automatically',
    conversation_load_failed: 'Conversation failed to load; retrying within the recovery budget',
    transport_interrupted: 'ChatGPT transport interrupted; awaiting recovery',
    rate_limited: 'Waiting for ChatGPT access or usage limits', observer_stale: 'Page observation timed out; check the browser', turn_complete: 'Turn completed',
    not_sent_retry: 'Confirmed not sent; awaiting retry', prepare_expired: 'Preparation expired; checking again', observer_unavailable: 'Page observer unavailable',
    recoverySettings: 'Recovery and triggers', recovery: 'Recover interrupted work', silenceSeconds: 'Base silence wait (seconds)', lifetimeHours: 'Lifetime (hours)',
    sessionId: 'Workflow Session (optional)', trigger: 'Trigger', afterTurn: 'After each turn', afterFinish: 'After finish tool',
    sessionHint: 'Optional: current ChatGPT links its Session automatically. Native finish hands off the next work.',
    recovery_exhausted: 'Recovery limit reached; review the chat before enabling again',
    uncertain: 'Checking delivery receipt', awaiting_receipt: 'Awaiting exact receipt; no replay', recovering: 'Recovering interrupted work', approval_required: 'Your approval or answer is required',
    ambiguous_tabs: 'Keep one tab for this chat before enabling again', runtime_busy: 'Background work is still running', runtime_unavailable: 'Background status unavailable',
    session_access_denied: 'Session access denied; use the task credential', session_unavailable: 'Linked Session unavailable',
    connection: 'Backend connection', serverUrl: 'Server origin', serverToken: 'Session collaboration PAT', connect: 'Save and connect',
    checking: 'Checking backend connection…', connected: 'Connected to backend', disconnected: 'Backend not connected', connection_failed: 'Connection failed',
    reconnect: 'Check connection', connecting: 'Connecting…', token_saved: 'PAT saved. Leave blank to reuse, or enter a replacement.',
    token_needed: 'Paste your account PAT. It is saved after a successful connection.', token_placeholder: 'Paste wc_pat_…', token_saved_placeholder: 'PAT saved',
    connection_required: 'Connect the backend. Server owns task continuation.', connection_invalid: 'Invalid origin or PAT. HTTP requires loopback or a private IPv4 address.',
    permission_denied: 'Server origin access was not granted.', backend_unavailable: 'Backend unavailable; nothing was sent.', backend_response: 'Invalid backend response.',
    continuation_account_required: 'Use an account PAT with session:collaborate scope.',
    pat_rejected: 'The PAT is invalid or expired. Paste a valid PAT.',
    ready: 'Waiting to send', dispatching: 'Awaiting receipt', goal_complete: 'Goal completed',
    state_conflict: 'Task state changed; reopen the popup to review.', invalid_request: 'Invalid objective, page or budget.', context_limit: 'Backend context limit reached; paused.',
    storage_error: 'Backend persistence failed; no new message sent.', task_capacity: 'Backend task limit reached; remove an old task.', task_not_found: 'Backend task was deleted.', page_changed: 'Page changed before claim; paused.',
    title: 'Long tasks', objective: 'Task objective', mode: 'Mode', goal: 'Goal · stop when done', loop: 'Loop · keep working',
    limit: 'Maximum rounds', delay: 'Interval (seconds)', start: 'Enable this chat', pause: 'Pause', continue: 'Continue · recover',
    tasks: 'Saved tasks', remove: 'Remove', empty: 'No saved tasks', loading: 'Loading…', idle: 'Open an existing ChatGPT conversation.', inactive: 'Continuation is off for this chat.',
    boundary: 'The current ChatGPT chat judges progress. Automatic messages use your account allowance; tasks pause for your input.',
    goal_marker_missing: 'The reply has no valid status marker. Review it before enabling again.', protocol_updated: 'Continuation protocol updated. Review the chat before enabling again.', control_busy: 'Backend busy. Try again shortly.',
    running: 'Continuation enabled', paused: 'Paused', done: 'Stopped',
    user: 'Paused by you', expired: 'Run expired', navigation: 'Conversation changed', unsupported: 'Page structure not recognized; reload the page or update the extension',
    delivery_unknown: 'Delivery uncertain; check the chat before enabling again', user_changed: 'New message; review the objective before enabling again',
    page_draft: 'The chat composer has an unsent draft. Send it before enabling continuation.',
    page_blocked: 'Page error or action required', chat_blocked: 'The chat needs your input',
    round_limit: 'Round limit reached', send_failed: 'Send not completed; check the draft', unavailable: 'Original tab unavailable',
    browser_restart: 'Browser restarted; review before enabling again',
    missing: 'No task for this chat', tab: 'Current tab unavailable', action: 'Unsupported action', generic: 'Operation failed; reopen the extension to check its state',
  },
};
/** popup 生命周期内的语言和标签状态；不保存自动发送权或网页正文。 */
let locale = 'zh', tabId = null, conversation = null, tasks = [], current = null, prearm = null, working = true, connected = false, refreshing = false;
/** 已保存凭据的 origin 和存在标志；popup 永不读回已保存的 PAT。 */
let savedUrl = '', tokenSaved = false;
/** 连接显示状态和安全错误码；与当前会话任务状态分别渲染。 */
let connectionState = 'checking', connectionError = '', actionError = '';
/** 每次连接尝试递增；忽略此前已发出查询的过期结果。 */
let connectionRevision = 0;
/** 未提交连接草稿只存浏览器会话可信存储；连接成功后删除，不发送给网页。 */
const CONNECTION_DRAFT = 'continuation.connection-draft.v1';
/** 表单状态和本地控件的明确映射；只读取这些字段，不反序列化任意页面对象。 */
const FORM_FIELDS = ['objective', 'mode', 'limit', 'delay', 'silenceSeconds', 'lifetimeHours', 'sessionId', 'localRunner', 'keepAwake', 'trigger', 'recovery', 'continuous', 'adaptiveTiming', 'queueKind', 'queueText'];

/** 读取当前控件的目标与策略；Continue 与 Goal 使用同一设置来源。 */
function optionsFromForm() {
  return { objective: element('objective').value, mode: element('mode').value,
    limit: Number(element('limit').value), delay: Number(element('delay').value),
    policy: { recovery: element('recovery').checked, continuous: element('continuous').checked, adaptive_timing: element('adaptiveTiming').checked,
      runner_id: element('localRunner').value.trim() || null, keep_awake: Boolean(element('localRunner').value.trim()) && element('keepAwake').checked,
      silence_seconds: Number(element('silenceSeconds').value), lifetime_hours: Number(element('lifetimeHours').value),
      trigger: element('trigger').value, session_id: element('sessionId').value.trim() || current?.policy?.session_id || null } };
}

/** 按 Server 和当前聊天保存有界表单草稿，存储失败直接显示，不清空用户输入。 */
function saveFormDraft() {
  if (!savedUrl || tabId === null) return;
  const values = Object.fromEntries(FORM_FIELDS.map(id => [id, element(id).type === 'checkbox' ? element(id).checked : element(id).value]));
  request('form_write', tabId, { origin: savedUrl, conversation, values }).catch(() => { actionError = 'storage_error'; render(); });
}
/** 从受控 HTML 读取唯一元素，调用方只使用固定 id。 */
const element = id => document.getElementById(id);

/** 返回已本地化的安全状态或通用错误，不展示未识别的异常正文。 */
function label(key) {
  return COPY[locale][key] || COPY[locale].generic;
}

/** 只有表单地址与保存地址相同才允许空 PAT；无效或新地址必须提供新凭据。 */
function canReuseToken() {
  try { return tokenSaved && serverOrigin(element('serverUrl').value) === savedUrl; }
  catch { return false; /* 尚未输入完整地址，不复用任何凭据。 */ }
}

/** 输入即保存有界草稿，关闭 popup 后可恢复；存储失败可见，禁止静默丢失输入。 */
function saveConnectionDraft() {
  chrome.storage.session.set({ [CONNECTION_DRAFT]: { url: element('serverUrl').value, token: element('serverToken').value } }).catch(() => {
    actionError = 'generic'; render();
  });
}

/** 渲染连接、凭据保存状态及按钮；输入值不随刷新重置。 */
function renderConnection() {
  const reuse = canReuseToken();
  const status = element('connectionStatus');
  status.dataset.state = connectionState;
  status.textContent = connectionState === 'failed' ? `${label('connection_failed')} · ${label(connectionError)}`
    : connectionState === 'connected' ? `${label('connected')} · ${savedUrl}`
    : label({ checking: 'checking', connected: 'connected', unconfigured: 'disconnected' }[connectionState]);
  element('tokenHint').textContent = label(reuse ? 'token_saved' : 'token_needed');
  element('serverToken').placeholder = label(reuse ? 'token_saved_placeholder' : 'token_placeholder');
  element('serverToken').required = !reuse;
  element('connect').textContent = label(working && connectionState === 'checking' ? 'connecting' : reuse ? 'reconnect' : 'connect');
  for (const id of ['serverUrl', 'serverToken', 'connect']) element(id).disabled = working;
  element('error').hidden = !actionError;
  element('error').textContent = actionError ? label(actionError) : '';
}

/** 将当前语言应用于静态文案与状态；不创建自绘下拉菜单。 */
function render() {
  document.documentElement.lang = locale === 'zh' ? 'zh-CN' : 'en';
  document.querySelectorAll('[data-text]').forEach(node => { node.textContent = label(node.dataset.text); });
  element('language').textContent = locale === 'zh' ? 'EN' : '中文';
  element('tasks').setAttribute('aria-label', label('tasks'));
  renderConnection();
  element('status').textContent = current ? `${label(current.state)} · ${current.rounds}/${current.limit}${current.reason ? ` · ${label(current.reason)}` : ''}` : label(prearm ? prearm.error || 'prearmed' : tabId === null ? 'idle' : 'inactive');
  const active = prearm || current && !['paused', 'done'].includes(current.state);
  if (current?.nativeHeld && current.state === 'running') element('status').textContent = `${label('native_running')} · ${current.rounds}/${current.limit}`;
  if (active && current?.pageStatus) element('status').textContent += ` · ${label(current.pageStatus)}`;
  /** 页面超过三次正常观察周期没有有效回报时，显示观察异常而不是继续声称健康运行。 */
  const STALE_MS = 90000;
  if (active && current?.observedAt && Date.now() - current.observedAt > STALE_MS) element('status').textContent += ` · ${label('observer_stale')}`;
  /** 防休眠租约最长四十五秒；仅展示接管时间，不能把它当作操作系统成功执行的证明。 */
  const RUNNER_STALE_MS = 45000;
  if (current?.policy?.runner_id && active && (!current.localAt || Date.now() - current.localAt > RUNNER_STALE_MS)) element('status').textContent += ` · ${label('runner_stale')}`;
  if (current?.policy?.runner_id && active && current.localStatus && current.localStatus !== 'ready') element('status').textContent += ` · ${label(current.localStatus)}`;
  element('keepAwake').disabled = !element('localRunner').value.trim();
  element('start').textContent = label(active ? 'update' : 'start');
  element('start').disabled = working || !connected || tabId === null;
  element('pause').disabled = working || !active;
  element('continue').disabled = working || !connected || tabId === null || active;
  element('remove').disabled = working || tasks.length === 0;
  for (const id of ['queueAdd', 'queuePlan']) element(id).disabled = working || !connected || !current;
  element('queueRemove').disabled = working || !connected || !current?.queue?.length;
  const selectedQueue = element('queueEntries').value;
  element('queueEntries').replaceChildren(...(current?.queue?.length ? current.queue.map(entry => {
    const option = document.createElement('option'); option.value = entry.id;
    option.textContent = `${entry.preview}${entry.delivery === 'native' || entry.delivery === 'uncertain' ? ` · ${label(`queue_${entry.delivery}`)}` : ''}`;
    option.selected = entry.id === selectedQueue; return option;
  }) : [new Option(label('empty'), '')]));
  element('tasks').replaceChildren(...(tasks.length ? tasks.map(task => {
    // task 文本来自用户目标，仅作为纯文本选项，不能执行网页标记。
    const option = document.createElement('option');
    option.value = task.id;
    option.textContent = `${label(task.state)} · ${task.objective.slice(0, 35)}${task.reason ? ` · ${label(task.reason)}` : ''}`;
    option.selected = task.id === current?.id;
    return option;
  }) : [new Option(label('empty'), '')]));
}

/** 应用后端任务投影；fill 只在首次打开时载入任务参数，不改正在编辑的连接字段。 */
function applyTasks(result, fill = false) {
  tasks = result.tasks;
  connected = result.connected === true;
  connectionState = connected ? 'connected' : 'unconfigured';
  connectionError = '';
  const matching = tasks.filter(task => task.tabId === tabId && task.binding.conversation === conversation);
  // 历史暂停记录不能遮住本标签仍在运行的任务。
  current = matching.find(task => !['paused', 'done'].includes(task.state)) || matching[0] || null;
  prearm = current ? null : result.prearm || null;
  const source = current || prearm?.options;
  if (fill && source) {
    for (const key of ['objective', 'limit', 'delay']) element(key).value = source[key];
    element('mode').value = source.mode;
    const policy = source.policy || {};
    element('recovery').checked = policy.recovery !== false;
    element('continuous').checked = policy.continuous !== false;
    element('adaptiveTiming').checked = policy.adaptive_timing !== false;
    element('silenceSeconds').value = policy.silence_seconds || 600;
    element('lifetimeHours').value = policy.lifetime_hours || 24;
    element('localRunner').value = policy.runner_id || '';
    element('keepAwake').checked = policy.keep_awake === true;
    element('sessionId').value = policy.session_id || '';
    element('trigger').value = policy.trigger || 'after_turn';
    element('sessionId').required = false;
  }
}

/** 向唯一 worker 请求操作；错误码在本地翻译，禁止直接渲染任意异常字符串。 */
async function request(action, target = tabId, extra = {}) {
  const result = await chrome.runtime.sendMessage({ action, tabId: target, taskId: current?.id, ...extra });
  if (!result?.ok) throw new Error(result?.error || 'generic');
  return result;
}

/** 重新读取账本；fill 为真时加载当前任务选项，不覆盖正在编辑的目标。 */
async function refresh(fill = false) {
  if (refreshing) return;
  refreshing = true;
  const revision = connectionRevision;
  try {
    const result = await request('list', tabId ?? -1);
    if (revision !== connectionRevision) return;
    applyTasks(result, fill);
    render();
  } catch (error) {
    if (revision === connectionRevision) throw error;
    // 已开始新连接时，旧地址查询失败也不能覆盖新连接状态。
  } finally { refreshing = false; }
}

/** 在一次控制请求期间禁用相关动作；失败保留可见错误并恢复控件。 */
async function perform(action, extra = {}, target = tabId) {
  working = true;
  actionError = '';
  render();
  try {
    await request(action, target, extra);
    await refresh();
  } catch (error) {
    actionError = error.message;
  } finally {
    working = false;
    render();
  }
}

/** 初始化 popup 的语言和当前站点；仅本次选中标签可以获得启用权。 */
async function initialize() {
  const saved = await chrome.storage.local.get('continuation.locale');
  locale = saved['continuation.locale'] === 'en' ? 'en' : 'zh';
  const [tab] = await chrome.tabs.query({ active: true, currentWindow: true });
  const location = chatLocation(tab?.url) || newChatLocation(tab?.url);
  if (tab?.id !== undefined && location) { tabId = tab.id; conversation = location.conversation; }
  const metadata = await request('connection', -1);
  savedUrl = metadata.url;
  tokenSaved = metadata.tokenSaved === true;
  const draft = (await chrome.storage.session.get(CONNECTION_DRAFT))[CONNECTION_DRAFT];
  const validDraft = draft && typeof draft.url === 'string' && typeof draft.token === 'string'
    && draft.url.length <= element('serverUrl').maxLength && draft.token.length <= element('serverToken').maxLength;
  element('serverUrl').value = validDraft ? draft.url : savedUrl;
  element('serverToken').value = validDraft ? draft.token : '';
  element('connectionPanel').open = !tokenSaved || validDraft;
  if (tokenSaved) await refresh(true);
  else { connectionState = 'unconfigured'; render(); }
  const fields = savedUrl && tabId !== null ? (await request('form_read', tabId, { origin: savedUrl, conversation })).draft : null;
  if (fields && typeof fields === 'object') {
    for (const id of FORM_FIELDS) {
      const input = element(id), value = fields[id];
      if (input.type === 'checkbox' && typeof value === 'boolean') input.checked = value;
      else if (typeof value === 'string' && value.length <= Math.max(input.maxLength || 0, 2000)) input.value = value;
    }
  }
}

/** 显式连接并保存；权限请求保留用户手势，只有认证和持久化成功后清空输入。 */
async function connect(event) {
  event.preventDefault();
  if (working) return;
  connectionRevision++;
  try {
    const origin = serverOrigin(element('serverUrl').value);
    // 权限弹窗可能转移焦点；先捕获字段，后台请求不再依赖输入控件的瞬时值。
    const token = element('serverToken').value;
    working = true;
    connectionState = 'checking';
    actionError = '';
    render();
    const granted = await chrome.permissions.request({ origins: [`${origin}/*`] });
    if (!granted) throw new Error('permission_denied');
    const result = await request('configure', -1, { connection: { url: origin, token } });
    await chrome.storage.session.remove(CONNECTION_DRAFT);
    applyTasks(result);
    savedUrl = result.url;
    tokenSaved = result.tokenSaved === true;
    element('serverUrl').value = savedUrl;
    element('serverToken').value = '';
  } catch (error) {
    connected = false;
    connectionState = 'failed';
    connectionError = error.message;
    element('connectionPanel').open = true;
  } finally {
    working = false;
    render();
  }
}

// event 是用户显式提交；字段在 HTML 和 worker 两处校验，不能由网页触发控制请求。
element('settings').addEventListener('submit', event => {
  event.preventDefault();
  perform(current && !['paused', 'done'].includes(current.state) ? 'update' : 'start', { options: optionsFromForm() });
});
// 每次输入保存当前聊天草稿，关闭面板或切换聊天后仍可恢复。
element('settings').addEventListener('input', saveFormDraft);
// 只有选择明确的本机 Runner 才能授权电源资源；清空 ID 不授予任何本机能力。
element('localRunner').addEventListener('input', () => { element('keepAwake').disabled = !element('localRunner').value.trim(); });
// 队列草稿按当前聊天保存，尚未提交的内容不进入自动投递。
element('queuePanel').addEventListener('input', saveFormDraft);
// 人工输入和阶段工作流使用固定类型；规划请求由当前 ChatGPT 执行。
element('queueAdd').addEventListener('click', () => {
  const kind = element('queueKind').value, text = element('queueText').value;
  perform('queue', { kind, text: kind === 'message' ? text : '', stages: kind === 'workflow' ? text.split('\n').map(line => line.trim()).filter(Boolean) : [], trigger: element('trigger').value });
});
// 请求当前聊天规划，不打开新聊天、不读取或发送任何模型密钥。
element('queuePlan').addEventListener('click', () => { perform('queue', { kind: 'plan', trigger: element('trigger').value }); });
// 按服务端条目 ID 精确移除，不把界面索引当成投递身份。
element('queueRemove').addEventListener('click', () => { perform('dequeue', { entryId: element('queueEntries').value }); });
// Session 可由原生工具关联；切换 finish 不要求用户手填内部身份。
element('trigger').addEventListener('change', () => { element('sessionId').required = false; saveFormDraft(); });
// 访问 Server 的权限只由用户点击连接时请求；密钥不传给任何 ChatGPT 页面。
element('connectionForm').addEventListener('submit', connect);
// 输入即保存会话草稿；地址变化同时更新空白 PAT 的复用边界。
element('serverUrl').addEventListener('input', () => { saveConnectionDraft(); renderConnection(); });
// 只保存用户刚输入的 PAT，不读取后端连接存储中的已保存密钥。
element('serverToken').addEventListener('input', saveConnectionDraft);
// 每个 button 对应固定操作，不将网页文本解释为操作名。
for (const action of ['pause', 'continue']) element(action).addEventListener('click', () => { perform(action, action === 'continue' ? { options: optionsFromForm() } : {}); });
// 移除只删除选中任务的本账户后端记录，不删除 ChatGPT 会话或项目文件。
element('remove').addEventListener('click', () => { perform('delete', { taskId: element('tasks').value }, -1); });
// 语言偏好只写独立键；不参与任务写队列或发送状态。
element('language').addEventListener('click', () => {
  locale = locale === 'zh' ? 'en' : 'zh';
  chrome.storage.local.set({ 'continuation.locale': locale }).then(render, () => {
    actionError = 'generic'; render();
  });
});
/** popup 打开期间每三秒读取后端投影；单次读取未结束时跳过，不积压请求。 */
const STATUS_INTERVAL_MS = 3000;
setInterval(() => {
  if (connected && !working) refresh().catch(error => {
    connected = false; connectionState = 'failed'; connectionError = error.message; render();
  });
}, STATUS_INTERVAL_MS);
render();
initialize().catch(error => {
  element('connectionPanel').open = true;
  connected = false; connectionState = 'failed'; connectionError = error.message; render();
}).finally(() => { working = false; render(); });

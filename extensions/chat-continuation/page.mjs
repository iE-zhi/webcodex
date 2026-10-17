/** 在 ChatGPT MAIN world 中读取公开回合模型或执行一次已预留发送；不访问 Cookie、网络或工具凭据。 */

/**
 * Chrome scripting 注入入口；必须自包含，不能捕获扩展环境。
 * action 为 snapshot 或 send；input 含预期 URL、用户/回复 ID 与正文，以及已持久化的发送正文。
 * 页面结构未知、身份冲突、草稿或生成中均拒绝发送；发送异常由 worker 保留未知状态。
 */
export async function pageAction(action = 'snapshot', input = null) {
  /** 每次最多检查 32 个已渲染节点，每个节点向上读取 80 层 React 分支。 */
  const MAX_NODES = 32, MAX_CLIMB = 80;
  /** 每次只扫描末尾 400 条消息；完整公开消息最多 128 KiB UTF-8，超限返回明确状态。 */
  const MAX_MESSAGES = 400, MAX_TEXT = 131072;
  /** React 提交路径最多 2,048 层、4,096 个兄弟节点；超限不读取旧渲染分支。 */
  const MAX_ROOT_DEPTH = 2048, MAX_SIBLINGS = 4096;
  /** 提供方消息 ID 上限 200 字符；只在至多 16 个公开内容引用中查找确切回退正文。 */
  const MAX_ID = 200, MAX_REFERENCES = 16;
  /** 页面内活动观察器唯一键；不保存凭据或任何推理/工具正文，离开主文档即销毁。 */
  const ACTIVITY_KEY = '__webcodexContinuationActivityV2';
  let activity = window[ACTIVITY_KEY];
  if (!activity) {
    activity = { revision: 0, user: '', stopped: '', target: null, observer: null, cancelled: new Set() };
    window[ACTIVITY_KEY] = activity;
    // 捕获阶段记录真实停止意图；页面脚本合成点击不能清除或授予续跑权限。
    document.addEventListener?.('click', event => {
      const button = event.target?.closest?.('button');
      if (event.isTrusted && button?.matches('[data-testid="stop-button"],button[aria-label="Stop streaming"],button[aria-label="停止生成"]')) activity.stopped = activity.user;
    }, true);
  }
  if (action === 'arm') activity.stopped = '';
  if (action === 'cancel') {
    if (typeof input?.id === 'string') {
      /** 只保留最近 64 个取消标识；用于拦截仍在当前文档等待发送按钮的旧投递。 */
      const MAX_CANCELLED = 64;
      activity.cancelled.add(input.id);
      if (activity.cancelled.size > MAX_CANCELLED) activity.cancelled.delete(activity.cancelled.values().next().value);
    }
    return { cancelled: true };
  }

  /** 识别输入框所属区域的附件及上传状态；没有文字的附件同样属于人工草稿。 */
  function attachments() {
    const editor = document.querySelector('#prompt-textarea');
    const host = editor?.closest('form') || editor?.parentElement;
    if (!host) return false;
    if (host.querySelector?.('[data-composer-attachments]')?.childElementCount > 0) return true;
    if (host.querySelector?.('[data-inline-file-uploading],[role="progressbar"],[data-testid="file-upload"],[data-testid="attachment"]')) return true;
    return [...(host.querySelectorAll?.('button[aria-label]') || [])].some(button =>
      /^(?:remove|delete)\s+(?:file|attachment|image)|(?:移除|删除).*(?:文件|附件|图片)/i.test(button.getAttribute('aria-label') || ''));
  }

  /** 从 node 的原始 Fiber 指针还原当前已提交路径；不修改 React 指针，不保留跨快照引用。 */
  function committedFiber(node) {
    const key = Object.keys(node).find(name => name.startsWith('__reactFiber$'));
    const fiber = key ? node[key] : null;
    const path = [], seen = new Set();
    let at = fiber, root = null, current = null, paired = false;
    for (let depth = 0; at && depth < MAX_ROOT_DEPTH; depth++, at = at.return) {
      if (seen.has(at)) return null;
      seen.add(at); paired ||= Boolean(at.alternate);
      if (at.tag === 3) {
        root = at.stateNode; current = root?.current;
        if (!current || current !== at && current !== at.alternate) return null;
        break;
      }
      path.push(at);
    }
    // 允许旧版无双缓冲的 owner facade；真实 React 树必须证明 root.current。
    if (!root) return !at && !paired && path.every(item => typeof item.tag !== 'number') ? fiber : null;
    let selected = current, view = { memoizedProps: current.memoizedProps, return: null }, remaining = MAX_SIBLINGS;
    for (let index = path.length - 1; index >= 0; index--) {
      const wanted = path[index];
      let childMatch = null;
      for (let child = selected.child; child; child = child.sibling) {
        if (--remaining < 0) return null;
        if (child !== wanted && child !== wanted.alternate) continue;
        if (childMatch) return null;
        childMatch = child;
      }
      if (!childMatch || root.current !== current) return null;
      selected = childMatch;
      view = { memoizedProps: selected.memoizedProps, return: view };
    }
    return view;
  }

  /** 从 message 读取完整公开正文，不读取私有推理或工具结果；超限返回空串使页面停止续跑。 */
  function textOf(message) {
    const parts = message.content?.parts;
    if (!Array.isArray(parts) || parts.length > MAX_MESSAGES) return '';
    let text = '';
    for (const part of parts) {
      if (typeof part !== 'string') continue;
      if (text.length + part.length > MAX_TEXT) return '';
      text += (text ? '\n' : '') + part;
    }
    return new TextEncoder().encode(text).byteLength <= MAX_TEXT ? text.trim() : '';
  }

  /** 将 shell 的公开消息投影为同一格式；保留其明确的最终完成状态。 */
  function shellMessages(turn) {
    if (!Array.isArray(turn.items)) return null;
    return turn.items.slice(-MAX_MESSAGES).flatMap(item => {
      // item 只允许用户和公开 assistant 项，其他项含推理或工具数据，不跨注入边界。
      if (item?.type === 'user-message') return [{ id: item.messageId || item.serverMessageId,
        author: { role: 'user' }, content: { parts: [item.message] } }];
      if (item?.type !== 'assistant-message') return [];
      let text = typeof item.content === 'string' ? item.content : '';
      if (/^\s*(?:::chatgpt-content-reference\{[^}\n]*\}\s*)+$/.test(text)) {
        if (!Array.isArray(item.contentReferences) || item.contentReferences.length > MAX_REFERENCES) return [];
        const refs = (Array.isArray(item.contentReferences) ? item.contentReferences : []).filter(ref =>
          ref?.type === 'dil' && ref.source_message_id === item.messageId && typeof ref.model_dil_v2?.fallbackMarkdown === 'string');
        // 只使用确切 messageId 的公开回退正文；不能用其他消息的内容替代。
        text = refs.length === 1 ? refs[0].model_dil_v2.fallbackMarkdown : '';
      }
      return [{ id: item.messageId || item.latestMessageId, author: { role: 'assistant' },
        channel: item.phase === 'final_answer' ? 'final' : 'commentary', recipient: 'all',
        end_turn: item.completed === true && item.phase === 'final_answer' && turn.status === 'complete',
        status: item.completed === true ? 'finished_successfully' : 'in_progress', content: { parts: [text] } }];
    });
  }

  /** 读取当前页面的公开状态；仅明确的身份与 terminal 模型可驱动后续动作。 */
  function snapshot() {
    const route = location.pathname.match(/^(?:\/g\/[^/]+)?\/c\/([a-f\d-]{36})\/?$/i);
    const conversation = route?.[1] || null;
    const editor = document.querySelector('#prompt-textarea');
    const busy = Boolean(document.querySelector('[data-testid="stop-button"],button[aria-label="Stop streaming"],button[aria-label="停止生成"]')) ||
      [...(editor?.closest('form')?.querySelectorAll('button svg path') || [])].some(path => /^\s*M4\.5 5\.75/.test(path.getAttribute('d') || ''));
    const base = { url: location.href, supported: false, busy, draft: Boolean(editor?.textContent?.trim()) || attachments(),
      blocked: false, interrupted: false, toolsBusy: false, approval: false, userId: null, userText: '', reply: null,
      unsupportedReason: 'unsupported', activity: { source_turn: '', revision: activity.revision, stop_requested: false,
        model: '', deliberate: false, thinking_failed: false, rate_limited: false, retry_after: 0 } };
    if (!editor) return base;
    const rows = document.querySelectorAll('[data-testid^="conversation-turn-"],[data-turn-key]');
    if (!conversation && rows.length === 0) return { ...base, supported: location.pathname === '/' || /^\/g\/[^/]+\/?$/.test(location.pathname) };
    const row = rows[rows.length - 1];
    // 当前回合和全局提示会阻止发送；历史回合的已保留错误不阻塞后续任务。
    const alerts = document.querySelectorAll('[data-testid="conversation-turn-error"],main [role="alert"]');
    for (let index = Math.max(0, alerts.length - MAX_NODES); index < alerts.length; index++) {
      const owner = alerts[index].closest('[data-testid^="conversation-turn-"],[data-turn-key]');
      if (!owner || owner === row) {
        // 只把明确中断提示作为可恢复错误；其余提示均等待用户，避免绕过审批。
        const text = alerts[index].textContent || '';
        if (/rate limit|usage limit|too many requests|reached.*limit|稍后重试|使用上限|次数上限|请求过多/i.test(text)) {
          base.activity.rate_limited = true;
          /** 提供方等待提示最多接受一天，单位秒；不解析任意日期或执行页面代码。 */
          const MAX_RETRY_SECONDS = 86400;
          const wait = text.match(/(?:in|等待|后|再过)\s*(\d+)\s*(seconds?|minutes?|hours?|秒|分钟|小时)/i);
          if (wait) base.activity.retry_after = Math.min(MAX_RETRY_SECONDS, Number(wait[1]) * (/minute|分钟/i.test(wait[2]) ? 60 : /hour|小时/i.test(wait[2]) ? 3600 : 1));
        } else if (/thinking failed|reasoning failed|思考失败|推理失败/i.test(text)) {
          base.interrupted = true; base.activity.thinking_failed = true;
        } else if (/interrupted|network error|something went wrong|中断|网络错误/i.test(text)) base.interrupted = true;
        else base.blocked = true;
      }
    }
    // 原生审批卡使用与界面语言无关的标记；只观察可见且可操作的卡片，不点击按钮。
    const approvals = document.querySelectorAll('[data-codex-approval-surface="true"],[data-testid="tool-approval"],[data-testid="approval-card"],[data-testid="confirm-tool-call"]');
    base.approval = Array.from(approvals).slice(-MAX_NODES).some(card => card.getClientRects().length > 0 &&
      !card.closest('[hidden],[aria-hidden="true"],[inert]') && Boolean(card.querySelector('button:not([disabled])')));
    base.toolsBusy = Boolean(document.querySelector('[data-testid="tool-call-running"],[data-testid="tool-progress"][aria-busy="true"]'));
    if (!row || !conversation) return base;
    if (activity.target !== row && typeof MutationObserver === 'function') {
      activity.observer?.disconnect();
      activity.target = row;
      // 仅计数当前回合的结构/文字变化，不读取或导出推理、工具正文；忽略动画属性变化。
      activity.observer = new MutationObserver(() => { activity.revision++; });
      activity.observer.observe(row, { childList: true, characterData: true, subtree: true });
    }
    const descendants = row.querySelectorAll('[data-message-author-role],[data-markdown-text-style],[data-content-search-unit-key]');
    const nodes = [row];
    for (let index = Math.max(0, descendants.length - MAX_NODES + 1); index < descendants.length; index++) nodes.push(descendants[index]);
    let selected = null;
    for (const node of nodes) {
      let fiber = committedFiber(node);
      let messages = null;
      let sourceTurn = '', model = '', deliberate = false;
      const owners = new Set();
      for (let depth = 0; fiber && depth < MAX_CLIMB; depth++, fiber = fiber.return) {
        const props = fiber.memoizedProps;
        if (!props || typeof props !== 'object') continue;
        const turn = props.entry?.turn || props.turn;
        if (typeof turn?.id === 'string') sourceTurn = turn.id.slice(0, MAX_ID);
        const modelName = turn?.modelSlug || props.modelSlug || props.model?.slug;
        if (typeof modelName === 'string' && modelName.length <= 100) model = modelName;
        deliberate ||= ['high', 'xhigh', 'extended'].includes(props.reasoningEffort || turn?.reasoningEffort);
        if (!messages && turn) messages = Array.isArray(turn.items) ? shellMessages(turn) : turn.messages;
        if (!messages && Array.isArray(props.allMessages)) messages = props.allMessages;
        for (const owner of [props.conversationId, props.clientThreadId, props.entry?.conversationId,
          props.turn?.conversationId, props.turn?.clientThreadId, props.conversation?.id]) {
          if (typeof owner === 'string' && /^[a-f\d-]{36}$/i.test(owner)) owners.add(owner);
        }
      }
      if (owners.size !== 1 || !owners.has(conversation) || !Array.isArray(messages)) continue;
      messages = messages.slice(-MAX_MESSAGES);
      const user = messages.findLast(message => message?.author?.role === 'user');
      const answer = messages.findLast(message => message?.author?.role === 'assistant' &&
        !['analysis', 'commentary'].includes(message.channel) && (!message.recipient || message.recipient === 'all'));
      if (typeof user?.id !== 'string' || !user.id || user.id.length > MAX_ID) continue;
      const userIndex = messages.indexOf(user), answerIndex = answer ? messages.indexOf(answer) : -1;
      const reply = answerIndex > userIndex && typeof answer.id === 'string' && answer.id.length > 0 && answer.id.length <= MAX_ID ? {
        id: answer.id, userId: user.id, text: textOf(answer),
        terminal: answer.end_turn === true && answer.status === 'finished_successfully',
      } : null;
      if (reply?.terminal && !reply.text) return { ...base, unsupportedReason: 'page_limit' };
      const userText = textOf(user);
      if (!userText) return { ...base, unsupportedReason: 'page_limit' };
      activity.user = user.id;
      // 来源证明只接受当前用户回合之后、面向用户的单行公开进度；不读取推理或工具结果。
      let sourceProof = null;
      for (const message of messages.slice(userIndex + 1).slice(-MAX_NODES)) {
        if (message?.author?.role !== 'assistant' || message.channel !== 'commentary' || message.recipient && message.recipient !== 'all'
            || typeof message.id !== 'string' || !message.id || message.id.length > MAX_ID || message.content?.parts?.length !== 1) continue;
        const text = message.content.parts[0];
        if (typeof text !== 'string' || text.length > 256) continue;
        const match = text.trim().match(/^\[\[WEBCODEX_SOURCE:([a-f\d-]{36}):(\d{1,16}):([a-f\d]{64})\]\]$/i);
        if (match && Number.isSafeInteger(Number(match[2]))) sourceProof = { task_id: match[1], generation: Number(match[2]), challenge: match[3], user_id: user.id, message_id: message.id };
      }
      const candidate = { ...base, supported: true, userId: user.id, userText, reply, userCount: messages.filter(message => message?.author?.role === 'user').length,
        sourceProof,
        activity: { ...base.activity, source_turn: sourceTurn || user.id, model, deliberate,
          stop_requested: activity.stopped === user.id } };
      if (selected && (selected.userId !== candidate.userId || selected.reply?.id !== candidate.reply?.id)) return base;
      selected = candidate;
    }
    return selected || base;
  }

  const page = snapshot();
  if (action === 'snapshot' || action === 'arm') return page;
  /** 发送正文最多 65,536 字符，覆盖完整工作流及目标控制提示；拒绝超限而非截断。 */
  const MAX_PROMPT = 65536;
  if (action !== 'send' || !input || typeof input.text !== 'string' || input.text.length > MAX_PROMPT ||
      !page.supported || page.url !== input.url || page.userId !== input.userId ||
      page.userText !== input.userText || page.reply?.id !== input.replyId || page.reply?.text !== input.replyText ||
      (!page.reply?.terminal && input.recovery !== true) || page.busy || page.draft || page.blocked || page.approval || page.toolsBusy || page.activity.stop_requested || page.activity.rate_limited || activity.cancelled.has(input.id)) return { sent: false, definite: true, cleared: !page.draft, reason: 'preflight' };
  const editor = document.querySelector('#prompt-textarea');
  if (!editor?.isContentEditable) return { sent: false, definite: true, cleared: true, reason: 'editor' };
  /** 只清理本次插入且用户未改动的正文；存在附件或用户编辑时保持原样并拒绝重试。 */
  function unsent(reason) {
    if (editor.isConnected && !attachments() && editor.innerText.replace(/\r/g, '') === input.text) {
      editor.focus();
      const selected = window.getSelection(), owned = document.createRange();
      owned.selectNodeContents(editor); selected.removeAllRanges(); selected.addRange(owned);
      document.execCommand('insertText', false, '');
    }
    return { sent: false, definite: true, cleared: !editor.textContent?.trim() && !attachments(), reason };
  }
  // 正文只以纯文本进入编辑器，不使用 innerHTML、page eval 或 Enter 键后备发送。
  editor.focus();
  const selection = window.getSelection(), range = document.createRange();
  range.selectNodeContents(editor);
  selection.removeAllRanges();
  selection.addRange(range);
  if (!document.execCommand('insertText', false, input.text)) return unsent('insert');
  /** 最多等待一秒让受控输入框更新；每次等待后都重新核对 URL、用户和草稿。 */
  const INPUT_ATTEMPTS = 10, INPUT_DELAY_MS = 100;
  for (let attempt = 0; attempt < INPUT_ATTEMPTS; attempt++) {
    const current = snapshot();
    if (current.url !== input.url || current.userId !== input.userId || current.userText !== input.userText ||
        current.reply?.id !== input.replyId || current.reply?.text !== input.replyText || (!current.reply?.terminal && input.recovery !== true) || current.busy || current.blocked || current.approval || current.toolsBusy ||
        current.activity.stop_requested || current.activity.rate_limited || attachments() || activity.cancelled.has(input.id) ||
        editor.innerText.replace(/\r/g, '') !== input.text || !editor.isConnected) return unsent('changed');
    const send = document.querySelector('[data-testid="send-button"],#composer-submit-button');
    if (send && !send.disabled && send.getAttribute('aria-disabled') !== 'true' && !send.closest('[hidden]')) {
      send.click();
      // 点击仅表示已尝试；worker 仍须从公开模型核对 WC_SEND 回执。
      return { sent: true };
    }
    // resolve 只负责本次有界等待，不保留页面或扩展资源。
    await new Promise(resolve => setTimeout(resolve, INPUT_DELAY_MS));
  }
  return unsent('send_button');
}

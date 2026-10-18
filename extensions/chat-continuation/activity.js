/** 隔离页面观察器：只唤醒扩展读取公开状态，并转发真实 Stop；不持有 PAT、任务或发送权。 */
(() => {
  /** 同一隔离文档只允许一个观察器；扩展更新先释放旧监听，重复注入不增加心跳。 */
  const KEY = '__webcodexContinuationObserver', VERSION = '7';
  if (globalThis[KEY]?.version === VERSION) return;
  globalThis[KEY]?.dispose?.();
  /** 每文档最多每秒一个活动通知，十五秒一个心跳；后台标签仍受 Chrome 调度限制。 */
  const EVENT_GAP_MS = 1000, HEARTBEAT_MS = 15000;
  /** 每次最多核对 32 个输入框候选；首发正文最多 128 KiB UTF-8，超限不记录启用意图。 */
  const MAX_EDITORS = 32, MAX_INTENT_BYTES = 131072;
  /** 当前文档只有一个在途通知；普通变化合并，真实 Stop 独立立即提交。 */
  let pending = false, timer = null, last = 0, closed = false;

  /** error 为同步调用或异步响应错误；上下文失效则释放旧文档资源，其他失败保留心跳重试并告警。 */
  function failed(error) {
    if (closed) return;
    if (/Extension context invalidated/i.test(error?.message ?? String(error))) dispose();
    else console.warn('Continuation page bridge unavailable; next heartbeat will retry.');
  }

  /** message 只含观察事件；同时接住 sendMessage 同步异常和 Promise 拒绝，不取得发送聊天消息的权限。 */
  function send(message) {
    if (closed) return Promise.resolve();
    try {
      // 扩展重新加载会使旧文档的 runtime.id 消失；旧观察器不能再次连接或反复报告失败。
      if (!globalThis.chrome?.runtime?.id) { dispose(); return Promise.resolve(); }
      return Promise.resolve(chrome.runtime.sendMessage(message)).catch(failed);
    } catch (error) {
      failed(error);
      return Promise.resolve();
    }
  }

  /** 只读取唯一可见聊天输入框，兼容旧 ID 与新版聊天表单；多候选或隐藏区域返回 null。 */
  function composer() {
    const candidates = document.querySelectorAll('#prompt-textarea,form[data-chatgpt-composer] [role="textbox"][contenteditable="true"].ProseMirror');
    if (candidates.length > MAX_EDITORS) return null;
    const visible = Array.from(candidates).filter(editor => editor.getClientRects().length > 0 &&
      !editor.closest('[hidden],[aria-hidden="true"],[inert]'));
    return visible.length === 1 ? visible[0] : null;
  }

  /** 只在空白聊天或明确项目首页的真实发送操作中记录首发意图；正文只用于 worker 比较摘要。 */
  function firstSend(event) {
    if (closed || !event.isTrusted) return;
    if (!/^(?:\/g\/[A-Za-z0-9_-]+)?\/?$/.test(location.pathname) && !/^\/g\/g-p-[a-f\d]{32}(?:-[A-Za-z0-9_-]+)?\/project\/?$/i.test(location.pathname)) return;
    const editor = composer();
    const text = editor?.innerText ?? editor?.value;
    if (typeof text !== 'string' || new TextEncoder().encode(text).length > MAX_INTENT_BYTES) return;
    send({ event: 'webcodex_intent', text });
  }

  /** 只发送唤醒或真实停止，失败不执行本地续跑；下一次心跳重新尝试。 */
  function notify(stop = false) {
    if (closed || !stop && pending) return;
    if (!stop) pending = true;
    last = Date.now();
    send({ event: 'webcodex_activity', stop }).finally(() => { if (!stop) pending = false; });
  }

  /** 合并当前文档的结构和文字变化，不读取、保存或导出变化正文。 */
  function changed() {
    if (closed || timer !== null) return;
    timer = setTimeout(() => { timer = null; notify(); }, Math.max(0, EVENT_GAP_MS - (Date.now() - last)));
  }

  /** 捕获原生用户点击；event 的合成输入不能暂停或启动自动任务。 */
  function clicked(event) {
    const button = event.target?.closest?.('button');
    if (event.isTrusted && button?.matches('[data-testid="stop-button"],button[aria-label="Stop streaming"],button[aria-label="停止生成"]')) notify(true);
    else if (button?.matches('[data-testid="send-button"],#composer-submit-button,form[data-chatgpt-composer] button[type="submit"]') && !button.disabled && button.getAttribute('aria-disabled') !== 'true') firstSend(event);
    else if (event.isTrusted && event.target?.closest?.('a[href]')) {
      // 人工导航不属于首次发送，撤销预置；不会触碰已存在的 Server 任务。
      send({ event: 'webcodex_prearm_cancel' });
    }
  }
  /** Enter 只观察真实、不组合输入的发送操作；合成事件不授予首次发送意图。 */
  function keydown(event) {
    if (event.key === 'Enter' && !event.shiftKey && !event.ctrlKey && !event.altKey && !event.metaKey && !event.isComposing && composer()?.contains(event.target)) firstSend(event);
  }
  /** 用户返回历史页面会撤销预置，网站合成 popstate 不取得控制权。 */
  function navigated(event) { if (event.isTrusted) send({ event: 'webcodex_prearm_cancel' }); }
  document.addEventListener('click', clicked, true);
  document.addEventListener('keydown', keydown, true);
  window.addEventListener('popstate', navigated);
  const observer = new MutationObserver(changed);
  observer.observe(document, { childList: true, characterData: true, subtree: true });
  const heartbeat = setInterval(() => notify(), HEARTBEAT_MS);
  // BFCache 恢复仍保留观察器；真正离开时由文档销毁释放监听、计时器和 Promise。
  window.addEventListener('pageshow', changed);
  /** BFCache 保留当前文档；真正离开或更新则取消所有本观察器资源。 */
  function hidden(event) { if (!event.persisted) dispose(); }
  /** 幂等撤销监听和计时器，不改任务、草稿或用户页面内容。 */
  function dispose() {
    if (closed) return;
    closed = true; observer.disconnect(); clearInterval(heartbeat); clearTimeout(timer);
    document.removeEventListener('click', clicked, true); document.removeEventListener('keydown', keydown, true);
    window.removeEventListener('popstate', navigated); window.removeEventListener('pageshow', changed); window.removeEventListener('pagehide', hidden);
    if (globalThis[KEY]?.dispose === dispose) delete globalThis[KEY];
  }
  window.addEventListener('pagehide', hidden);
  globalThis[KEY] = { version: VERSION, dispose };
  notify();
})();

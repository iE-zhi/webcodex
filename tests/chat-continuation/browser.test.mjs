/** 在独立 Chromium profile 中验证生产页面适配器和 popup；不加载个人资料，不连接真实 ChatGPT。 */
import test from 'node:test';
import assert from 'node:assert/strict';
import { spawn } from 'node:child_process';
import { mkdtemp, readFile, writeFile, rm, access } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import path from 'node:path';
import { createServer } from 'node:http';
import { once } from 'node:events';
import { pageAction } from '../../extensions/chat-continuation/page.mjs';

/** 测试进程、协议请求和等待的统一上限，单位毫秒。 */
const TIMEOUT_MS = 15000;
/** CDP 单次未解析缓冲最多 8 Mi 字符，覆盖截图而限制异常进程输出。 */
const MAX_CDP_BUFFER_CHARS = 8 * 1024 * 1024;
/** 合成页面使用的固定会话，不包含真实聊天数据。 */
const CONVERSATION = '11111111-1111-4111-8111-111111111111';
/** 生产扩展目录；HTTP 测试服务器只允许显式列出的静态文件。 */
const ROOT = path.resolve('extensions/chat-continuation');
/** 可通过 BROWSER_BINARY 选择 CI 的浏览器；默认只检查标准安装位置。 */
const CANDIDATES = [process.env.BROWSER_BINARY, 'C:/Program Files/Google/Chrome/Application/chrome.exe',
  'C:/Program Files (x86)/Microsoft/Edge/Application/msedge.exe', '/usr/bin/google-chrome', '/usr/bin/chromium'].filter(Boolean);

/** 找到首个可访问的浏览器；缺失时显式跳过，不把未执行写成通过。 */
async function browserBinary() {
  for (const candidate of CANDIDATES) {
    try { await access(candidate); return candidate; } catch { /* 当前候选未安装，继续有限候选列表。 */ }
  }
  return null;
}

/** 使用子进程私有 CDP pipe；每个请求拥有超时，关闭时拒绝未完成请求，不暴露调试端口。 */
async function protocol(browser) {
  const pending = new Map(), input = browser.stdio[3], output = browser.stdio[4];
  let sequence = 0, buffer = '';
  output.setEncoding('utf8');
  /** 收齐以 NUL 分隔的协议包；测试流量不包含个人浏览器数据。 */
  output.on('data', chunk => {
    buffer += chunk;
    if (buffer.length > MAX_CDP_BUFFER_CHARS) {
      for (const call of pending.values()) { clearTimeout(call.timer); call.reject(new Error('CDP response exceeds limit')); }
      pending.clear(); buffer = ''; input.destroy(); output.destroy(); return;
    }
    let boundary;
    while ((boundary = buffer.indexOf('\0')) !== -1) {
      const response = JSON.parse(buffer.slice(0, boundary));
      buffer = buffer.slice(boundary + 1);
      const call = pending.get(response.id);
      if (!call) continue;
      pending.delete(response.id); clearTimeout(call.timer);
      if (response.error) call.reject(new Error(`${call.method}: ${response.error.message}`)); else call.resolve(response.result);
    }
  });
  /** pipe 关闭或写入失败统一终止挂起请求，避免留下超时句柄。 */
  const closed = () => {
    for (const call of pending.values()) { clearTimeout(call.timer); call.reject(new Error('CDP closed')); }
    pending.clear();
  };
  output.on('close', closed);
  input.on('error', closed);
  return {
    /** method 和 params 为受控测试命令，sessionId 仅指本测试创建的页面。 */
    call(method, params = {}, sessionId) {
      return new Promise((resolve, reject) => {
        const id = ++sequence;
        const timer = setTimeout(() => { pending.delete(id); reject(new Error(`CDP timeout: ${method}`)); }, TIMEOUT_MS);
        pending.set(id, { resolve, reject, timer, method });
        input.write(JSON.stringify({ id, method, params, ...(sessionId ? { sessionId } : {}) }) + '\0');
      });
    },
    /** 主动关闭私有 pipe 并释放挂起请求。 */
    close() { closed(); input.destroy(); output.destroy(); },
  };
}

/** 采用短间隔有界等待文件或页面就绪；read 返回 truthy 即结束，不用于竞态覆盖。 */
async function until(read) {
  const deadline = Date.now() + TIMEOUT_MS;
  while (Date.now() < deadline) {
    const value = await read();
    if (value) return value;
    await new Promise(resolve => setTimeout(resolve, 50));
  }
  throw new Error('Browser fixture timeout');
}

// 合成会话与 popup 验证发送及保存边界；总预算 90 秒覆盖有界原生弹窗初始化和清理。
test('Chromium 验证确切 terminal、无草稿发送及 popup 明暗中英文布局', { timeout: 90000 }, async context => {
  const binary = await browserBinary();
  if (!binary) { context.skip('未安装 Chromium；真实浏览器适配测试未执行'); return; }
  const profile = await mkdtemp(path.join(tmpdir(), 'webcodex-continuation-'));
  const fixture = `<article data-testid="conversation-turn-1"></article><form><div id="prompt-textarea" contenteditable="true"></div><button type="button" data-testid="send-button">Send</button></form>`;
  /** 与已观察新版首页相同的输入框和按钮标识，不依赖旧 ID 或旧 data-testid。 */
  const modernFixture = '<form data-chatgpt-composer><div class="ProseMirror" role="textbox" contenteditable="true"></div><button type="submit">Send</button></form>';
  const server = createServer(async (request, response) => {
    if (request.url === '/api/continuations') {
      // 仅隔离扩展访问本地协议 stub；不触及真实服务或令牌。
      response.setHeader('Access-Control-Allow-Origin', '*');
      response.setHeader('Access-Control-Allow-Headers', 'Authorization, Content-Type');
      if (request.method === 'OPTIONS') { response.end(); return; }
      response.setHeader('Content-Type', 'application/json');
      response.end(JSON.stringify({ tasks: [] })); return;
    }
    const name = request.url.slice(1);
    // 测试表单明确作为 HTML 返回，不依赖浏览器对无 MIME 片段的内容嗅探。
    if (request.url === '/' || request.url.startsWith('/c/')) {
      response.setHeader('Content-Type', 'text/html; charset=utf-8');
      response.end(request.url === '/' ? modernFixture : fixture); return;
    }
    if (!['popup.html', 'popup.css', 'popup.mjs', 'core.mjs'].includes(name)) { response.writeHead(404).end(); return; }
    try {
      response.setHeader('Content-Type', name.endsWith('.mjs') ? 'text/javascript' : name.endsWith('.css') ? 'text/css' : 'text/html');
      response.end(await readFile(path.join(ROOT, name)));
    } catch { response.writeHead(500).end(); }
  });
  server.listen(0, '127.0.0.1'); await once(server, 'listening');
  const origin = `http://127.0.0.1:${server.address().port}`;
  const browser = spawn(binary, ['--headless=new', '--no-first-run', '--disable-background-networking', '--no-default-browser-check',
    '--enable-unsafe-extension-debugging', '--remote-debugging-pipe', `--user-data-dir=${profile}`, 'about:blank'], { stdio: ['ignore', 'ignore', 'pipe', 'pipe', 'pipe'], windowsHide: true });
  /** 浏览器启动诊断上限 4,000 字符，仅包含隔离 profile 的错误，不含用户数据。 */
  let diagnostic = '', startupError = null;
  browser.stderr.on('data', chunk => { diagnostic = (diagnostic + chunk.toString()).slice(-4000); });
  browser.on('error', error => { startupError = error; });
  let cdp;
  try {
    cdp = await protocol(browser);
    try { await cdp.call('Browser.getVersion'); }
    catch (error) { throw new Error(`Browser startup failed: ${startupError?.message || diagnostic || error.message}`); }
    const { targetId } = await cdp.call('Target.createTarget', { url: 'about:blank' });
    const { sessionId } = await cdp.call('Target.attachToTarget', { targetId, flatten: true });
    /** expression 仅在本测试页面执行；异常转为测试失败，不读取个人浏览器。 */
    const evaluate = async expression => {
      const output = await cdp.call('Runtime.evaluate', { expression, awaitPromise: true, returnByValue: true }, sessionId);
      if (output.exceptionDetails) throw new Error(output.exceptionDetails.text + ': ' + output.result.description);
      return output.result.value;
    };
    await cdp.call('Page.enable', {}, sessionId);
    // 新版首页没有历史回合，必须允许预置；只有真实 Enter/提交按钮才记录首发意图。
    await cdp.call('Page.navigate', { url: origin + '/' }, sessionId);
    await until(() => evaluate('document.querySelector("form[data-chatgpt-composer] [role=textbox]") !== null'));
    const blank = await evaluate(`(${pageAction})()`);
    assert.equal(blank.supported, true); assert.equal(blank.userId, null); assert.equal(blank.draft, false);
    const activitySource = await readFile(path.join(ROOT, 'activity.js'), 'utf8');
    await evaluate(`window.observedActivity=[];window.chrome={runtime:{id:'isolated-activity',sendMessage:async message=>{observedActivity.push(message)}}};
      document.querySelector('form').onsubmit=event=>event.preventDefault();` + activitySource);
    await evaluate(`{const editor=document.querySelector('[role=textbox]');editor.textContent='首发任务';editor.focus();
      editor.dispatchEvent(new KeyboardEvent('keydown',{key:'Enter',bubbles:true}));}`);
    assert.equal(await evaluate('observedActivity.filter(message=>message.event==="webcodex_intent").length'), 0);
    await cdp.call('Input.dispatchKeyEvent', { type: 'keyDown', key: 'Enter', code: 'Enter', windowsVirtualKeyCode: 13 }, sessionId);
    await cdp.call('Input.dispatchKeyEvent', { type: 'keyUp', key: 'Enter', code: 'Enter', windowsVirtualKeyCode: 13 }, sessionId);
    assert.equal(await evaluate('observedActivity.filter(message=>message.event==="webcodex_intent")[0].text'), '首发任务');
    await evaluate(`document.querySelector('[role=textbox]').textContent='按钮首发';`);
    const submitPoint = await evaluate(`{const r=document.querySelector('button[type=submit]').getBoundingClientRect();({x:r.x+r.width/2,y:r.y+r.height/2})}`);
    await cdp.call('Input.dispatchMouseEvent', { type: 'mousePressed', ...submitPoint, button: 'left', clickCount: 1 }, sessionId);
    await cdp.call('Input.dispatchMouseEvent', { type: 'mouseReleased', ...submitPoint, button: 'left', clickCount: 1 }, sessionId);
    assert.equal(await evaluate('observedActivity.filter(message=>message.event==="webcodex_intent").at(-1).text'), '按钮首发');
    await cdp.call('Page.navigate', { url: `${origin}/c/${CONVERSATION}` }, sessionId);
    await until(() => evaluate('document.querySelector("#prompt-textarea") !== null'));
    await evaluate(`window.fixtureMessages = [
      {id:'u1',author:{role:'user'},content:{parts:['原始任务']}},
      {id:'a1',author:{role:'assistant'},channel:'final',recipient:'all',status:'finished_successfully',end_turn:true,content:{parts:['完成']}},
      {id:'private',author:{role:'assistant'},channel:'analysis',status:'in_progress',content:{parts:['不得输出的私有内容']}}
    ]; document.querySelector('article').__reactFiber$test = {memoizedProps:{conversationId:'${CONVERSATION}',turn:{messages:window.fixtureMessages}}};
    document.querySelector('button').onclick=event=>{ event.preventDefault();window.clickedText=document.querySelector('form [contenteditable="true"]').innerText; };`);
    const snapshot = await evaluate(`(${pageAction})()`);
    assert.equal(snapshot.reply.id, 'a1');
    assert.equal(snapshot.reply.terminal, true);
    assert.doesNotMatch(JSON.stringify(snapshot), /不得输出/);
    // 来源确认仅接受当前人工回合的单行公开进度；分析、工具结果及其他收件人都不算。
    const sourceLine = `[[WEBCODEX_SOURCE:${CONVERSATION}:7:${'a'.repeat(64)}]]`;
    for (const [role, channel, recipient, expected] of [['assistant', 'analysis', 'all', false], ['tool', 'commentary', 'all', false], ['assistant', 'commentary', 'worker', false], ['assistant', 'commentary', 'all', true]]) {
      await evaluate(`fixtureMessages.push({id:'source-proof',author:{role:${JSON.stringify(role)}},channel:${JSON.stringify(channel)},recipient:${JSON.stringify(recipient)},content:{parts:[${JSON.stringify(sourceLine)}]}})`);
      const source = await evaluate(`(${pageAction})()`);
      assert.equal(Boolean(source.sourceProof), expected);
      if (expected) assert.deepEqual(source.sourceProof, { task_id: CONVERSATION, generation: 7, challenge: 'a'.repeat(64), user_id: 'u1', message_id: 'source-proof' });
      await evaluate('fixtureMessages.pop()');
    }
    const text = '[[WC_SEND:test]]\n继续当前任务';
    const sendInput = { url: snapshot.url, userId: 'u1', userText: snapshot.userText, replyId: 'a1', replyText: snapshot.reply.text, text };
    const send = await evaluate(`(${pageAction})('send',${JSON.stringify(sendInput)})`);
    assert.equal(send.sent, true, JSON.stringify(send));
    assert.equal(await evaluate('window.clickedText'), text);
    const draft = await evaluate(`(${pageAction})('send',${JSON.stringify(sendInput)})`);
    assert.equal(draft.sent, false);
    // 切换为实际新版表单结构：无旧 ID、无旧按钮标识，发送仍需确切回执且只能点击本表单。
    await evaluate(`{const editor=document.querySelector('#prompt-textarea');editor.textContent='';editor.removeAttribute('id');editor.className='ProseMirror';editor.setAttribute('role','textbox');
      document.querySelector('form').setAttribute('data-chatgpt-composer','');const button=document.querySelector('form button');button.removeAttribute('data-testid');button.type='submit';}`);
    assert.equal((await evaluate(`(${pageAction})()`)).supported, true);
    assert.equal((await evaluate(`(${pageAction})('send',${JSON.stringify(sendInput)})`)).sent, true);
    assert.equal(await evaluate('window.clickedText'), text);
    assert.equal((await evaluate(`(${pageAction})('send',${JSON.stringify(sendInput)})`)).sent, false);
    // 不同可见聊天输入框造成歧义时拒绝；隐藏副本不影响唯一可见输入框。
    await evaluate(`document.querySelector('form [contenteditable="true"]').textContent='';document.body.insertAdjacentHTML('beforeend','<form id="other-composer" data-chatgpt-composer><div role="textbox" class="ProseMirror" contenteditable="true">其他草稿</div></form>')`);
    assert.equal((await evaluate(`(${pageAction})()`)).supported, false);
    assert.equal((await evaluate(`(${pageAction})('send',${JSON.stringify(sendInput)})`)).sent, false);
    await evaluate(`document.querySelector('#other-composer').hidden=true`);
    assert.equal((await evaluate(`(${pageAction})()`)).supported, true);
    await evaluate(`document.querySelector('#other-composer').remove()`);
    // 空文字附件和上传同样阻止发送；使用真实 DOM 和生产 pageAction，不替换其判断。
    await evaluate(`document.querySelector('form [contenteditable="true"]').textContent='';{const attachment=document.createElement('button');attachment.type='button';attachment.setAttribute('aria-label','Remove file draft.txt');document.querySelector('form').appendChild(attachment);}`);
    assert.equal((await evaluate(`(${pageAction})()`)).draft, true);
    assert.equal((await evaluate(`(${pageAction})('send',${JSON.stringify(sendInput)})`)).sent, false);
    await evaluate(`document.querySelector('button[aria-label="Remove file draft.txt"]').remove()`);
    // 合成 click 不代表人工停止；CDP 的真实输入必须记录停止，按钮移除后仍保留该回合的停止意图。
    await evaluate(`{const stop=document.createElement('button');stop.type='button';stop.dataset.testid='stop-button';stop.textContent='Stop';document.body.appendChild(stop);stop.click();}`);
    assert.equal((await evaluate(`(${pageAction})()`)).activity.stop_requested, false);
    const stopPoint = await evaluate(`{const r=document.querySelector('[data-testid="stop-button"]').getBoundingClientRect();({x:r.x+r.width/2,y:r.y+r.height/2})}`);
    await cdp.call('Input.dispatchMouseEvent', { type: 'mousePressed', ...stopPoint, button: 'left', clickCount: 1 }, sessionId);
    await cdp.call('Input.dispatchMouseEvent', { type: 'mouseReleased', ...stopPoint, button: 'left', clickCount: 1 }, sessionId);
    await evaluate(`document.querySelector('[data-testid="stop-button"]').remove()`);
    assert.equal((await evaluate(`(${pageAction})()`)).activity.stop_requested, true);
    assert.equal((await evaluate(`(${pageAction})('arm')`)).activity.stop_requested, false);
    await evaluate(`document.querySelector('form [contenteditable="true"]').textContent=''; fixtureMessages.push({id:'a2',author:{role:'assistant'},channel:'final',status:'in_progress',end_turn:false,content:{parts:['未完成']}});`);
    assert.equal((await evaluate(`(${pageAction})()`)).reply.terminal, false);
    // 同一未完成回合只有 Server 授权的恢复发送可通过；仍检查草稿和审批。
    const partial = await evaluate(`(${pageAction})()`);
    const recovery = { ...sendInput, replyId: partial.reply.id, replyText: partial.reply.text, recovery: true };
    assert.equal((await evaluate(`(${pageAction})('send',${JSON.stringify({ ...recovery, recovery: false })})`)).sent, false);
    await evaluate(`{const card=document.createElement('div');card.dataset.codexApprovalSurface='true';card.innerHTML='<button>允许</button>';document.body.appendChild(card);}`);
    assert.equal((await evaluate(`(${pageAction})()`)).approval, true);
    assert.equal((await evaluate(`(${pageAction})('send',${JSON.stringify(recovery)})`)).sent, false);
    await evaluate(`document.querySelector('[data-codex-approval-surface]').remove()`);
    assert.equal((await evaluate(`(${pageAction})('send',${JSON.stringify(recovery)})`)).sent, true);
    await evaluate(`document.querySelector('form [contenteditable="true"]').textContent=''`);
    await evaluate(`document.querySelector('article').__reactFiber$test.memoizedProps={entry:{conversationId:'${CONVERSATION}',turn:{status:'complete',items:[{type:'user-message',messageId:'u2',message:'新任务'},{type:'assistant-message',messageId:'a3',content:'最终回复',phase:'final_answer',completed:true}]}}};`);
    const shell = await evaluate(`(${pageAction})()`);
    assert.equal(shell.userId, 'u2'); assert.equal(shell.reply.id, 'a3'); assert.equal(shell.reply.terminal, true);
    // 模拟 DOM 仍指向旧 Fiber、root.current 已切换；旧 terminal 不得获得发送权。
    await evaluate(`{
      const old=document.querySelector('article').__reactFiber$test;
      const newer={tag:5,memoizedProps:{conversationId:'${CONVERSATION}',turn:{messages:[fixtureMessages[0],fixtureMessages.at(-1)]}}};
      old.tag=5; old.alternate=newer; newer.alternate=old;
      const state={}; const a={tag:3,stateNode:state,child:old}; const b={tag:3,stateNode:state,child:newer};
      a.alternate=b; b.alternate=a; old.return=a; newer.return=b; state.current=b; window.testRoot=state; true;
    }`);
    const committed = await evaluate(`(${pageAction})()`);
    assert.equal(committed.reply.id, 'a2'); assert.equal(committed.reply.terminal, false);
    await evaluate('testRoot.current.child=null');
    assert.equal((await evaluate(`(${pageAction})()`)).supported, false);

    // popup 使用 Chrome API fake；它只记录指令，绝不将按钮操作发送给真实会话。
    await cdp.call('Page.addScriptToEvaluateOnNewDocument', { source: `globalThis.configureFailure=true;globalThis.permissionGranted=true;globalThis.savedConnection=true;globalThis.chrome={
      storage:{local:{get:async()=>({}),set:async()=>{}},session:{get:async()=>({}),set:async value=>{globalThis.connectionDraft=value},remove:async()=>{globalThis.connectionDraft=null}},onChanged:{addListener:()=>{}}},
      tabs:{query:async()=>[{id:7,url:'https://chatgpt.com/c/${CONVERSATION}'}]},
      permissions:{request:async()=>globalThis.permissionGranted},
      runtime:{sendMessage:async message=>{(globalThis.commands??=[]).push(message);
        if(message.action==='connection')return {ok:true,url:'http://127.0.0.1:8080',tokenSaved:globalThis.savedConnection};
        if(message.action==='configure'&&globalThis.configureFailure)return {ok:false,error:'continuation_account_required'};
        return {ok:true,tasks:[],connected:true,url:message.connection?.url||'http://127.0.0.1:8080',tokenSaved:true}}}
    };` }, sessionId);
    await cdp.call('Page.navigate', { url: `${origin}/popup.html` }, sessionId);
    await until(() => evaluate('document.querySelector("#status")?.textContent.includes("尚未启用")'));
    await cdp.call('Emulation.setDeviceMetricsOverride', { width: 400, height: 650, deviceScaleFactor: 1, mobile: false }, sessionId);
    await cdp.call('Emulation.setEmulatedMedia', { features: [{ name: 'prefers-color-scheme', value: 'light' }] }, sessionId);
    assert.equal(await evaluate('getComputedStyle(document.body).backgroundColor'), 'rgb(248, 250, 251)');
    assert.equal(await evaluate('document.documentElement.scrollWidth <= innerWidth'), true);
    await evaluate('document.querySelector("#connectionPanel").open=true');
    assert.equal(await evaluate('document.documentElement.scrollWidth <= innerWidth'), true);
    assert.equal(await evaluate('document.querySelector("#serverToken").type'), 'password');
    assert.match(await evaluate('document.querySelector("#connectionStatus").textContent'), /已连接/);
    assert.equal(await evaluate('document.querySelector("#serverToken").placeholder'), '已保存 PAT');
    // 认证和权限失败都保留原输入；成功后用保存提示代替空白，无需用户猜测结果。
    await evaluate(`document.querySelector('#serverToken').value='wc_pat_'+'x'.repeat(64);document.querySelector('#connectionForm').requestSubmit();`);
    await until(() => evaluate('document.querySelector("#connectionStatus").dataset.state === "failed"'));
    assert.equal(await evaluate('document.querySelector("#serverToken").value.length'), 71);
    assert.match(await evaluate('document.querySelector("#connectionStatus").textContent'), /session:collaborate/);
    assert.equal(await evaluate('document.querySelector("#continue").disabled'), true);
    await evaluate(`permissionGranted=false;document.querySelector('#connectionForm').requestSubmit();`);
    await until(() => evaluate('document.querySelector("#connectionStatus").textContent.includes("未允许")'));
    assert.equal(await evaluate('document.querySelector("#serverToken").value.length'), 71);
    await evaluate(`permissionGranted=true;configureFailure=false;document.querySelector('#connectionForm').requestSubmit();`);
    await until(() => evaluate('document.querySelector("#connectionStatus").dataset.state === "connected"'));
    assert.equal(await evaluate('document.querySelector("#serverToken").value'), '');
    assert.equal(await evaluate('connectionDraft'), null);
    assert.equal(await evaluate('document.querySelector("#serverToken").required'), false);
    assert.match(await evaluate('document.querySelector("#tokenHint").textContent'), /已保存/);
    await evaluate(`document.querySelector('#serverUrl').value='http://10.28.221.1:18080';document.querySelector('#serverUrl').dispatchEvent(new Event('input'));`);
    assert.equal(await evaluate('document.querySelector("#serverToken").required'), true);
    await evaluate(`document.querySelector('#serverUrl').value='http://127.0.0.1:8080';document.querySelector('#serverUrl').dispatchEvent(new Event('input'));`);
    await evaluate('document.querySelector("#connectionPanel").open=false');
    assert.equal(await evaluate('document.querySelector("#continue").disabled'), false);
    // 展开的恢复分组遵守相同宽度；finish 允许当前聊天关联 Session，无需用户手填身份。
    await evaluate(`document.querySelector('#recoverySettings').open=true;document.querySelector('#trigger').value='finish';document.querySelector('#trigger').dispatchEvent(new Event('change'));`);
    assert.equal(await evaluate('document.querySelector("#sessionId").required'), false);
    assert.equal(await evaluate('document.querySelector("#sessionId").checkValidity()'), true);
    assert.equal(await evaluate('document.documentElement.scrollWidth <= innerWidth'), true);
    await evaluate(`document.querySelector('#trigger').value='after_turn';document.querySelector('#trigger').dispatchEvent(new Event('change'));`);
    assert.equal(await evaluate('document.querySelector("#sessionId").required'), false);
    if (process.env.CONTINUATION_SCREENSHOT) {
      const metrics = await cdp.call('Page.getLayoutMetrics', {}, sessionId);
      const screenshot = await cdp.call('Page.captureScreenshot', { format: 'png', captureBeyondViewport: true,
        clip: { x: 0, y: 0, width: 400, height: Math.ceil(metrics.cssContentSize.height), scale: 1 } }, sessionId);
      await writeFile(process.env.CONTINUATION_SCREENSHOT.replace(/\.png$/, '-light.png'), Buffer.from(screenshot.data, 'base64'));
    }
    await evaluate('document.querySelector("#continue").click()');
    await until(() => evaluate('commands.some(message=>message.action === "continue")'));
    await evaluate(`document.querySelector('#objective').value='只修改当前问题';document.querySelector('#mode').value='loop';document.querySelector('#settings').dispatchEvent(new Event('submit',{bubbles:true,cancelable:true}));`);
    await until(() => evaluate('commands.some(message=>message.action === "start")'));
    assert.deepEqual(await evaluate('commands.find(message=>message.action === "start").options'),
      { objective: '只修改当前问题', mode: 'loop', limit: 100, delay: 30, policy: { recovery: true, continuous: true, adaptive_timing: true, runner_id: null, keep_awake: false, silence_seconds: 600, lifetime_hours: 24, trigger: 'after_turn', session_id: null } });
    await evaluate('document.querySelector("#language").click()');
    await until(() => evaluate('document.documentElement.lang === "en"'));
    assert.match(await evaluate('document.querySelector("#continue").textContent'), /recover/);
    await cdp.call('Emulation.setEmulatedMedia', { features: [{ name: 'prefers-color-scheme', value: 'dark' }, { name: 'prefers-reduced-motion', value: 'reduce' }] }, sessionId);
    assert.equal(await evaluate('getComputedStyle(document.body).backgroundColor'), 'rgb(21, 27, 35)');
    assert.equal(await evaluate('getComputedStyle(document.querySelector("button")).transitionDuration'), '0s');
    await cdp.call('Emulation.setDeviceMetricsOverride', { width: 280, height: 700, screenWidth: 280, screenHeight: 700, deviceScaleFactor: 1, mobile: true }, sessionId);
    assert.equal(await evaluate('document.documentElement.scrollWidth <= innerWidth'), true);
    if (process.env.CONTINUATION_SCREENSHOT) {
      const screenshot = await cdp.call('Page.captureScreenshot', { format: 'png' }, sessionId);
      await writeFile(process.env.CONTINUATION_SCREENSHOT, Buffer.from(screenshot.data, 'base64'));
    }
    // 加载生产扩展并打开真实 action popup，不用普通网页的固定视口替代尺寸验收。
    const { id } = await cdp.call('Extensions.loadUnpacked', { path: ROOT });
    const worker = await until(async () => (await cdp.call('Target.getTargets')).targetInfos.find(info => info.type === 'service_worker' && info.url === `chrome-extension://${id}/worker.mjs`));
    const workerSession = await cdp.call('Target.attachToTarget', { targetId: worker.targetId, flatten: true });
    await cdp.call('Runtime.enable', {}, workerSession.sessionId);
    await cdp.call('Runtime.runIfWaitingForDebugger', {}, workerSession.sessionId);
    await until(async () => (await cdp.call('Runtime.evaluate', { expression: 'typeof chrome !== "undefined" && !!chrome.storage', returnByValue: true }, workerSession.sessionId)).result.value);
    const seeded = await cdp.call('Runtime.evaluate', { expression: `chrome.storage.local.set(${JSON.stringify({
      'continuation.connection.v2': { url: origin, token: 'wc_pat_' + 'x'.repeat(64) },
    })})`, awaitPromise: true, returnByValue: true }, workerSession.sessionId);
    assert.equal(seeded.exceptionDetails, undefined);
    /** 打开 action 并附着真实文档；expectDraft 指定重开后是否应恢复会话内 PAT 草稿。 */
    const openPopup = async (expectDraft = false) => {
      await cdp.call('Target.activateTarget', { targetId });
      // 原生弹窗关闭后异步释放 action；只在测试中有界等待它重新可打开，不发送任何会话动作。
      await until(async () => {
        const opened = await cdp.call('Runtime.evaluate', { expression: 'chrome.action.openPopup()', awaitPromise: true, returnByValue: true }, workerSession.sessionId);
        return !opened.exceptionDetails;
      });
      const target = await until(async () => (await cdp.call('Target.getTargets')).targetInfos.find(info => info.url === `chrome-extension://${id}/popup.html`));
      const attached = await cdp.call('Target.attachToTarget', { targetId: target.targetId, flatten: true });
      /** 仅评估隔离扩展文档；执行异常必须作为测试失败暴露。 */
      const read = async expression => {
        const result = await cdp.call('Runtime.evaluate', { expression, awaitPromise: true, returnByValue: true }, attached.sessionId);
        if (result.exceptionDetails) throw new Error(result.result.description);
        return result.result.value;
      };
      await until(() => read('document.querySelector("#connectionStatus")?.dataset.state === "connected"'));
      assert.equal(await read('document.body.getBoundingClientRect().width'), 400, '真实 popup 内容宽度不包含浏览器原生滚动条');
      assert.equal(await read('document.documentElement.scrollWidth <= innerWidth'), true);
      assert.equal(await read('document.querySelector("#serverToken").value'), expectDraft ? 'wc_pat_' + 'y'.repeat(64) : '');
      assert.match(await read('document.querySelector("#serverToken").placeholder'), /已保存/);
      return { target, read, sessionId: attached.sessionId };
    };
    const popup = await openPopup();
    if (process.env.CONTINUATION_SCREENSHOT) {
      const screenshot = await cdp.call('Page.captureScreenshot', { format: 'png' }, popup.sessionId);
      await writeFile(process.env.CONTINUATION_SCREENSHOT.replace(/\.png$/, '-popup.png'), Buffer.from(screenshot.data, 'base64'));
    }
    await popup.read(`document.querySelector('#serverToken').value='wc_pat_'+'y'.repeat(64);document.querySelector('#serverToken').dispatchEvent(new Event('input'));true`);
    await until(async () => (await cdp.call('Runtime.evaluate', {
      expression: 'chrome.storage.session.get("continuation.connection-draft.v1").then(value=>value["continuation.connection-draft.v1"]?.token.length===71)',
      awaitPromise: true, returnByValue: true,
    }, workerSession.sessionId)).result.value);
    await popup.read('window.close(); true');
    await until(async () => !(await cdp.call('Target.getTargets')).targetInfos.some(info => info.targetId === popup.target.targetId));
    const reopened = await openPopup(true);
    assert.equal(await reopened.read('document.querySelector("#serverUrl").value'), origin);
    assert.equal(await reopened.read('document.querySelector("#serverToken").value'), 'wc_pat_' + 'y'.repeat(64));
  } finally {
    if (cdp) {
      try { await cdp.call('Browser.close'); } catch (error) { console.warn('Browser close request failed:', error.message); }
      cdp.close();
    }
    if (browser.exitCode === null) { browser.kill(); await Promise.race([once(browser, 'exit'), new Promise(resolve => setTimeout(resolve, 3000))]); }
    await new Promise(resolve => server.close(resolve));
    // profile 来自本测试 mkdtemp，解析后必须仍位于系统临时目录才能递归清理。
    const relative = path.relative(path.resolve(tmpdir()), path.resolve(profile));
    if (!relative.startsWith('..') && !path.isAbsolute(relative) && path.basename(profile).startsWith('webcodex-continuation-'))
      await rm(profile, { recursive: true, force: true, maxRetries: 3, retryDelay: 100 });
  }
});

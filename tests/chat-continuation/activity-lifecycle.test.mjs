/** 隔离执行生产观察器；受控计时器和 Chrome fake 验证重载清理，不接触个人页面或凭据。 */
import test from 'node:test';
import assert from 'node:assert/strict';
import { readFile } from 'node:fs/promises';
import vm from 'node:vm';

/** 与生产观察器相同的源码，每个测试文档拥有独立的隔离全局。 */
const SOURCE = await readFile(new URL('../../extensions/chat-continuation/activity.js', import.meta.url), 'utf8');

/** mode 控制 Chrome 同步失败、异步拒绝或正常响应；监听器和计时器由夹具显式推进。 */
function fixture(mode = 'ok') {
  const intervals = new Map(), timeouts = new Map(), documentEvents = new Map(), windowEvents = new Map();
  const calls = [], warnings = [];
  let sequence = 0, connected = false, mutation;
  /** events 是目标上的监听集合；同一类型只有生产脚本的一个监听器。 */
  const target = events => ({ addEventListener(name, callback) { events.set(name, callback); }, removeEventListener(name, callback) { if (events.get(name) === callback) events.delete(name); } });
  const runtime = { id: 'isolated-extension', sendMessage(message) {
    calls.push(message);
    if (mode === 'throw') throw new Error('Extension context invalidated.');
    if (mode === 'reject') return Promise.reject(new Error('Extension context invalidated.'));
    if (mode === 'temporary') return Promise.reject(new Error('Receiving end does not exist.'));
    return Promise.resolve();
  } };
  const sandbox = {
    chrome: { runtime }, location: { pathname: '/' }, TextEncoder,
    document: { ...target(documentEvents), querySelector() { return { innerText: 'first request' }; } },
    window: target(windowEvents), console: { warn(message) { warnings.push(message); } },
    MutationObserver: class {
      /** callback 为生产变化通知；夹具保留它以验证清理后晚到通知不能恢复计时器。 */
      constructor(callback) { mutation = callback; }
      /** 只记录资源是否被生产代码持有，不模拟页面正文。 */
      observe() { connected = true; }
      /** 断开后不再产生主动通知；已排队回调由测试显式调用。 */
      disconnect() { connected = false; }
    },
    setInterval(callback) { const id = ++sequence; intervals.set(id, callback); return id; },
    clearInterval(id) { intervals.delete(id); },
    setTimeout(callback) { const id = ++sequence; timeouts.set(id, callback); return id; },
    clearTimeout(id) { timeouts.delete(id); },
  };
  const context = vm.createContext(sandbox);
  vm.runInContext(SOURCE, context);
  return { calls, warnings, intervals, timeouts, documentEvents, windowEvents, runtime,
    get connected() { return connected; },
    setMode(value) { mode = value; },
    inject() { vm.runInContext(SOURCE, context); },
    change() { mutation(); },
    dispose() { context.__webcodexContinuationObserver?.dispose(); },
    /** kind 选择真实事件入口；fake 只模拟浏览器已经确认 isTrusted 的输入。 */
    trigger(kind) {
      if (kind === 'heartbeat') return intervals.values().next().value?.();
      if (kind === 'popstate') return windowEvents.get('popstate')?.({ isTrusted: true });
      if (kind === 'enter') return documentEvents.get('keydown')?.({ key: 'Enter', isTrusted: true, target: { closest() { return {}; } } });
      const button = { disabled: false, getAttribute() { return null; }, matches(selector) { return kind === 'stop' ? selector.includes('stop-button') : kind === 'send' && selector.includes('send-button'); } };
      documentEvents.get('click')?.({ isTrusted: true, target: { closest(selector) { return selector === 'button' ? button : kind === 'navigate' ? {} : null; } } });
    },
  };
}

/** 等待 fake Promise 拒绝及 finally 执行，不使用定时等待推断竞态结果。 */
async function settled() { await new Promise(resolve => setImmediate(resolve)); }

/** app 必须释放全部文档资源；晚到变化和旧心跳不允许重新申请资源或发送。 */
function assertDisposed(app) {
  assert.equal(app.connected, false);
  assert.equal(app.intervals.size, 0);
  assert.equal(app.timeouts.size, 0);
  assert.equal(app.documentEvents.size, 0);
  assert.equal(app.windowEvents.size, 0);
  const calls = app.calls.length;
  app.change(); app.trigger('heartbeat'); app.dispose();
  assert.equal(app.timeouts.size, 0);
  assert.equal(app.calls.length, calls);
  assert.deepEqual(app.warnings, []);
}

// 六条事件入口的同步抛错和异步拒绝均必须被接住，并关闭当前观察器而不重复报错。
test('扩展上下文失效时所有页面消息入口清理资源且无未捕获异常', async () => {
  for (const mode of ['throw', 'reject']) {
    for (const kind of ['heartbeat', 'stop', 'send', 'enter', 'navigate', 'popstate']) {
      const app = fixture();
      await settled(); app.change(); app.setMode(mode);
      assert.doesNotThrow(() => app.trigger(kind));
      await settled();
      assertDisposed(app);
    }
  }
});

// 注入时已失效，以及运行中 runtime.id 消失，都不调用失效的消息 API。
test('runtime身份消失时立即释放观察器，初始化同步失效同样安全', async () => {
  const app = fixture(); await settled(); app.runtime.id = undefined;
  app.trigger('heartbeat'); await settled();
  assert.equal(app.calls.length, 1); assertDisposed(app);
  const invalid = fixture('throw'); await settled(); assertDisposed(invalid);
});

// 临时 worker 不可达不等于扩展卸载：必须告警、释放 pending，并允许下个心跳成功重试。
test('临时桥接失败保留观察器并在下个心跳重试', async () => {
  const app = fixture('temporary'); await settled();
  assert.equal(app.connected, true); assert.equal(app.warnings.length, 1);
  app.setMode('ok'); app.trigger('heartbeat'); await settled();
  assert.equal(app.calls.length, 2); assert.equal(app.connected, true);
  app.dispose();
});

// 同版本重复注入不增加心跳；失效清理后再注入只建立一套新监听和计时器。
test('重复注入幂等，失效清理后可建立新的观察器', async () => {
  const app = fixture(); await settled();
  app.inject(); assert.equal(app.intervals.size, 1); assert.equal(app.calls.length, 1);
  app.setMode('throw'); app.trigger('heartbeat'); await settled(); assertDisposed(app);
  app.setMode('ok'); app.inject(); await settled();
  assert.equal(app.connected, true); assert.equal(app.intervals.size, 1); assert.equal(app.documentEvents.size, 2);
  app.dispose();
});

/** 浏览器端仅验证站点和会话路由；任务状态和预算验证见 Rust 后端测试。 */
import test from 'node:test';
import assert from 'node:assert/strict';
import { chatLocation, privateHttpHost, serverOrigin } from '../../extensions/chat-continuation/core.mjs';

/** 固定合成会话 ID，不指向真实聊天。 */
const CONVERSATION = '11111111-1111-4111-8111-111111111111';

test('只识别明确的 ChatGPT 会话和项目，其他站点路径及凭据 URL 被拒绝', () => {
  for (const url of ['https://chatgpt.com.evil.test/c/' + CONVERSATION, 'https://user@chatgpt.com/c/' + CONVERSATION,
    'https://chatgpt.com/settings', 'https://chatgpt.com/', 'http://chatgpt.com/c/' + CONVERSATION]) assert.equal(chatLocation(url), null);
  assert.deepEqual(chatLocation(`https://chatgpt.com/g/g-p-project/c/${CONVERSATION}`),
    { origin: 'https://chatgpt.com', prefix: '/g/g-p-project', conversation: CONVERSATION });
});

/** 覆盖三个 RFC 1918 网段边界；公网、链路本地、DNS 名称和 IPv6 均不放宽。 */
test('HTTP 只接受本机与私网 IPv4，拒绝公网及 DNS 重绑定入口', () => {
  for (const host of ['localhost', '127.0.0.1', '10.28.221.1', '172.16.0.1', '172.31.255.255', '192.168.1.1']) assert.equal(privateHttpHost(host), true, host);
  for (const host of ['8.8.8.8', '172.15.255.255', '172.32.0.1', '192.169.0.1', '169.254.1.1', 'router.local', 'example.com', '::1', '10.999.1.1', '10..1.1', '10.1e2.1.1']) assert.equal(privateHttpHost(host), false, host);
});

/** 权限申请和 worker 共用同一校验；无效地址不获得站点权限，也不会收到 PAT。 */
test('Server origin 拒绝公网 HTTP、凭据、路径和不完整地址', () => {
  assert.equal(serverOrigin('http://10.28.221.1:18080'), 'http://10.28.221.1:18080');
  assert.equal(serverOrigin('https://server.example/'), 'https://server.example');
  for (const input of ['http://8.8.8.8', 'http://router.local', 'http://user@10.28.221.1', 'http://10.28.221.1/api', 'https://server.example/?key=x', 'invalid', 'file:///tmp'])
    assert.throws(() => serverOrigin(input), /connection_invalid/);
});

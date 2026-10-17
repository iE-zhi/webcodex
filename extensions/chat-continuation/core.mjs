/** 浏览器端校验会话路由及私网连接范围；续跑状态、预算和决策统一由 Server 持有。 */

/** 只允许 loopback 或 RFC 1918 的 IPv4 字面地址使用 HTTP；拒绝依赖 DNS 的私网判断。 */
export function privateHttpHost(hostname) {
  if (hostname === 'localhost' || hostname === '127.0.0.1') return true;
  if (!/^\d{1,3}(?:\.\d{1,3}){3}$/.test(hostname)) return false;
  const parts = hostname.split('.').map(Number);
  if (parts.length !== 4 || !parts.every(part => Number.isInteger(part) && part >= 0 && part <= 255)) return false;
  // RFC 1918 的三个私网范围：10/8、172.16/12、192.168/16。
  return parts[0] === 10 || parts[0] === 172 && parts[1] >= 16 && parts[1] <= 31 || parts[0] === 192 && parts[1] === 168;
}

/** 校验用户填写的 Server origin；失败抛固定错误，调用方必须在申请站点权限前执行。 */
export function serverOrigin(value) {
  let url;
  try { url = new URL(value); }
  catch { throw new Error('connection_invalid'); }
  if (!['https:', 'http:'].includes(url.protocol) || url.username || url.password || url.search || url.hash || url.pathname !== '/' ||
      url.protocol === 'http:' && !privateHttpHost(url.hostname)) throw new Error('connection_invalid');
  return url.origin;
}

/** 解析允许的 ChatGPT origin、项目路径及会话 UUID，拒绝其他页面或凭据 URL。 */
export function chatLocation(value) {
  try {
    const url = new URL(value);
    if (!['https://chatgpt.com', 'https://chat.openai.com'].includes(url.origin) || url.username || url.password) return null;
    const match = url.pathname.match(/^(\/g\/[^/]+)?\/c\/([a-f0-9-]{36})\/?$/i);
    if (!match || !/^[a-f\d]{8}(?:-[a-f\d]{4}){3}-[a-f\d]{12}$/i.test(match[2])) return null;
    return { origin: url.origin, prefix: match[1] || '', conversation: match[2] };
  } catch {
    // 无法解析的 URL 不具有会话身份。
    return null;
  }
}

/** 只接受尚无会话 ID 的新聊天入口；不把搜索、设置或历史会话当成可预置页面。 */
export function newChatLocation(value) {
  try {
    const url = new URL(value);
    if (!['https://chatgpt.com', 'https://chat.openai.com'].includes(url.origin) || url.username || url.password || url.search || url.hash) return null;
    const match = url.pathname.match(/^(\/g\/[A-Za-z0-9_-]+)?\/?$/);
    return match ? { origin: url.origin, prefix: match[1] || '', conversation: null } : null;
  } catch { return null; /* 不完整地址没有预置授权。 */ }
}

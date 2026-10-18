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

/** 比较同一已校验 origin 下的聊天范围；仅固定项目 ID 相同才允许名称后缀变化，普通/GPT 路径仍须完全一致。 */
export function sameChatScope(left, right) {
  if (typeof left !== 'string' || typeof right !== 'string') return false;
  if (left === right) return true;
  // 项目身份只取 g-p- 后的三十二位十六进制 ID；后缀限于单个路径组件，不接受嵌套路径。
  const project = /^\/g\/g-p-([a-f\d]{32})(?:-[A-Za-z0-9_-]+)?$/i;
  // 两侧都必须能提取固定 ID，不能把普通 GPT 或缺失范围归入项目别名。
  const first = left.match(project), second = right.match(project);
  return Boolean(first && second && first[1].toLowerCase() === second[1].toLowerCase());
}

/** 接受普通/GPT 新聊天和明确的项目首页；项目首页统一返回不含 /project 的绑定前缀，其他页面拒绝预置。 */
export function newChatLocation(value) {
  try {
    const url = new URL(value);
    if (!['https://chatgpt.com', 'https://chat.openai.com'].includes(url.origin) || url.username || url.password || url.search || url.hash) return null;
    const match = url.pathname.match(/^(\/g\/[A-Za-z0-9_-]+)?\/?$/)
      || url.pathname.match(/^(\/g\/g-p-[a-f\d]{32}(?:-[A-Za-z0-9_-]+)?)\/project\/?$/i);
    return match ? { origin: url.origin, prefix: match[1] || '', conversation: null } : null;
  } catch { return null; /* 不完整地址没有预置授权。 */ }
}

//! 浏览器续跑的 Runner 租约协议；只提供有界唤醒和系统防休眠，不传递模型或 Runner 凭据。
use serde::{Deserialize, Serialize};

/// 单次防休眠租约最长四十五秒；Runner 使用单调时钟扣除 HTTP 耗时，失联不会永久持有。
pub const LOCAL_LEASE_MS: u32 = 45_000;
/// 每次最多唤醒四个已授权会话，任务总恢复次数仍由 Server 持久化控制。
pub const MAX_LOCAL_WAKES: usize = 4;

/// 本机适配器的固定健康状态；不携带操作系统异常正文、路径或凭据，不等同于模型执行状态。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LocalStatus {
    /// 最近一次操作未报告失败；不声称系统电源已经处于防休眠状态。
    #[default]
    Ready,
    /// 操作系统或固定平台工具未能建立/释放电源资源，用户需检查 Runner 日志。
    PowerUnavailable,
    /// 本机浏览器配置、启动或句柄回收失败，没有重放任何发送动作。
    BrowserUnavailable,
    /// Server 租约暂不可读取或认证/实例被拒绝，旧电源资源已撤销。
    LeaseUnavailable,
    /// 返回的 nonce、大小、期限或 URL 不符合协议，整次返回已拒绝。
    ProtocolError,
}

/// Runner 使用自己的 agent token 请求租约；认证层必须重验账户、client 和活动进程实例。
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LocalLeaseRequest {
    /// 上一次本机操作的固定结果；缺省 ready，故障只作为本账户界面反馈。
    #[serde(default)]
    pub status: LocalStatus,
    /// 本机 Runner 的注册身份，不接受其他 Runner 身份。
    pub client_id: String,
    /// 当前 Runner 进程 UUID；旧实例不能继承新实例的租约。
    pub agent_instance_id: String,
    /// 每次请求的新 UUID，响应必须原样回传，拒绝旧响应或代理缓存重放。
    pub request_id: String,
    /// Runner 已确认本地配置的浏览器进程完全消失；未知或无权限时必须 false。
    pub browser_absent: bool,
}

/// 单个已领取的浏览器唤醒；URL 由 Server 绑定生成，不能替换本机可执行文件或参数。
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BrowserWake {
    /// 所属任务 UUID，不授予发送或工具执行权限。
    pub task_id: String,
    /// 当前任务控制版本；仅作日志关联，不能作为发送授权。
    pub generation: u64,
    /// 已验证的 ChatGPT 会话 HTTPS URL；Runner 仍须重新验证主机和路由。
    pub url: String,
}

/// Server 返回的进程范围租约；不保存到 Runner 磁盘，不跨重启或未知认证状态继续生效。
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LocalLease {
    /// 与本次请求完全相同的 UUID；不匹配则整次拒绝。
    pub request_id: String,
    /// 相对服务端发行时刻的最长毫秒时长，范围零至 LOCAL_LEASE_MS。
    pub valid_for_ms: u32,
    /// 本账户确有绑定此 Runner 的活动任务且用户允许防休眠。
    pub keep_awake: bool,
    /// 最多 MAX_LOCAL_WAKES 个领取成功的 URL；未知领取响应不能重放。
    pub wake: Vec<BrowserWake>,
}

//! Continue、Goal、Loop 的版本化 JSON 契约；浏览器报告只是观测，不授予本地工具权限。
use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// 每个账户最多保存 32 个任务；删除后释放名额。
pub const MAX_TASKS: usize = 32;
/// 每个任务最多发送 10,000 轮，失败的投递也计入。
pub const MAX_ROUNDS: u32 = 10_000;
/// 单个目标最多 8,000 字节；超限拒绝而不截断。
pub const MAX_OBJECTIVE: usize = 8000;
/// 单条公开消息最多 128 KiB，不接受截断的决策依据。
pub const MAX_MESSAGE: usize = 128 * 1024;
/// 单个任务的人工约束和当前公开回执最多 1 MiB，超限暂停而不丢弃人工约束。
pub const MAX_CONTEXT: usize = 1024 * 1024;
/// 旧客户端默认授权时限为 24 小时，单位毫秒；显式策略可覆盖此默认值。
pub const TASK_TTL_MS: i64 = 24 * 60 * 60 * 1000;
/// 发送 90 秒未确认转入待对账状态；保留原回执，禁止自动重放。
pub const INFLIGHT_MS: i64 = 90_000;
/// 未获发送授权的准备最多保留三十秒；超时可安全重新排队。
pub(crate) const PREPARED_MS: i64 = 30_000;
/// 消息和浏览器文档标识最多 200 字节。
const MAX_ID: usize = 200;

/// 续跑方式；序列化使用小写，Loop 不接受模型的完成决定。
#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Mode {
    /// 监看当前回合并恢复中断；成功终结后按 continuous 等待新请求或停止。
    Continue,
    /// 当前 ChatGPT 确认原目标完成后按 continuous 等待新请求或停止。
    Goal,
    /// 在原请求范围内继续，直到人工暂停、阻塞或预算耗尽。
    Loop,
}

/// 一个浏览器实例中的确切会话绑定；可持久化，跨进程必须重新核对同一安装和当前公开回合。
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Binding {
    /// 扩展安装 UUID，保留至用户清除扩展数据。
    pub client: String,
    /// 浏览器进程 UUID，重启后必须改变。
    pub browser: String,
    /// 当前 ChatGPT 会话 UUID。
    pub conversation: String,
    /// 允许的 ChatGPT HTTPS origin。
    pub origin: String,
    /// ChatGPT 项目路径，无项目时为空串。
    pub prefix: String,
    /// 浏览器实例内的标签 ID，不作为账户授权依据。
    pub tab_id: i64,
}

/// 用户明确授权的目标和资源预算；保存及恢复时均校验。
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Options {
    /// 原始目标，不超过 MAX_OBJECTIVE 字节；空串表示沿用当前聊天的人工请求。
    pub objective: String,
    /// 本次控制选择的模式。
    pub mode: Mode,
    /// 允许的投递次数，范围 1～10,000。
    pub limit: u32,
    /// 两次投递最小间隔，范围 30～3,600 秒。
    pub delay: u32,
    /// 恢复、触发和授权时限；旧客户端使用安全默认值。
    #[serde(default)]
    pub policy: Policy,
}

/// 可持久化的恢复策略；不允许通过策略授予工具权限或跨账户访问。
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(default, deny_unknown_fields)]
pub struct Policy {
    /// 明确选择的本机 Runner；None 不唤醒浏览器或持有系统电源，按账户重新授权。
    #[serde(default)]
    pub runner_id: Option<String>,
    /// 仅绑定 Runner 且任务活动时防休眠，默认关闭；不保持显示器常亮。
    #[serde(default)]
    pub keep_awake: bool,
    /// 自动恢复未完成回合，默认开启。
    pub recovery: bool,
    /// 基础静默窗口，120～3,600 秒，默认 600；启用自适应时已知模型可覆盖。
    pub silence_seconds: u32,
    /// 本次授权持续小时数，1～720，默认 24；重绑定不延长。
    pub lifetime_hours: u32,
    /// after_turn 或 finish；后者要求绑定确切 Workflow Session。
    pub trigger: String,
    /// 用户明确固定的后端 Session；None 时由主聊天来源确认自动关联，不推测账户内无关工作。
    pub session_id: Option<String>,
    /// 保持当前聊天的开关；完成后等待新人工消息，默认开启，不自动重开已完成回合。
    pub continuous: bool,
    /// 默认按已知模型及失败类型调整静默窗口；关闭时严格使用 silence_seconds。
    pub adaptive_timing: bool,
}

impl Default for Policy {
    /// 返回十分钟静默恢复和二十四小时授权；不隐式绑定后端 Session。
    fn default() -> Self {
        Self {
            runner_id: None,
            keep_awake: false,
            recovery: true,
            silence_seconds: 600,
            lifetime_hours: (TASK_TTL_MS / 3_600_000) as u32,
            trigger: "after_turn".into(),
            session_id: None,
            continuous: true,
            adaptive_timing: true,
        }
    }
}

/// 公开回复；正常目标决策要求 terminal=true，中断恢复另需持久化刷新和静默证明。
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Reply {
    /// 提供方的确切回复 ID，不使用文本哈希替代身份。
    pub id: String,
    /// 本回复对应的用户消息 ID。
    pub user_id: String,
    /// 完整公开正文；不含私有推理、工具参数或工具结果。
    pub text: String,
    /// 页面模型明确宣告此回合成功终结。
    pub terminal: bool,
}

/// 浏览器提交的一次主文档观测；任何未知或截断状态都不得发送。
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Observation {
    /// 当前主聊天公开进度中的随机来源挑战；只证明该确切用户回合看到了原生绑定结果。
    #[serde(default)]
    pub source_proof: Option<SourceProof>,
    /// 当前页面 URL，必须与绑定的 origin、项目和会话完全对应。
    pub url: String,
    /// 公开页面模型是否获得确切身份。
    pub supported: bool,
    /// 当前仍在生成；中间文字不构成回合结束。
    pub busy: bool,
    /// 输入框已有草稿；禁止覆盖。
    pub draft: bool,
    /// 页面有当前错误或等待人工操作。
    pub blocked: bool,
    /// 当前用户消息 ID。
    pub user_id: String,
    /// 当前用户公开正文，用于投递回执和人工修正。
    pub user_text: String,
    /// 当前最终回复候选；None 表示仍无法获得。
    pub reply: Option<Reply>,
    /// 当前回合明确中断；允许恢复，不等同于人工审批。
    #[serde(default)]
    pub interrupted: bool,
    /// 当前页面或可信后端仍有工具/子任务执行，禁止恢复与发送。
    #[serde(default)]
    pub tools_busy: bool,
    /// 当前等待人工批准或回答，保持启用等待且不能自动批准。
    #[serde(default)]
    pub approval: bool,
    /// 可信后端提供的最近成功 finish 事件 ID；HTTP 层覆盖网页输入。
    #[serde(default)]
    pub finish_id: Option<String>,
    /// Chrome 主文档 ID，用于确认刷新后确实出现新文档；旧客户端缺省为空。
    #[serde(default)]
    pub document_id: String,
    /// 当前页面的非正文活动及停止意图；不含私有推理、工具参数或结果。
    #[serde(default)]
    pub activity: Activity,
}

/// 公开进度中的来源回执；不含控制令牌，服务端只接受当前任务版本和确切用户消息。
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct SourceProof {
    /// 当前续跑任务 UUID，不能引用其他任务。
    pub task_id: String,
    /// 发行挑战时的控制版本，人工变更会使其失效。
    pub generation: u64,
    /// 服务端给当前 MCP 候选 Window 的六十四位随机挑战，不由模型选择。
    pub challenge: String,
    /// 出现该公开进度的当前用户消息 ID，必须等于实际已确认自动投递的用户身份。
    pub user_id: String,
    /// 提供方公开进度消息 ID，只接受本次页面已读取的确切消息。
    pub message_id: String,
}

/// 浏览器公开活动摘要；跨请求只保存版本、类型和身份，不保存私有内容。
#[derive(Clone, Debug, Default, Deserialize, Serialize)]
#[serde(default, deny_unknown_fields)]
pub struct Activity {
    /// 提供方当前回合 ID；无法取得时使用当前用户消息 ID，不作为 MCP 归属证明。
    pub source_turn: String,
    /// 主文档内单调递增进展版本；只由当前回合状态变化推进。
    pub revision: u64,
    /// 用户在当前回合主动点击停止；必须人工重新启用才撤销。
    pub stop_requested: bool,
    /// 当前模型的公开名称，用于选择静默窗口，最多一百字节。
    pub model: String,
    /// 公开模型设置表明使用较长推理窗口。
    pub deliberate: bool,
    /// 页面明确报告推理失败。
    pub thinking_failed: bool,
    /// 页面明确报告速率或访问限制，等待期间不恢复。
    pub rate_limited: bool,
    /// 服务提供方提示的剩余等待秒数；范围零至一天，不扩展任务授权。
    pub retry_after: u32,
}

/// 每次操作都由 HTTP 层附加认证账户；JSON 不能指定或更换账户。
#[derive(Debug, Deserialize, Serialize)]
#[serde(tag = "action", rename_all = "snake_case", deny_unknown_fields)]
pub enum Request {
    /// 明确启用或恢复任务；相同 request_id 幂等，恢复需传当前任务及版本。
    Start {
        /// 本次用户操作的 UUID，重试必须保持相同值。
        request_id: String,
        /// 恢复现有任务时提供 ID，新任务为 None。
        task_id: Option<String>,
        /// 恢复时的旧版本，新任务为 None。
        generation: Option<u64>,
        /// 明确绑定当前浏览器和会话。
        binding: Binding,
        /// 用户选择的模式、目标和预算。
        options: Options,
        /// 当前公开页面状态。
        page: Observation,
    },
    /// 原地更新目标和策略；保留已消耗轮数及授权截止，撤销尚未授权的旧消息。
    Update {
        /// 当前任务 ID。
        task_id: String,
        /// 更新前控制版本。
        generation: u64,
        /// 当前绑定，不能借更新迁移发送者。
        binding: Binding,
        /// 新目标和预算，轮数不能低于已消耗次数。
        options: Options,
        /// 当前完整页面状态，用于防止覆盖并发人工输入。
        page: Observation,
    },
    /// 列出本扩展安装的任务，不返回消息正文、凭据或投递正文。
    List {
        /// 扩展安装 UUID。
        client: String,
    },
    /// 本账户确切任务的无令牌投影，供 MCP 适配核对 Session 与来源。
    Inspect {
        /// 已知任务 UUID；其他账户的任务返回不存在。
        task_id: String,
    },
    /// 提交公开状态并让后端决定是否生成下一条消息。
    Observe {
        /// 任务 ID。
        task_id: String,
        /// 当前控制版本。
        generation: u64,
        /// 当前绑定，不能通过观察迁移浏览器发送权。
        binding: Binding,
        /// 当前公开状态。
        page: Observation,
    },
    /// 对 ready 消息作一次性持久化领取；丢失响应不能再次领取。
    Claim {
        /// 任务 ID。
        task_id: String,
        /// 当前控制版本。
        generation: u64,
        /// 当前绑定。
        binding: Binding,
        /// 此次领取的确切待发消息 ID。
        delivery_id: String,
        /// 重新读取的页面状态，用于阻止草稿或并发人工消息。
        page: Observation,
    },
    /// 准备一次投递；正文可以重新读取，但没有 Authorize 成功不得点击发送。
    Prepare {
        /// 当前任务 ID。
        task_id: String,
        /// 当前控制版本。
        generation: u64,
        /// 当前唯一浏览器绑定。
        binding: Binding,
        /// 确切待发消息 ID。
        delivery_id: String,
        /// 准备前重新取得的完整页面状态。
        page: Observation,
    },
    /// 在真正发送前重新核对任务、页面和后台状态；同一消息只授权一次。
    Authorize {
        /// 当前任务 ID。
        task_id: String,
        /// 当前控制版本。
        generation: u64,
        /// 当前唯一浏览器绑定。
        binding: Binding,
        /// 已准备的确切消息 ID。
        delivery_id: String,
        /// 尚未插入正文时的最新页面状态。
        page: Observation,
    },
    /// 页面明确证明没有点击且自有草稿已清除；未知异常不得使用此操作。
    NotSent {
        /// 当前任务 ID。
        task_id: String,
        /// 当前控制版本。
        generation: u64,
        /// 当前唯一浏览器绑定。
        binding: Binding,
        /// 此次发送授权的消息 ID。
        delivery_id: String,
        /// 清除自有草稿后的完整页面状态。
        page: Observation,
    },
    /// 添加人工输入、完整工作流或由当前 ChatGPT 生成的规划请求；不调用其他模型。
    Queue {
        /// 当前任务 ID。
        task_id: String,
        /// 当前控制版本。
        generation: u64,
        /// 当前绑定。
        binding: Binding,
        /// message、workflow 或 plan；plan 的指令由服务端生成。
        kind: String,
        /// 人工排队正文；只有 message 使用，最多八千字节。
        #[serde(default)]
        text: String,
        /// 完整工作流阶段；只有 workflow 使用，二至十二项。
        #[serde(default)]
        stages: Vec<String>,
        /// after_turn 或 finish；人工消息先于阶段检查点。
        trigger: String,
    },
    /// 删除一条排队输入；已获授权的投递只对账，不能据删除推断未发送。
    Dequeue {
        /// 当前任务 ID。
        task_id: String,
        /// 当前控制版本。
        generation: u64,
        /// 当前绑定。
        binding: Binding,
        /// 要删除的确切排队条目 ID。
        entry_id: String,
    },
    /// 同一扩展安装恢复确切会话；版本、旧绑定和最新用户正文必须一致。
    Rebind {
        /// 当前任务 ID。
        task_id: String,
        /// 当前版本，拒绝并发旧浏览器。
        generation: u64,
        /// 原有持久化绑定。
        binding: Binding,
        /// 同一安装、同一会话的新浏览器/标签绑定。
        replacement: Binding,
        /// 新标签的公开状态。
        page: Observation,
    },
    /// 在持久化次数预算内领取一次刷新或打开标签；领取失败不得操作浏览器。
    Repair {
        /// 当前任务 ID。
        task_id: String,
        /// 当前版本。
        generation: u64,
        /// 原有绑定。
        binding: Binding,
        /// open 或 reload；reload 必须提供完整当前页面。
        kind: String,
        /// open 无页面，reload 必须有页面。
        page: Option<Observation>,
    },
    /// 人工或浏览器错误暂停，晚到的决策和领取因版本变化失效。
    Pause {
        /// 任务 ID。
        task_id: String,
        /// 当前控制版本。
        generation: u64,
        /// 操作的原始浏览器绑定。
        binding: Binding,
        /// 固定暂停原因；省略表示 user，不允许写入任意网页错误正文。
        #[serde(default)]
        reason: Option<String>,
    },
    /// 删除本账户任务与其公开上下文，不影响 ChatGPT 会话或项目。
    Delete {
        /// 任务 ID。
        task_id: String,
        /// 原始扩展安装 UUID。
        client: String,
    },
}

/// 持久化的公开会话核对记录；只记录用户和最终公开回复。
#[derive(Clone, Debug, Deserialize, Serialize)]
pub(crate) struct Message {
    /// 提供方消息 ID，用于消除重复记录。
    pub id: String,
    /// 只允许 user 或 assistant。
    pub role: String,
    /// human、automatic 或 assistant，不能仅凭 user 角色推断人工授权。
    pub source: String,
    /// 完整公开正文，受任务总字节预算限制。
    pub content: String,
}

/// 后端持有的待发消息；Claim 之前不向扩展暴露正文。
#[derive(Clone, Debug, Deserialize, Serialize)]
pub(crate) struct Pending {
    /// 一次性投递 UUID，也用于浏览器用户消息回执。
    pub id: String,
    /// 后端生成的纯文本指令，不含凭据。
    pub text: String,
    /// 排队条目 ID；None 表示自动恢复或 Goal 续跑，不因未授权准备而出队。
    #[serde(default)]
    pub queue_id: Option<String>,
    /// 准备后不可变的完整发送正文，包含原生令牌；新 ID 重试时重新生成。
    #[serde(default)]
    pub wire: Option<String>,
}

impl Pending {
    /// 完整回执正文；旧账本缺少 wire 时沿用原协议，不截断或更改身份。
    pub(crate) fn wire_text(&self) -> String {
        self.wire
            .clone()
            .unwrap_or_else(|| format!("[[WC_SEND:{}]]\n{}", self.id, self.text))
    }
}

/// 一条持久化输入；由账户明确排队或当前聊天规划产生，不包含工具执行权。
#[derive(Clone, Debug, Deserialize, Serialize)]
pub(crate) struct QueuedInput {
    /// 服务端生成的稳定 UUID，直到确切接收回执或人工删除才移除。
    pub id: String,
    /// message、stage 或 plan；人工 message/plan 优先于 stage。
    pub kind: String,
    /// 完整待发正文，受整个任务队列字节数限制。
    pub text: String,
    /// after_turn 或 finish；后者只由可信原生 finish 事实解锁。
    pub trigger: String,
    /// queued、native 或 uncertain；未知工具结果不能换通道重发。
    #[serde(default = "queued_delivery")]
    pub delivery: String,
}

/// 旧队列记录默认尚未投递，仅用于新增状态字段的兼容。
fn queued_delivery() -> String {
    "queued".into()
}

/// 面板中的有界队列投影；无发送权，不返回未领取的完整自动指令。
#[derive(Debug, Deserialize, Serialize)]
pub struct QueueView {
    /// 队列条目稳定 ID，用于精确删除。
    pub id: String,
    /// message、stage 或 plan。
    pub kind: String,
    /// 正文前八十个字符，仅用于账户自己的面板显示。
    pub preview: String,
    /// after_turn 或 finish。
    pub trigger: String,
    /// queued、native 或 uncertain，区分等待投递和等待核对。
    #[serde(default)]
    pub delivery: String,
}

/// SQLite JSON 任务记录；账户由数据库复合主键隔离，不接受客户端覆盖。
#[derive(Clone, Debug, Deserialize, Serialize)]
pub(crate) struct Task {
    /// 稳定任务 UUID。
    pub id: String,
    /// 最近一次启用操作 UUID，用于安全重试。
    pub request_id: String,
    /// 启停控制版本，旧操作不能变更新的启用版本。
    pub generation: u64,
    /// 当前会话的唯一发送者绑定。
    pub binding: Binding,
    /// 目标、模式和资源预算。
    pub options: Options,
    /// running、ready、dispatching、uncertain、paused 或 done；旧 deciding 记录仅供升级恢复。
    pub state: String,
    /// 安全状态码，不保存网络异常正文。
    pub reason: String,
    /// 已领取发送次数，包括结果未知的投递。
    pub rounds: u32,
    /// Unix 毫秒授权截止时间。
    pub expires_at: i64,
    /// Unix 毫秒最早领取时间。
    pub next_at: i64,
    /// 最近状态转换的 Unix 毫秒时间，用于超时恢复。
    pub changed_at: i64,
    /// 当前允许继续的确切用户消息 ID。
    pub expected_user: String,
    /// 当前用户公开正文的 SHA-256，用于识别同一消息 ID 的人工编辑，不作为授权。
    pub expected_user_hash: String,
    /// 已消费的公开最终回复 ID，None 表示尚未消费。
    pub last_reply: Option<String>,
    /// 待发或待确认消息；未知投递时保留，不自动重发。
    pub pending: Option<Pending>,
    /// 有界公开会话记录，由后端维护。
    pub messages: Vec<Message>,
    /// 当前会话标记协议版本；旧记录缺省为 0，恢复前必须显式重新启用。
    #[serde(default)]
    pub protocol_version: u8,
    /// 本次启用的协议提示已获确切发送回执，之后才读取完成标记。
    #[serde(default)]
    pub protocol_armed: bool,
    /// 当前未完成回合的进展和恢复预算；跨进程保存，避免重启重置次数。
    #[serde(default)]
    pub recovery: Recovery,
    /// 本次消息的准备、授权和确定未发送次数；旧记录默认为未授权。
    #[serde(default)]
    pub delivery: Delivery,
    /// 最近有效页面观测的 Unix 毫秒时间；零表示尚无观测。
    #[serde(default)]
    pub observed_at: i64,
    /// 绑定 Runner 最近成功领取租约的服务端 Unix 毫秒时间；零表示从未接管。
    #[serde(default)]
    pub local_at: i64,
    /// 绑定 Runner 上次操作的固定健康状态；默认 ready 不表示已获得系统电源锁。
    #[serde(default)]
    pub local_status: webcodex_core::continuation::LocalStatus,
    /// Goal 最近三轮的规范化公开答案摘要；Loop 不使用此保护。
    #[serde(default)]
    pub answer_keys: Vec<String>,
    /// 持久化输入队列；领取不删除，确切用户消息回执才出队，容量由 queue 模块限制。
    #[serde(default)]
    pub queue: Vec<QueuedInput>,
    /// 已收到当前 ChatGPT 规划请求的发送回执，下一终结回复必须给出匹配规划。
    #[serde(default)]
    pub planning: bool,
    /// 已确认规划请求的原触发器；None 兼容旧记录，不能被之后的策略更新改写。
    #[serde(default)]
    pub planning_trigger: Option<String>,
    /// 当前聊天的原生控制，随机令牌只保存于私有账本和本回合指令。
    #[serde(default)]
    pub native: crate::native::NativeState,
    /// 最多六十四个公开进展变化，跨重启保留；不含任何控制令牌或消息正文。
    #[serde(default)]
    pub journal: Vec<crate::journal::ProgressEvent>,
}

/// 当前投递的持久化状态；授权后只允许回执对账或确切未发送证明，不能自动重放。
#[derive(Clone, Debug, Default, Deserialize, Serialize)]
pub(crate) struct Delivery {
    /// 未获授权的准备时间，Unix 毫秒；零表示未准备。
    pub prepared_at: i64,
    /// 发送授权提交时间，Unix 毫秒；None 表示尚可安全重新准备。
    pub authorized_at: Option<i64>,
    /// 提交发送授权时的控制版本；只允许该发送者报告确切未发送，暂停不伪造已发送事实。
    #[serde(default)]
    pub generation: u64,
    /// 已证明未发送的次数；只允许一次自动重新排队。
    pub not_sent: u32,
}

/// 当前回合的恢复记录；只保存摘要及计时，不保存私有推理或工具正文。
#[derive(Clone, Debug, Default, Deserialize, Serialize)]
pub(crate) struct Recovery {
    /// 提供方明确要求等待的 Unix 毫秒截止；隐藏错误提示不解除等待。
    #[serde(default)]
    pub provider_until: i64,
    /// 当前错误提示的等待秒数，零表示没有明确时长。
    #[serde(default)]
    pub provider_hint: u32,
    /// 上次观察仍有额度/速率限制，防止同一静态提示不断延长等待。
    #[serde(default)]
    pub provider_active: bool,
    /// 本回合恢复等待秒数；零按用户策略，空闲中断或确认刷新后按任务间隔，其他状态采用模型静默窗口。
    #[serde(default)]
    pub window_seconds: u32,
    /// 最近公开状态摘要，变化时重新开始静默计时。
    pub fingerprint: String,
    /// 最近公开进展的 Unix 毫秒时间。
    pub progress_at: i64,
    /// 本回合已领取的刷新/重开次数，最多三次。
    pub attempts: u32,
    /// 最近领取修复的 Unix 毫秒时间；浏览器修复动作遵守当前恢复窗口的冷却。
    pub repaired_at: i64,
    /// 当前待发正文是否用于未完成回合的恢复。
    pub pending: bool,
    /// 最近已观察的可信 finish 事件；None 不触发 finish 模式。
    pub finish_id: Option<String>,
    /// 本用户回合是否收到新的 finish；收到最终回复后消费。
    pub finish_seen: bool,
    /// 修复前最近已验证的主文档 ID；从未取得文档时为空，不跨用户回合继承。
    #[serde(default)]
    pub document_id: String,
    /// 已观察到修复后的新主文档；仅领取修复动作不能作为完成证明。
    #[serde(default)]
    pub confirmed: bool,
}

impl Recovery {
    /// 当前实际静默窗口，尚未观察模型时使用基础值，不把已知的短失败窗口拉长。
    /// # 参数
    /// - `policy`：当前用户策略，单位秒，仅用于尚未观察到实际窗口的记录。
    pub(crate) fn window(&self, policy: &Policy) -> u32 {
        if self.window_seconds == 0 {
            policy.silence_seconds
        } else {
            self.window_seconds
        }
    }
}

/// 无消息正文的任务投影；用于浏览器控件及调度查询。
#[derive(Debug, Serialize, Deserialize)]
pub struct TaskView {
    /// Runner 上次固定健康状态，不含原始异常、凭据或文件路径。
    #[serde(default)]
    pub local_status: webcodex_core::continuation::LocalStatus,
    /// 绑定 Runner 最近领取租约时间；不等同于浏览器健康或操作系统已成功阻止休眠。
    #[serde(default)]
    pub local_at: i64,
    /// 普通列表最近八条，Inspect 返回全部有界进展；不提供发送权或私有正文。
    #[serde(default)]
    pub journal: Vec<crate::journal::ProgressEvent>,
    /// 稳定任务 UUID。
    pub id: String,
    /// 控制版本。
    pub generation: u64,
    /// 当前唯一发送者绑定。
    pub binding: Binding,
    /// 用户目标和预算。
    pub options: Options,
    /// 后端当前状态。
    pub state: String,
    /// 安全暂停或完成原因。
    pub reason: String,
    /// 已领取次数。
    pub rounds: u32,
    /// 最早领取时间，Unix 毫秒。
    pub next_at: i64,
    /// 待发消息 ID；不授予领取权。
    pub delivery_id: Option<String>,
    /// 后端是否允许未完成回合发送；浏览器不能自行推断。
    pub recovery_send: bool,
    /// 最早允许请求刷新时间，Unix 毫秒；预算耗尽为 None，领取仍需重新核对。
    pub repair_at: Option<i64>,
    /// 最近有效观察时间，Unix 毫秒；用于区别 Server 连通和页面仍被观察。
    #[serde(default)]
    pub observed_at: i64,
    /// 本条待发消息是否已获发送授权；仅用于状态展示，不能替代 Authorize。
    #[serde(default)]
    pub send_authorized: bool,
    /// 当前账户的有界队列摘要，按实际投递顺序排列。
    #[serde(default)]
    pub queue: Vec<QueueView>,
    /// 已绑定原生 Window 的域隔离摘要；None 时不猜测账户内其他聊天。
    #[serde(default)]
    pub native_window: Option<String>,
    /// 原生 finish 是否仍持有当前回合。
    #[serde(default)]
    pub native_held: bool,
    /// 仅同进程适配器读取的 MCP 主体关联，不序列化到 HTTP。
    #[serde(skip)]
    pub native_principal: Option<(String, String)>,
    /// 同进程适配读取的其他已关联 Session，不向浏览器暴露内部工作关系。
    #[serde(skip)]
    pub native_sessions: Vec<String>,
    /// 已由本回合来源证明关联的主 Session；不改写用户手动 Session 偏好，人工新回合会撤销。
    #[serde(skip)]
    pub native_primary_session: Option<String>,
}

/// 一次领取成功才返回的纯文本消息；重试不会重新返回正文。
#[derive(Debug, Serialize, Deserialize)]
pub struct Dispatch {
    /// 回执 UUID。
    pub id: String,
    /// 包含确切回执标记的发送正文。
    pub text: String,
}

/// 后端操作结果；没有 dispatch 时扩展不得自行构造续跑指令。
#[derive(Debug, Default, Serialize, Deserialize)]
pub struct Response {
    /// 当前任务列表或单任务投影。
    pub tasks: Vec<TaskView>,
    /// 仅一次性领取成功时存在。
    pub dispatch: Option<Dispatch>,
    /// 仅本次成功领取返回的浏览器修复动作；重复领取受次数和冷却限制。
    pub repair: Option<String>,
}

impl Binding {
    /// 校验浏览器绑定及 ChatGPT URL；失败不改变任务。
    /// # 参数
    /// - `page`：本次完整公开页面观测，仅借用于校验。
    pub(crate) fn validate(&self, page: &Observation) -> Result<(), crate::Error> {
        for id in [&self.client, &self.browser, &self.conversation] {
            Uuid::parse_str(id).map_err(|_| crate::Error::Invalid)?;
        }
        if self.tab_id < 0
            || !["https://chatgpt.com", "https://chat.openai.com"].contains(&self.origin.as_str())
        {
            return Err(crate::Error::Invalid);
        }
        let expected = format!("{}{}/c/{}", self.origin, self.prefix, self.conversation);
        let url = url::Url::parse(&page.url).map_err(|_| crate::Error::Invalid)?;
        if (!self.prefix.is_empty()
            && (!self.prefix.starts_with("/g/")
                || self.prefix.len() <= 3
                || self.prefix.len() > MAX_ID
                || !self.prefix[3..]
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))))
            || url.origin().ascii_serialization() != self.origin
            || url.username() != ""
            || url.password().is_some()
            || url.path().trim_end_matches('/')
                != url::Url::parse(&expected)
                    .map_err(|_| crate::Error::Invalid)?
                    .path()
            || !page.supported
            || page.user_id.is_empty()
            || page.user_id.len() > MAX_ID
            || page.user_text.len() > MAX_MESSAGE
            || page.document_id.len() > MAX_ID
            || page.finish_id.as_ref().is_some_and(|id| id.len() > MAX_ID)
            || page.activity.source_turn.len() > MAX_ID
            || page.source_proof.as_ref().is_some_and(|proof| {
                proof.user_id != page.user_id
                    || proof.message_id.is_empty()
                    || proof.message_id.len() > MAX_ID
                    || Uuid::parse_str(&proof.task_id).is_err()
                    || proof.challenge.len() != 64
                    || !proof.challenge.bytes().all(|byte| byte.is_ascii_hexdigit())
            })
            || page.activity.model.len() > 100
            || page.activity.retry_after > 86_400
        {
            return Err(crate::Error::Invalid);
        }
        if let Some(reply) = &page.reply {
            if reply.id.is_empty()
                || reply.id.len() > MAX_ID
                || reply.user_id.len() > MAX_ID
                || reply.text.len() > MAX_MESSAGE
            {
                return Err(crate::Error::Invalid);
            }
        }
        Ok(())
    }
}

impl Options {
    /// 校验用户目标和发送预算；失败不截断或替换输入。
    pub(crate) fn validate(&self) -> Result<(), crate::Error> {
        if self.objective.len() > MAX_OBJECTIVE
            || self.policy.runner_id.as_ref().is_some_and(|id| {
                id.is_empty()
                    || id.len() > 128
                    || !id
                        .bytes()
                        .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'-' | b'.'))
            })
            || self.policy.keep_awake && self.policy.runner_id.is_none()
            || !(1..=MAX_ROUNDS).contains(&self.limit)
            || !(30..=3600).contains(&self.delay)
            || !(120..=3600).contains(&self.policy.silence_seconds)
            || !(1..=720).contains(&self.policy.lifetime_hours)
            || !["after_turn", "finish"].contains(&self.policy.trigger.as_str())
            || self.policy.session_id.as_ref().is_some_and(|id| {
                id.is_empty()
                    || id.len() > 128
                    || !id
                        .bytes()
                        .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
            })
        {
            return Err(crate::Error::Invalid);
        }
        Ok(())
    }
}

impl Task {
    /// 生成不含公开上下文及待发正文的客户端投影，不修改任务。
    pub(crate) fn view(&self) -> TaskView {
        TaskView {
            local_at: self.local_at,
            local_status: self.local_status,
            journal: self
                .journal
                .iter()
                .skip(
                    self.journal
                        .len()
                        .saturating_sub(crate::journal::VIEW_EVENTS),
                )
                .cloned()
                .collect(),
            id: self.id.clone(),
            generation: self.generation,
            binding: self.binding.clone(),
            options: self.options.clone(),
            state: self.state.clone(),
            reason: self.reason.clone(),
            rounds: self.rounds,
            next_at: self.next_at,
            delivery_id: self.pending.as_ref().map(|pending| pending.id.clone()),
            recovery_send: self.recovery.pending,
            repair_at: (self.options.policy.recovery
                && self.recovery.attempts < crate::recovery::MAX_REPAIRS)
                .then_some(
                    self.recovery.progress_at.max(self.recovery.repaired_at)
                        + i64::from(self.recovery.window(&self.options.policy)) * 1000,
                )
                .map(|deadline| deadline.max(self.recovery.provider_until)),
            observed_at: self.observed_at,
            send_authorized: self.delivery.authorized_at.is_some(),
            queue: self
                .queue
                .iter()
                .map(|entry| QueueView {
                    id: entry.id.clone(),
                    kind: entry.kind.clone(),
                    preview: entry.text.chars().take(80).collect(),
                    trigger: entry.trigger.clone(),
                    delivery: entry.delivery.clone(),
                })
                .collect(),
            native_window: self.native.window.clone(),
            native_held: self.native.held,
            native_principal: self.native.principal.clone(),
            native_sessions: self.native.related_session_ids.clone(),
            native_primary_session: self.native.session_id.clone(),
        }
    }

    /// 暂停并撤销异步版本；保留未知投递，供用户检查。
    /// # 参数
    /// - `reason`：固定安全状态码，不使用外部异常正文。
    /// - `now`：Unix 毫秒时间。
    pub(crate) fn pause(&mut self, reason: &str, now: i64) {
        self.state = "paused".into();
        self.reason = reason.into();
        self.changed_at = now;
        self.generation += 1;
        self.native.held = false;
    }
}

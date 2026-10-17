//! 当前 ChatGPT 的 MCP 续跑协议；与浏览器共用预算和队列，不调用决策 API。
use crate::model::{Mode, Task, MAX_MESSAGE};
use crate::{decision, queue, Error};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use uuid::Uuid;

/// 每任务最多一万次控制检查，等待后台任务也不能无限轮询。
const MAX_CONTROL_CALLS: u32 = 10_000;
/// 控制检查名称最多一百二十八字节，仅接受 ASCII 字母、数字、下划线和连字符。
const MAX_OPERATION_ID: usize = 128;
/// Loop 连续三次要求完成则停止本次循环，保留人工阻塞边界。
const MAX_LOOP_STOP_ATTEMPTS: u32 = 3;
/// 同一投递最多四个待确认 Window；仅主聊天公开回执选中的一个取得控制，其他候选失效。
const MAX_SOURCE_CANDIDATES: usize = 4;
/// 来源挑战有效期两分钟，单位毫秒；重复 bind 不延长，任务授权到期优先。
const SOURCE_CONFIRM_MS: i64 = 120_000;

/// 尚未获得浏览器主聊天回执的 MCP 候选；所有字段仅存私有账本，不授予执行权。
#[derive(Clone, Debug, Deserialize, Serialize)]
pub(crate) struct SourceCandidate {
    /// 候选调用已验证的其他 Session；来源确认后才纳入任务的实际等待范围。
    #[serde(default)]
    related_session_ids: Vec<String>,
    /// 服务端随机六十四位挑战；只能从确切主聊天公开回执确认。
    challenge: String,
    /// 候选可信 Window，来自 MCP Host，不接受模型参数替换。
    window: String,
    /// 已重新授权的 Session，确认后才关联实际任务。
    session_id: String,
    /// 认证 MCP 主体，不序列化到普通任务列表。
    principal: (String, String),
    /// 挑战绝对失效时间，Unix 毫秒。
    expires_at: i64,
}

/// 当前聊天的原生请求；账户、Window 和 Session 权限必须由可信调用方重新验证。
#[derive(Clone)]
pub struct NativeRequest {
    /// 本次显式声明的其他工作/子任务 Session，最多十五个；同一逻辑重试须保持集合相同。
    pub related_session_ids: Vec<String>,
    /// 当前指令发行的任务 UUID。
    pub task_id: String,
    /// 当前控制版本，暂停或人工改动会撤销旧版本。
    pub generation: u64,
    /// 只供当前聊天使用的随机令牌，不写入审计正文。
    pub control_token: String,
    /// bind 或 finish。
    pub action: String,
    /// 单个逻辑检查的稳定名称，重试同一检查必须保持不变。
    pub request_id: String,
    /// finish 使用 continue、complete 或 blocked；bind 使用空串。
    pub decision: String,
    /// 当前 ChatGPT 的公开进度、规划或完成依据，最多 MAX_MESSAGE 字节。
    pub summary: String,
}

/// 可信运行时事实；不能由模型或网页反序列化提供，不包含工具参数或私有结果。
#[derive(Clone)]
pub struct NativeFacts {
    /// 可信运行时无法确认本回合工具/依赖状态；已确认主聊天必须暂停，不能把未知当成空闲。
    pub runtime_unavailable: bool,
    /// 重新验证权限后的其他关联 Session，含此前已关联项，最多十五个，不含主 Session。
    pub related_session_ids: Vec<String>,
    /// 经认证 MCP 主体的类型与稳定 ID，不能来自网页或模型。
    pub principal: (String, String),
    /// 经认证 MCP Window 的域隔离摘要，避免其他聊天和 worker 接管当前回合。
    pub window: String,
    /// 重新验证账户及项目权限后的确切 Workflow Session。
    pub session_id: String,
    /// 当前 Window/Session 有其他工具、Job 或子任务执行。
    pub busy: bool,
    /// 当前等待人工批准、回答或指导，不能自动处理。
    pub approval: bool,
}

/// 原生后续工作；同一逻辑请求重读保持相同 sequence，不能据重复结果再次执行未知操作。
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct NativeResponse {
    /// 所属任务 ID，不授予其他任务权限。
    pub task_id: String,
    /// 当前控制版本。
    pub generation: u64,
    /// 已交出的工作序号，重复请求不增加，跨重启保持。
    pub sequence: u32,
    /// 自动关联的确切 Session，后续工具复用该 ID。
    pub session_id: String,
    /// 本回合已释放，可以向用户报告结果或阻塞。
    pub released: bool,
    /// 等待真实工具或人工操作，不得忙轮询或代替用户批准。
    pub waiting: bool,
    /// 下一次原生检查的令牌；释放后为 None，不传给其他聊天或子代理。
    pub control_token: Option<String>,
    /// 当前原始请求的后续工作或停止/等待指令。
    pub instructions: String,
}

/// 私有账本中的原生回合状态；令牌不进入浏览器任务列表或日志。
#[derive(Clone, Debug, Default, Deserialize, Serialize)]
#[serde(default)]
pub(crate) struct NativeState {
    /// 已由确认的主聊天关联的其他 Session，最多十五个；持久化后继续参与工具和人工等待。
    #[serde(default)]
    pub related_session_ids: Vec<String>,
    /// 至多四个待主聊天回执的 Window；只有确切公开回执才能选定主聊天。
    #[serde(default)]
    candidates: Vec<SourceCandidate>,
    /// MCP 主体关联，只存私有账本和同进程投影，不进入 HTTP 任务列表。
    pub principal: Option<(String, String)>,
    /// 当前令牌；未准备消息时为空。
    pub token: String,
    /// 发行令牌的浏览器投递 ID。
    pub delivery_id: Option<String>,
    /// 已从确切浏览器回执确认令牌消息；原生调用可以早于该回执。
    pub delivered: bool,
    /// 主聊天公开来源回执确认的可信 MCP Window 摘要。
    pub window: Option<String>,
    /// 已重新授权的 Session，不根据最近活动推断。
    pub session_id: Option<String>,
    /// 当前回合由原生 finish 继续持有，浏览器不会另行创造同一队列工作。
    pub held: bool,
    /// complete 或 blocked；None 表示尚未释放。
    pub released: Option<String>,
    /// 累计原生后续工作序号。
    pub sequence: u32,
    /// 累计控制检查数，等待也受容量限制。
    pub calls: u32,
    /// Loop 连续拒绝继续的次数。
    pub stop_attempts: u32,
    /// 最近消费令牌的 SHA-256，只用于安全幂等重读。
    pub replay_token: String,
    /// 最近逻辑请求及正文的摘要，不能更改相同操作的决定。
    pub replay_input: String,
    /// 最近已提交的返回内容；重读不再次出队或计费。
    pub replay: Option<NativeResponse>,
    /// 已通过工具结果交出的队列 ID，只有下一有效令牌调用才确认接收。
    pub queued_receipt: Option<String>,
}

/// 主聊天的确切公开进度确认来源；worker 的工具调用或其他会话文本都不能完成此绑定。
/// # 参数
/// - `task`：本次页面已通过账户、版本和 URL 校验，并已处理确切自动投递回执的任务。
/// - `proof`：同一用户回合读到的公开来源回执；None 或不匹配时不改变权限。
/// - `now`：服务端 Unix 毫秒时间，不延长挑战或任务有效期。
pub(crate) fn confirm_source(task: &mut Task, proof: Option<&crate::model::SourceProof>, now: i64) {
    let Some(proof) = proof else {
        return;
    };
    if task.native.window.is_some()
        || !task.native.delivered
        || matches!(task.state.as_str(), "paused" | "done" | "watching")
        || proof.task_id != task.id
        || proof.generation != task.generation
        || proof.user_id != task.expected_user
        || proof.message_id.is_empty()
        || proof.message_id.len() > 200
        || now >= task.expires_at
    {
        return;
    }
    let Some(candidate) = task
        .native
        .candidates
        .iter()
        .find(|candidate| candidate.challenge == proof.challenge && now < candidate.expires_at)
        .cloned()
    else {
        return;
    };
    task.native.window = Some(candidate.window);
    task.native.principal = Some(candidate.principal);
    task.native.session_id = Some(candidate.session_id.clone());
    task.native.related_session_ids = candidate.related_session_ids;
    task.native.held = true;
    task.protocol_armed = true;
    task.native.candidates.clear();
    if task.reason == "source_confirmation_required" {
        task.reason.clear();
    }
    task.recovery.progress_at = now;
}

/// 生成两份独立 UUID 组成的随机令牌，不编码账户、正文或本机身份。
fn token() -> String {
    format!("{}{}", Uuid::new_v4().simple(), Uuid::new_v4().simple())
}

/// 对受限输入生成摘要，只借用本次 value，不保存或记录原文。
/// # 参数
/// - `value`：令牌或逻辑请求的完整受限文本。
fn digest(value: &str) -> String {
    format!("{:x}", Sha256::digest(value.as_bytes()))
}

/// 撤销旧原生来源；未知工具结果保留待核对，不因新人工消息或配置更新而自动重发。
/// # 参数
/// - `task`：当前事务任务，保留累计工作序号和检查预算，不修改消息正文。
pub(crate) fn revoke(task: &mut Task) {
    // 旧浏览器消息已经授权但尚无确切回执时，其队列项不得在新人工回合中重发。
    let unknown_browser = task
        .pending
        .as_ref()
        .filter(|_| task.delivery.authorized_at.is_some())
        .and_then(|pending| pending.queue_id.as_ref());
    for entry in &mut task.queue {
        if entry.delivery == "native" || unknown_browser == Some(&entry.id) {
            entry.delivery = "uncertain".into();
        }
    }
    task.planning = false;
    task.planning_trigger = None;
    task.native = NativeState {
        sequence: task.native.sequence,
        calls: task.native.calls,
        ..NativeState::default()
    };
}

/// 首次准备当前消息时发行原生令牌；重复准备不会改变令牌或重复附加指令。
/// # 参数
/// - `task`：当前事务任务，必须已有 pending；累计工作序号和检查预算保持不变。
pub(crate) fn seal(task: &mut Task) {
    let Some(pending) = task.pending.as_mut() else {
        return;
    };
    if task.native.delivery_id.as_ref() == Some(&pending.id) {
        return;
    }
    // 旧工具结果未获下一令牌确认时保留为未知，不能换一个通道重发同一工作。
    for entry in &mut task.queue {
        if entry.delivery == "native" {
            entry.delivery = "uncertain".into();
        }
    }
    task.native = NativeState {
        token: token(),
        delivery_id: Some(pending.id.clone()),
        sequence: task.native.sequence,
        calls: task.native.calls,
        ..NativeState::default()
    };
    let preamble = format!("\n\n本轮原生续跑：task_id={}，generation={}，control_token={}。令牌仅供当前主聊天使用，不传给子代理或其他聊天。有 WebCodex 工具时先调用 update_task_continuation(action=bind,request_id=bind,以上身份,session_id=本任务已有Session；没有时先正常调用start_session创建，不需要用户手填)。每次阶段收尾先做原请求要求的验证，再调用 update_task_continuation(action=finish,request_id=本次唯一检查名,decision=continue/complete/blocked,summary=实际进度及完成依据)。按返回在本聊天内继续，released=true 才结束。重复 sequence 只核对已提交工作，不重复执行结果未知的操作；工具不可用时沿用公开回复标记协议，不调用模型 API。", task.id, task.generation, task.native.token);
    pending.wire = Some(format!("[[WC_SEND:{}]]\n{preamble}\n首次 bind 会返回来源回执：必须在本主聊天按返回输出单行公开进度，再次 bind 等待扩展确认，不能用子代理的消息替代。本任务若跨项目或有子任务 Session，用 related_session_ids 声明其他全部依赖（最多十五个）；服务端会持续等待这些依赖。先完成原生绑定；返回 waiting 时仅等待已提交工作或人工处理，不执行新阶段。然后在已关联 Session 中处理以下原任务：\n{}", pending.id, pending.text));
}

/// 从已校验任务生成结果；暂停、完成或明确释放后不再返回控制令牌。
/// # 参数
/// - `task`：当前任务副本。
/// - `waiting`：是否等待真实工具或人工处理。
/// - `instructions`：本次明确授权的后续指令或停止说明。
fn response(task: &Task, waiting: bool, instructions: String) -> NativeResponse {
    let released = task.native.released.is_some()
        || matches!(task.state.as_str(), "paused" | "done" | "watching");
    NativeResponse {
        task_id: task.id.clone(),
        generation: task.generation,
        sequence: task.native.sequence,
        session_id: task.native.session_id.clone().unwrap_or_default(),
        released,
        waiting,
        control_token: (!released).then(|| task.native.token.clone()),
        instructions,
    }
}

/// 有效下一令牌证明前一工具结果已接收；人工原文单独保存，自动提示不升级为人工约束。
/// # 参数
/// - `task`：尚未覆盖 queued_receipt 的当前事务任务。
/// # Errors
/// 人工约束超过上下文容量或冲突时返回错误，外层事务回滚队列与接收状态。
fn acknowledge(task: &mut Task) -> Result<(), Error> {
    let Some(id) = task.native.queued_receipt.as_ref() else {
        return Ok(());
    };
    let Some(index) = task.queue.iter().position(|entry| &entry.id == id) else {
        task.native.queued_receipt = None;
        return Ok(());
    };
    let entry = task.queue[index].clone();
    if entry.kind == "message" {
        crate::store::record(task, &format!("queue:{}", entry.id), "user", &entry.text)?;
    }
    task.queue.remove(index);
    task.planning = entry.kind == "plan";
    task.planning_trigger = task.planning.then_some(entry.trigger);
    task.native.queued_receipt = None;
    Ok(())
}

/// 将已释放结果应用到浏览器状态；没有原 pending 回执时由调用方延后至确切回执。
/// # 参数
/// - `task`：原生释放已提交的任务；不重置预算、版本或授权时限。
pub(crate) fn release_browser_state(task: &mut Task) {
    match task.native.released.as_deref() {
        Some("complete") => {
            task.state = if task.options.policy.continuous {
                "watching"
            } else {
                "done"
            }
            .into()
        }
        Some("blocked") => task.state = "paused".into(),
        _ => {}
    }
}

/// 校验令牌、检查名及可信来源，读取重复检查时只返回同一已提交结果。
/// # 参数
/// - `task`：按认证账户从账本读取的任务。
/// - `request`：受限模型请求。
/// - `facts`：可信运行时重新确认的 Window 和 Session。
/// - `now`：Unix 毫秒时间，不延长授权。
/// # 返回
/// 合法新请求返回其摘要和 None；同一旧令牌检查返回缓存；不合法请求返回固定错误。
fn validate(
    task: &Task,
    request: &NativeRequest,
    facts: &NativeFacts,
    now: i64,
) -> Result<(String, String, Option<NativeResponse>), Error> {
    if !["bind", "finish"].contains(&request.action.as_str())
        || request.request_id.is_empty()
        || request.request_id.len() > MAX_OPERATION_ID
        || !request
            .request_id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-'))
        || request.summary.len() > MAX_MESSAGE
        || facts.window.len() != 64
        || facts.session_id.is_empty()
        || request.control_token.len() != 64
        || !request
            .control_token
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit())
    {
        return Err(Error::Invalid);
    }
    if task.generation != request.generation
        || now >= task.expires_at
        || task
            .options
            .policy
            .session_id
            .as_ref()
            .is_some_and(|session| session != &facts.session_id)
        || task
            .native
            .window
            .as_ref()
            .is_some_and(|window| window != &facts.window)
        || task
            .native
            .session_id
            .as_ref()
            .is_some_and(|session| session != &facts.session_id)
        || task
            .native
            .principal
            .as_ref()
            .is_some_and(|principal| principal != &facts.principal)
    {
        return Err(Error::Conflict);
    }
    /// 每个主聊天最多十五个其他 Session，主 Session 单独占一项，限制权限检查与序列化开销。
    const MAX_RELATED_SESSIONS: usize = 15;
    if request.related_session_ids.len() > MAX_RELATED_SESSIONS
        || facts.related_session_ids.len() > MAX_RELATED_SESSIONS
        || request
            .related_session_ids
            .iter()
            .chain(&facts.related_session_ids)
            .any(|id| {
                id.is_empty()
                    || id.len() > 128
                    || !id
                        .bytes()
                        .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'-'))
            })
    {
        return Err(Error::Invalid);
    }
    let mut related = request.related_session_ids.clone();
    related.sort();
    related.dedup();
    let related = related
        .iter()
        .map(|id| format!("{}:{id}", id.len()))
        .collect::<Vec<_>>()
        .join("|");
    let input = digest(&format!(
        "{}\0{}\0{}\0{}\0{}",
        request.action, request.request_id, request.decision, request.summary, related
    ));
    let presented = digest(&request.control_token);
    if presented == task.native.replay_token {
        if input != task.native.replay_input {
            return Err(Error::Conflict);
        }
        return Ok((
            input,
            presented,
            Some(task.native.replay.clone().ok_or(Error::Conflict)?),
        ));
    }
    if presented != digest(&task.native.token)
        || task.native.token.is_empty()
        || !(task.native.delivered || task.delivery.authorized_at.is_some())
        || matches!(task.state.as_str(), "paused" | "done" | "watching")
        || task.native.released.is_some()
    {
        return Err(Error::Conflict);
    }
    Ok((input, presented, None))
}

/// 应用原生 bind/finish，后续指令与已消费令牌必须在同一事务中保存。
/// # 参数
/// - `task`：按认证账户读取的事务任务。
/// - `request`：当前聊天自己提交的公开决定和令牌。
/// - `facts`：可信运行时的来源、Session 及真实等待事实。
/// - `now`：Unix 毫秒时间，不扩展原预算或授权。
/// # Errors
/// 身份或版本不符返回 Conflict，非法决定返回 Invalid，上下文或队列超限时整笔事务回滚。
pub(crate) fn apply(
    task: &mut Task,
    request: &NativeRequest,
    facts: &NativeFacts,
    now: i64,
) -> Result<NativeResponse, Error> {
    let (input, presented, replay) = validate(task, request, facts, now)?;
    if facts.runtime_unavailable {
        if task.native.window.is_none() {
            return Err(Error::Conflict);
        }
        task.pause("runtime_unavailable", now);
        return Ok(response(task, false, "后台调用或依赖状态无法确认，已暂停。先核对已提交及结果未知的操作，不重复执行，再向用户说明。".into()));
    }
    if let Some(result) = replay {
        return Ok(result);
    }
    if task.native.calls >= MAX_CONTROL_CALLS {
        task.pause("round_limit", now);
        return Ok(response(
            task,
            false,
            "控制检查预算已用完，停止自动工作并向用户说明。".into(),
        ));
    }
    task.native.calls += 1;
    if task.native.window.is_none() {
        if request.action != "bind" || !request.decision.is_empty() {
            return Err(Error::Conflict);
        }
        task.native
            .candidates
            .retain(|candidate| now < candidate.expires_at);
        let existing = task.native.candidates.iter_mut().find(|candidate| {
            candidate.window == facts.window
                && candidate.session_id == facts.session_id
                && candidate.principal == facts.principal
        });
        let challenge = if let Some(candidate) = existing {
            candidate.related_session_ids = facts.related_session_ids.clone();
            candidate.challenge.clone()
        } else {
            if task.native.candidates.len() >= MAX_SOURCE_CANDIDATES {
                return Err(Error::Capacity);
            }
            let challenge = token();
            task.native.candidates.push(SourceCandidate {
                related_session_ids: facts.related_session_ids.clone(),
                challenge: challenge.clone(),
                window: facts.window.clone(),
                session_id: facts.session_id.clone(),
                principal: facts.principal.clone(),
                expires_at: task.expires_at.min(now + SOURCE_CONFIRM_MS),
            });
            challenge
        };
        task.reason = "source_confirmation_required".into();
        let marker = format!(
            "[[WEBCODEX_SOURCE:{}:{}:{challenge}]]",
            task.id, task.generation
        );
        let mut result = response(task, true, format!("尚未取得当前主聊天来源证明。先在本聊天输出一条用户可见的进度消息，整条内容必须恰为以下一行，不能放入代码块、引用、工具参数或子代理消息：\n{marker}\n等待浏览器扩展确认后，用相同 control_token 再调用 bind；waiting=true 时只等待，不执行新阶段，不忙轮询。该回执不要求用户输入，不打开新聊天。"));
        result.session_id = facts.session_id.clone();
        return Ok(result);
    }
    task.native.related_session_ids = facts.related_session_ids.clone();
    task.native.held = true;
    task.protocol_armed = true;
    acknowledge(task)?;
    if request.action == "bind" {
        if !request.decision.is_empty() {
            return Err(Error::Invalid);
        }
        if facts.approval || facts.busy {
            return Ok(response(task, true, "已关联当前聊天；原任务仍有后台工作或人工等待。先取得确切结果，不执行新的阶段，不代替用户批准。".into()));
        }
        return Ok(response(task, false, "已关联当前主聊天与原任务。继续执行，每阶段先做必要验证，再用 finish 报告实际进度；控制令牌不传给子代理。".into()));
    }
    if !["continue", "complete", "blocked"].contains(&request.decision.as_str())
        || request.summary.trim().is_empty()
    {
        return Err(Error::Invalid);
    }
    if facts.approval {
        return Ok(response(
            task,
            true,
            "等待用户批准或回答；不得代替用户操作，也不把审批等待当作故障刷新。用户处理后再检查。"
                .into(),
        ));
    }
    if facts.busy {
        return Ok(response(task, true, "当前工具、后台 Job 或子任务尚未结束。等待确切结果并核对已提交操作，不重复启动，不忙轮询本工具。".into()));
    }
    if now < task.recovery.provider_until {
        return Ok(response(
            task,
            true,
            "提供方等待期限尚未结束；保留当前任务和队列，不新建工作或忙轮询，期限结束后再检查。"
                .into(),
        ));
    }
    if task.planning {
        queue::accept_plan(task, &request.summary)?;
    }
    if decision::repeating(task, &request.summary) {
        task.pause("no_progress", now);
        return Ok(response(
            task,
            false,
            "连续三轮公开进度相同，已暂停；核对原任务和已提交结果，不制造新工作。".into(),
        ));
    }
    if request.decision == "blocked" {
        task.native.released = Some("blocked".into());
        task.reason = "chat_blocked".into();
    } else if task
        .queue
        .iter()
        .find(|entry| entry.delivery == "queued")
        .is_some_and(|entry| entry.trigger == "after_turn")
    {
        task.native.released = Some("yield".into());
        task.reason = "after_turn_queue".into();
    } else if request.decision == "complete"
        && task.options.mode != Mode::Loop
        && task.queue.is_empty()
    {
        task.native.released = Some("complete".into());
        task.reason = "goal_complete".into();
    } else {
        if task.rounds >= task.options.limit {
            task.pause("round_limit", now);
            return Ok(response(
                task,
                false,
                "已到授权轮数上限，停止自动工作。".into(),
            ));
        }
        if request.decision == "complete" && task.options.mode == Mode::Loop {
            task.native.stop_attempts += 1;
            if task.native.stop_attempts >= MAX_LOOP_STOP_ATTEMPTS {
                task.native.released = Some("blocked".into());
                task.reason = "loop_stop_refused".into();
            }
        } else {
            task.native.stop_attempts = 0;
        }
    }
    if task.native.released.is_some() {
        task.native.held = false;
        // 原生完成不等待 UI 回执；pending 仅为晚到的确切回执保留，禁止再投递。
        release_browser_state(task);
        let result = response(task, false, format!("本轮已释放。按原请求报告实际结果或阻塞，保留未完成事项，不声称未验证内容已完成。\n{}", request.summary));
        task.native.replay_token = presented;
        task.native.replay_input = input;
        task.native.replay = Some(result.clone());
        return Ok(result);
    }
    if task.queue.iter().any(|entry| entry.delivery == "uncertain") {
        return Ok(response(task, true, "前一队列工具结果是否收到尚不确定。先核对当前聊天与已提交操作，不重发该项，必要时请用户检查队列。".into()));
    }
    let next = if let Some(index) = task
        .queue
        .iter()
        .position(|entry| entry.delivery == "queued")
    {
        let entry = &mut task.queue[index];
        entry.delivery = "native".into();
        task.native.queued_receipt = Some(entry.id.clone());
        if entry.kind == "plan" {
            queue::plan_instruction(task)
        } else {
            entry.text.clone()
        }
    } else {
        decision::instruction(task)
    };
    task.rounds += 1;
    task.native.sequence += 1;
    task.native.token = token();
    task.recovery.progress_at = now;
    let instructions = format!("继续同一原始任务，工作序号 {}。需要人工时报告 blocked，不能为 Loop 编造任务。\n{next}\n控制令牌已更换，下次 finish 使用本返回的 control_token，不发送新的用户消息。", task.native.sequence);
    let result = response(task, false, instructions);
    task.native.replay_token = presented;
    task.native.replay_input = input;
    task.native.replay = Some(result.clone());
    Ok(result)
}

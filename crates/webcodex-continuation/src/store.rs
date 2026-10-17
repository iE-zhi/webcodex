//! SQLite 任务账本：事务内校验当前会话标记并预留消息；账户、任务版本和确切回执隔离操作。
use crate::decision::{self, Decision, PROTOCOL_VERSION};
use crate::model::*;
use crate::{
    delivery, journal, native, queue, recovery, Error, NativeFacts, NativeRequest, NativeResponse,
};
use rusqlite::{params, Connection, OptionalExtension, Transaction, TransactionBehavior};
use sha2::{Digest, Sha256};
use std::path::Path;
use std::time::Duration;
use uuid::Uuid;

/// 所有账户合计最多 512 个任务，限制上下文、启动恢复和磁盘增长。
const MAX_TOTAL_TASKS: i64 = 512;
/// SQLite 写锁最多等待两秒；失败由调用方重试原幂等操作。
const LOCK_TIMEOUT: Duration = Duration::from_secs(2);
/// 专用账本当前模式版本；未知版本拒绝打开，不尝试降级。
const SCHEMA_VERSION: i64 = 1;

/// 对完整用户正文生成摘要，只识别编辑，不替代提供方消息身份。
/// # 参数
/// - `text`：已校验长度的公开用户正文，仅读取本次借用。
pub(crate) fn user_hash(text: &str) -> String {
    format!("{:x}", Sha256::digest(text.as_bytes()))
}

/// 单个受 Mutex 保护的数据库连接；仅在阻塞工作线程内访问，不跨 await 持锁。
pub(crate) struct Store {
    /// WAL/FULL 模式连接，销毁时关闭；没有活动事务可以跨出本模块。
    connection: Connection,
}

/// 在事务内读取确切账户的任务；不存在或其他账户的 ID 均返回 NotFound。
/// # 参数
/// - `tx`：当前写事务。
/// - `owner`：已认证账户主键。
/// - `id`：任务 UUID。
fn load(tx: &Transaction<'_>, owner: &str, id: &str) -> Result<Task, Error> {
    let body: Option<String> = tx
        .query_row(
            "SELECT body FROM continuation_tasks WHERE owner=?1 AND id=?2",
            params![owner, id],
            |row| row.get(0),
        )
        .optional()?;
    serde_json::from_str(&body.ok_or(Error::NotFound)?).map_err(|_| Error::Storage)
}

/// 发布任务和索引字段；外层事务提交前不会暴露局部状态。
/// # 参数
/// - `tx`：当前写事务。
/// - `owner`：已认证账户主键。
/// - `task`：完整受限任务状态，只借用本次写入。
fn save(tx: &Transaction<'_>, owner: &str, task: &Task) -> Result<(), Error> {
    let deadline = if task.state == "prepared" {
        task.expires_at.min(task.delivery.prepared_at + PREPARED_MS)
    } else if matches!(task.state.as_str(), "deciding" | "dispatching") {
        task.expires_at.min(task.changed_at + INFLIGHT_MS)
    } else {
        task.expires_at
    };
    let body = serde_json::to_string(task).map_err(|_| Error::Storage)?;
    tx.execute("INSERT INTO continuation_tasks(owner,id,request_id,client,state,deadline,body) VALUES(?1,?2,?3,?4,?5,?6,?7)
        ON CONFLICT(owner,id) DO UPDATE SET request_id=excluded.request_id,client=excluded.client,state=excluded.state,deadline=excluded.deadline,body=excluded.body",
        params![owner, task.id, task.request_id, task.binding.client, task.state, deadline, body])?;
    Ok(())
}

/// 同一消息只记录一次；人工编辑可更新用户正文，assistant 终结正文的冲突则拒绝。
/// # 参数
/// - `task`：当前事务持有的任务副本。
/// - `id`：提供方消息 ID。
/// - `role`：仅由服务端传 user 或 assistant。
/// - `content`：完整公开正文，不含工具记录。
pub(crate) fn record(task: &mut Task, id: &str, role: &str, content: &str) -> Result<(), Error> {
    // 判定只需要当前公开回复和确切回执；保留全部人工约束，不累计历史自动消息。
    task.messages.retain(|message| {
        message.id == id || (message.role != "assistant" && message.source != "automatic")
    });
    let used: usize = task
        .messages
        .iter()
        .map(|message| message.content.len())
        .sum();
    if let Some(existing) = task.messages.iter_mut().find(|message| message.id == id) {
        if existing.role != role || existing.content != content && role != "user" {
            return Err(Error::Conflict);
        }
        if used - existing.content.len() + content.len() > MAX_CONTEXT {
            return Err(Error::ContextLimit);
        }
        existing.content = content.into();
        return Ok(());
    }
    if task.messages.len() >= (MAX_ROUNDS as usize + 1) * 2 || used + content.len() > MAX_CONTEXT {
        return Err(Error::ContextLimit);
    }
    task.messages.push(Message {
        id: id.into(),
        role: role.into(),
        source: if role == "user" { "human" } else { "assistant" }.into(),
        content: content.into(),
    });
    Ok(())
}

/// 校验控制版本及浏览器绑定，拒绝晚到的旧操作。
/// # 参数
/// - `task`：当前数据库记录。
/// - `generation`：调用方读到的控制版本。
/// - `binding`：原操作绑定，必须逐字段一致。
fn fence(task: &Task, generation: u64, binding: &Binding) -> Result<(), Error> {
    if task.generation != generation || &task.binding != binding {
        return Err(Error::Conflict);
    }
    Ok(())
}

impl Store {
    /// 扫描单个账户最多三十二个任务，先持久化唤醒预算再交出固定会话 URL。
    /// # 参数
    /// - `owner`：认证账户 ID，由外层校验，不能取自请求正文。
    /// - `request`：已验证活动 Runner 身份的请求，browser_absent 仅为其本机进程证明。
    /// - `now`：服务端 Unix 毫秒时间；陈旧观察超过九十秒才可请求打开浏览器。
    /// # Errors
    /// 非法身份或数据库失败回滚整次发行；不能安全领取的任务只跳过该次唤醒。
    pub(crate) fn local_lease(
        &mut self,
        owner: &str,
        request: webcodex_core::continuation::LocalLeaseRequest,
        now: i64,
    ) -> Result<webcodex_core::continuation::LocalLease, Error> {
        use webcodex_core::continuation::{
            BrowserWake, LocalLease, LOCAL_LEASE_MS, MAX_LOCAL_WAKES,
        };
        /// 页面九十秒无有效观测才进入本机恢复，不把短暂后台调度当成浏览器退出。
        const OBSERVER_GRACE_MS: i64 = 90_000;
        Uuid::parse_str(&request.request_id).map_err(|_| Error::Invalid)?;
        Uuid::parse_str(&request.agent_instance_id).map_err(|_| Error::Invalid)?;
        if request.client_id.is_empty()
            || request.client_id.len() > 128
            || !request
                .client_id
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'-' | b'.'))
        {
            return Err(Error::Invalid);
        }
        let tx = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let mut statement = tx.prepare("SELECT body FROM continuation_tasks WHERE owner=?1 AND state NOT IN ('paused','done','watching') LIMIT ?2")?;
        let rows = statement.query_map(params![owner, MAX_TASKS as i64], |row| {
            row.get::<_, String>(0)
        })?;
        let mut result = LocalLease {
            request_id: request.request_id,
            valid_for_ms: LOCAL_LEASE_MS,
            keep_awake: false,
            wake: Vec::new(),
        };
        for body in rows {
            let mut task: Task = serde_json::from_str(&body?).map_err(|_| Error::Storage)?;
            if task.options.policy.runner_id.as_deref() != Some(&request.client_id)
                || now >= task.expires_at
            {
                continue;
            }
            task.local_at = now;
            task.local_status = request.status;
            if request.browser_absent
                && result.wake.len() < MAX_LOCAL_WAKES
                && now.saturating_sub(task.observed_at) >= OBSERVER_GRACE_MS
            {
                if task.options.policy.recovery && task.recovery.attempts >= recovery::MAX_REPAIRS {
                    task.pause("recovery_exhausted", now);
                } else {
                    match recovery::claim(&mut task, "open", None, now) {
                        Ok(()) => {
                            result.wake.push(BrowserWake {
                                task_id: task.id.clone(),
                                generation: task.generation,
                                url: format!(
                                    "{}{}/c/{}",
                                    task.binding.origin,
                                    task.binding.prefix,
                                    task.binding.conversation
                                ),
                            });
                            journal::append(
                                &mut task,
                                journal::ProgressEvent {
                                    kind: "runner_wake".into(),
                                    ..journal::ProgressEvent::default()
                                },
                                now,
                            );
                        }
                        Err(Error::Conflict) => {} // 冷却、提供方等待或预算已用完：不发行新动作。
                        Err(error) => return Err(error),
                    }
                }
            }
            result.keep_awake |= task.options.policy.keep_awake && task.state != "paused";
            save(&tx, owner, &task)?;
        }
        drop(statement);
        tx.commit()?;
        Ok(result)
    }
    /// 原生检查与浏览器控制共用写事务，令牌、队列和预算不会局部发布。
    /// # 参数
    /// - `owner`：认证账户主键。
    /// - `request`：受限原生请求，任务 ID 不得跨账户。
    /// - `facts`：调用方已验证的 Session 和 Window。
    /// - `now`：Unix 毫秒时间，不扩展授权。
    /// # Errors
    /// 任何校验、容量或数据库错误均回滚；成功提交后才向当前聊天交出下一工作。
    pub(crate) fn native(
        &mut self,
        owner: &str,
        request: NativeRequest,
        facts: NativeFacts,
        now: i64,
    ) -> Result<NativeResponse, Error> {
        let tx = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let mut task = load(&tx, owner, &request.task_id)?;
        let response = native::apply(&mut task, &request, &facts, now)?;
        journal::append(
            &mut task,
            journal::ProgressEvent {
                kind: if request.action == "bind" {
                    "native_bind"
                } else {
                    "native_finish"
                }
                .into(),
                ..journal::ProgressEvent::default()
            },
            now,
        );
        save(&tx, owner, &task)?;
        tx.commit()?;
        Ok(response)
    }
    /// 打开专用增量数据库并恢复中断状态；不修改现有 WebCodex 数据库。
    /// # 参数
    /// - `path`：Server 数据目录内的专用数据库路径，不允许符号链接。
    /// - `now`：当前 Unix 毫秒时间。
    /// # Errors
    /// 文件、SQLite 或恢复事务失败时返回 Storage，调用方不能启动续跑服务。
    pub fn open(path: &Path, now: i64) -> Result<Self, Error> {
        match std::fs::symlink_metadata(path) {
            Ok(meta) if !meta.is_file() || meta.file_type().is_symlink() => {
                return Err(Error::Storage)
            }
            Ok(_) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                let mut options = std::fs::OpenOptions::new();
                options.create_new(true).write(true);
                #[cfg(unix)]
                {
                    use std::os::unix::fs::OpenOptionsExt;
                    options.mode(0o600);
                }
                match options.open(path) {
                    Ok(file) => drop(file),
                    Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {}
                    Err(_) => return Err(Error::Storage),
                }
            }
            Err(_) => return Err(Error::Storage),
        }
        // 在 SQLite 创建 WAL 前限定主文件权限，使用不跟随链接的句柄，避免改到被替换的路径目标。
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            use std::os::unix::fs::PermissionsExt;
            let parent = path.parent().ok_or(Error::Storage)?;
            let metadata = std::fs::symlink_metadata(parent).map_err(|_| Error::Storage)?;
            if !metadata.is_dir()
                || metadata.file_type().is_symlink()
                || metadata.permissions().mode() & 0o022 != 0
            {
                return Err(Error::Storage);
            }
            let file = std::fs::OpenOptions::new()
                .read(true)
                .write(true)
                .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC)
                .open(path)
                .map_err(|_| Error::Storage)?;
            file.set_permissions(std::fs::Permissions::from_mode(0o600))
                .map_err(|_| Error::Storage)?;
        }
        let connection = Connection::open_with_flags(
            path,
            rusqlite::OpenFlags::SQLITE_OPEN_READ_WRITE
                | rusqlite::OpenFlags::SQLITE_OPEN_CREATE
                | rusqlite::OpenFlags::SQLITE_OPEN_NOFOLLOW,
        )?;
        connection.busy_timeout(LOCK_TIMEOUT)?;
        let version: i64 = connection.pragma_query_value(None, "user_version", |row| row.get(0))?;
        if version != 0 && version != SCHEMA_VERSION {
            return Err(Error::Storage);
        }
        connection.execute_batch("PRAGMA journal_mode=WAL; PRAGMA synchronous=FULL;
          CREATE TABLE IF NOT EXISTS continuation_tasks(owner TEXT NOT NULL,id TEXT NOT NULL,request_id TEXT NOT NULL,client TEXT NOT NULL,state TEXT NOT NULL,deadline INTEGER NOT NULL,body TEXT NOT NULL,PRIMARY KEY(owner,id),UNIQUE(owner,request_id));
          CREATE INDEX IF NOT EXISTS continuation_due ON continuation_tasks(state,deadline);
          CREATE INDEX IF NOT EXISTS continuation_client ON continuation_tasks(owner,client);")?;
        connection.pragma_update(None, "user_version", SCHEMA_VERSION)?;
        let mut store = Self { connection };
        store.maintain(now, true)?;
        Ok(store)
    }

    /// 在一次事务内处理客户端操作；待发消息和领取状态提交后才返回，当前会话标记在同一事务校验。
    /// # 参数
    /// - `owner`：HTTP 层验证的账户主键，客户端 JSON 不能指定。
    /// - `request`：已反序列化操作，其边界在此继续校验。
    /// - `now`：当前 Unix 毫秒时间。
    pub fn apply(&mut self, owner: &str, request: Request, now: i64) -> Result<Response, Error> {
        let tx = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let mut dispatch = None;
        let mut repair = None;
        let event = journal::capture(&request);
        let source_proof = journal::source_proof(&request);
        let mut task = match request {
            Request::Inspect { task_id } => {
                let task = load(&tx, owner, &task_id)?;
                let mut view = task.view();
                view.journal = task.journal;
                return Ok(Response {
                    tasks: vec![view],
                    ..Response::default()
                });
            }
            Request::List { client } => {
                Uuid::parse_str(&client).map_err(|_| Error::Invalid)?;
                let mut statement = tx.prepare("SELECT body FROM continuation_tasks WHERE owner=?1 AND client=?2 ORDER BY rowid DESC LIMIT ?3")?;
                let rows = statement
                    .query_map(params![owner, client, MAX_TASKS as i64], |row| {
                        row.get::<_, String>(0)
                    })?;
                let mut tasks = Vec::new();
                for row in rows {
                    tasks.push(
                        serde_json::from_str::<Task>(&row?)
                            .map_err(|_| Error::Storage)?
                            .view(),
                    );
                }
                return Ok(Response {
                    tasks,
                    dispatch: None,
                    repair: None,
                });
            }
            Request::Delete { task_id, client } => {
                let task = load(&tx, owner, &task_id)?;
                if task.binding.client != client {
                    return Err(Error::Conflict);
                }
                tx.execute(
                    "DELETE FROM continuation_tasks WHERE owner=?1 AND id=?2",
                    params![owner, task_id],
                )?;
                tx.commit()?;
                return Ok(Response::default());
            }
            Request::Start {
                request_id,
                task_id,
                generation,
                binding,
                options,
                page,
            } => {
                Uuid::parse_str(&request_id).map_err(|_| Error::Invalid)?;
                options.validate()?;
                binding.validate(&page)?;
                if page.draft || page.blocked || page.approval {
                    return Err(Error::Conflict);
                }
                let prior: Option<String> = tx
                    .query_row(
                        "SELECT body FROM continuation_tasks WHERE owner=?1 AND request_id=?2",
                        params![owner, request_id],
                        |row| row.get(0),
                    )
                    .optional()?;
                if let Some(body) = prior {
                    let prior: Task = serde_json::from_str(&body).map_err(|_| Error::Storage)?;
                    if prior.binding != binding
                        || prior.options != options
                        || task_id.as_ref().is_some_and(|id| id != &prior.id)
                    {
                        return Err(Error::Conflict);
                    }
                    return Ok(Response {
                        tasks: vec![prior.view()],
                        dispatch: None,
                        repair: None,
                    });
                }
                let mut task = if let Some(id) = task_id {
                    let mut old = load(&tx, owner, &id)?;
                    if old.state != "paused" && old.state != "done"
                        || Some(old.generation) != generation
                        || old.binding.client != binding.client
                        || old.binding.conversation != binding.conversation
                        || old.binding.origin != binding.origin
                        || old.binding.prefix != binding.prefix
                    {
                        return Err(Error::Conflict);
                    }
                    old.generation += 1;
                    old
                } else {
                    let owner_count: i64 = tx.query_row(
                        "SELECT count(*) FROM continuation_tasks WHERE owner=?1",
                        [owner],
                        |row| row.get(0),
                    )?;
                    let total: i64 =
                        tx.query_row("SELECT count(*) FROM continuation_tasks", [], |row| {
                            row.get(0)
                        })?;
                    if owner_count >= MAX_TASKS as i64 || total >= MAX_TOTAL_TASKS {
                        return Err(Error::Capacity);
                    }
                    Task {
                        id: Uuid::new_v4().to_string(),
                        request_id: String::new(),
                        generation: 1,
                        binding: binding.clone(),
                        options: options.clone(),
                        state: String::new(),
                        reason: String::new(),
                        rounds: 0,
                        expires_at: 0,
                        next_at: 0,
                        changed_at: now,
                        expected_user: String::new(),
                        expected_user_hash: String::new(),
                        last_reply: None,
                        pending: None,
                        messages: Vec::new(),
                        protocol_version: PROTOCOL_VERSION,
                        protocol_armed: false,
                        recovery: Recovery::default(),
                        delivery: Delivery::default(),
                        observed_at: now,
                        local_at: 0,
                        local_status: Default::default(),
                        answer_keys: Vec::new(),
                        queue: Vec::new(),
                        planning: false,
                        planning_trigger: None,
                        native: Default::default(),
                        journal: Vec::new(),
                    }
                };
                // 一个账户对同一提供方会话最多一个活动任务，不允许两个标签并发发送。
                let mut statement = tx.prepare("SELECT body FROM continuation_tasks WHERE owner=?1 AND state NOT IN ('paused','done')")?;
                for row in statement.query_map([owner], |row| row.get::<_, String>(0))? {
                    let other: Task = serde_json::from_str(&row?).map_err(|_| Error::Storage)?;
                    if other.id != task.id
                        && other.binding.conversation == binding.conversation
                        && other.binding.origin == binding.origin
                    {
                        return Err(Error::Conflict);
                    }
                }
                drop(statement);
                task.request_id = request_id;
                task.binding = binding;
                task.options = options;
                task.state = "running".into();
                task.reason.clear();
                // 人工恢复不抹掉已消耗轮数；明确重新启用到期任务才授予新的时间窗口。
                if task.rounds >= task.options.limit {
                    return Err(Error::Conflict);
                }
                if task.expires_at == 0 || now >= task.expires_at {
                    task.expires_at =
                        now + i64::from(task.options.policy.lifetime_hours) * 3_600_000;
                }
                task.next_at = now;
                task.changed_at = now;
                if task.expected_user != page.user_id
                    || task.expected_user_hash != user_hash(&page.user_text)
                {
                    record(&mut task, &page.user_id, "user", &page.user_text)?;
                }
                task.expected_user_hash = user_hash(&page.user_text);
                task.expected_user = page.user_id.clone();
                task.last_reply = None;
                native::revoke(&mut task);
                task.pending = None;
                task.protocol_version = PROTOCOL_VERSION;
                task.protocol_armed = false;
                task.delivery = Delivery::default();
                task.observed_at = now;
                task.answer_keys.clear();
                task.recovery = Recovery {
                    progress_at: now,
                    fingerprint: recovery::fingerprint(&page),
                    finish_id: page.finish_id.clone(),
                    ..Recovery::default()
                };
                task
            }
            Request::Update {
                task_id,
                generation,
                binding,
                options,
                page,
            } => {
                let mut task = load(&tx, owner, &task_id)?;
                fence(&task, generation, &binding)?;
                binding.validate(&page)?;
                options.validate()?;
                if options.limit < task.rounds
                    || page.user_id != task.expected_user
                    || user_hash(&page.user_text) != task.expected_user_hash
                {
                    return Err(Error::Conflict);
                }
                task.options = options;
                task.generation += 1;
                native::revoke(&mut task);
                task.protocol_armed = false;
                task.answer_keys.clear();
                task.observed_at = now;
                // 已授权的消息可能已经点击，只保留对账；其他旧内容在新版本下失效。
                if task.delivery.authorized_at.is_none() {
                    task.pending = None;
                    task.delivery = Delivery::default();
                    task.last_reply = None;
                    if !matches!(task.state.as_str(), "paused" | "done") {
                        task.state = "running".into();
                        task.reason.clear();
                    }
                }
                task
            }
            Request::Queue {
                task_id,
                generation,
                binding,
                kind,
                text,
                stages,
                trigger,
            } => {
                let mut task = load(&tx, owner, &task_id)?;
                fence(&task, generation, &binding)?;
                queue::add(&mut task, &kind, &text, &stages, &trigger)?;
                queue::invalidate_preparation(&mut task, now);
                task
            }
            Request::Dequeue {
                task_id,
                generation,
                binding,
                entry_id,
            } => {
                let mut task = load(&tx, owner, &task_id)?;
                fence(&task, generation, &binding)?;
                let index = task
                    .queue
                    .iter()
                    .position(|entry| entry.id == entry_id)
                    .ok_or(Error::NotFound)?;
                task.queue.remove(index);
                queue::invalidate_preparation(&mut task, now);
                task
            }
            Request::Pause {
                task_id,
                generation,
                binding,
                reason,
            } => {
                let mut task = load(&tx, owner, &task_id)?;
                fence(&task, generation, &binding)?;
                let reason = reason.as_deref().unwrap_or("user");
                if !matches!(
                    reason,
                    "user"
                        | "unavailable"
                        | "browser_restart"
                        | "send_failed"
                        | "delivery_unknown"
                        | "unsupported"
                        | "navigation"
                        | "ambiguous_tabs"
                        | "user_stopped"
                        | "page_limit"
                        | "observer_unavailable"
                        | "session_access_denied"
                        | "session_unavailable"
                        | "runtime_unavailable"
                        | "runner_access_denied"
                ) {
                    return Err(Error::Invalid);
                }
                task.pause(reason, now);
                task
            }
            Request::Repair {
                task_id,
                generation,
                binding,
                kind,
                page,
            } => {
                let mut task = load(&tx, owner, &task_id)?;
                fence(&task, generation, &binding)?;
                if !["open", "reload", "wake"].contains(&kind.as_str()) {
                    return Err(Error::Invalid);
                }
                if matches!(task.state.as_str(), "paused" | "done")
                    || task.state == "watching"
                    || !task.options.policy.recovery
                    || now >= task.expires_at
                {
                    return Err(Error::Conflict);
                }
                if task.recovery.attempts >= recovery::MAX_REPAIRS {
                    task.pause("recovery_exhausted", now);
                } else {
                    recovery::claim(&mut task, &kind, page.as_ref(), now)?;
                    repair = Some(kind.clone());
                }
                if page
                    .as_ref()
                    .is_some_and(|page| user_hash(&page.user_text) != task.expected_user_hash)
                {
                    return Err(Error::Conflict);
                }

                task
            }
            Request::Rebind {
                task_id,
                generation,
                binding,
                replacement,
                page,
            } => {
                let mut task = load(&tx, owner, &task_id)?;
                fence(&task, generation, &binding)?;
                replacement.validate(&page)?;
                if matches!(task.state.as_str(), "paused" | "done")
                    || now >= task.expires_at
                    || replacement.client != binding.client
                    || replacement.conversation != binding.conversation
                    || replacement.origin != binding.origin
                    || replacement.prefix != binding.prefix
                    || page.draft
                    || page.approval
                {
                    return Err(Error::Conflict);
                }
                let receipt = task
                    .pending
                    .as_ref()
                    .is_some_and(|p| page.user_text == p.wire_text());
                if !receipt
                    && (page.user_id != task.expected_user
                        || user_hash(&page.user_text) != task.expected_user_hash)
                {
                    return Err(Error::Conflict);
                }
                // 只迁移发送者，不清除消息债务、恢复次数、任务期限或标记版本。
                task.binding = replacement;
                task
            }
            Request::Observe {
                task_id,
                generation,
                binding,
                page,
            } => {
                let mut task = load(&tx, owner, &task_id)?;
                fence(&task, generation, &binding)?;
                binding.validate(&page)?;
                task.observed_at = now;
                // 原生完成可先于浏览器观察；关闭状态仍可只确认旧回执，不重新启用。
                let released_receipt = task.native.released.is_some()
                    && page.user_id != task.expected_user
                    && task
                        .pending
                        .as_ref()
                        .is_some_and(|pending| page.user_text == pending.wire_text());
                if !page.approval
                    && !page.activity.rate_limited
                    && matches!(task.reason.as_str(), "approval_required" | "rate_limited")
                {
                    task.reason.clear();
                }
                if matches!(task.state.as_str(), "paused" | "done") && !released_receipt {
                    task
                } else if now >= task.expires_at {
                    task.pause("expired", now);
                    task
                } else if page.activity.stop_requested {
                    task.pause("user_stopped", now);
                    task
                } else if page.approval {
                    // 人工批准仍由用户完成；等待不关闭开关，也不消耗刷新预算。
                    task.reason = "approval_required".into();
                    task.recovery.progress_at = now;
                    task
                } else if page.blocked {
                    task.pause("page_blocked", now);
                    task
                } else if (matches!(task.state.as_str(), "dispatching" | "uncertain")
                    || released_receipt)
                    && page.user_id != task.expected_user
                    && task.pending.as_ref().is_some_and(|pending| {
                        // 回执正文必须完全一致；带同一标记的人工修改仍作为新约束暂停。
                        page.user_text == pending.wire_text()
                    })
                {
                    let sent = task.pending.as_ref().ok_or(Error::Conflict)?.text.clone();
                    if let Some((id, text)) = queue::acknowledge(&mut task) {
                        record(&mut task, &id, "user", &text)?;
                    }
                    if record(&mut task, &page.user_id, "user", &sent).is_err() {
                        task.pause("context_limit", now);
                        journal::append(&mut task, event, now);
                        save(&tx, owner, &task)?;
                        tx.commit()?;
                        return Ok(Response {
                            tasks: vec![task.view()],
                            dispatch: None,
                            repair: None,
                        });
                    }
                    if let Some(message) = task
                        .messages
                        .last_mut()
                        .filter(|message| message.id == page.user_id)
                    {
                        message.source = "automatic".into();
                    }
                    task.pending = None;
                    task.delivery = Delivery::default();
                    task.native.delivered = true;
                    task.protocol_armed = !task.native.token.is_empty()
                        || sent
                            .contains(&format!("[[WEBCODEX_GOAL:{}:{}:", task.id, task.generation));
                    task.state = "running".into();
                    task.changed_at = now;
                    task.expected_user_hash = user_hash(&page.user_text);
                    task.expected_user = page.user_id.clone();
                    task.next_at = now + i64::from(task.options.delay) * 1000;
                    task.reason.clear();
                    // 回执较晚时 finish 可能已发生；保留新事件，不能把它当成旧基线丢弃。
                    task.recovery = Recovery {
                        progress_at: now,
                        fingerprint: recovery::fingerprint(&page),
                        finish_seen: page.finish_id.is_some()
                            && page.finish_id != task.recovery.finish_id,
                        finish_id: page.finish_id.clone(),
                        ..Recovery::default()
                    };
                    native::release_browser_state(&mut task);
                    task
                } else if page.user_id != task.expected_user
                    || user_hash(&page.user_text) != task.expected_user_hash
                {
                    // 新人工输入撤销旧轮次的待发权；持续开关只授权跟随同一聊天的人工请求。
                    if record(&mut task, &page.user_id, "user", &page.user_text).is_err() {
                        task.pause("context_limit", now);
                    } else if task.options.policy.continuous {
                        task.generation += 1;
                        native::revoke(&mut task);
                        task.expected_user = page.user_id.clone();
                        task.expected_user_hash = user_hash(&page.user_text);
                        task.pending = None;
                        task.delivery = Delivery::default();
                        task.last_reply = None;
                        task.protocol_armed = false;
                        task.answer_keys.clear();
                        task.state = "running".into();
                        task.reason.clear();
                        task.recovery = Recovery {
                            progress_at: now,
                            fingerprint: recovery::fingerprint(&page),
                            finish_id: page.finish_id.clone(),
                            ..Recovery::default()
                        };
                    } else {
                        task.pause("user_changed", now);
                    }
                    task
                } else if recovery::provider_wait(&mut task, &page, now) {
                    task
                } else if task.pending.is_none()
                    && task.queue.iter().any(|entry| entry.delivery == "uncertain")
                {
                    task.pause("native_receipt_unknown", now);
                    task
                } else if task.state != "running" || page.busy || page.draft || page.tools_busy {
                    if task.reason == "approval_required" || task.reason == "rate_limited" {
                        task.reason.clear();
                    }
                    if task.state == "running" {
                        recovery::watch(&mut task, &page, now);
                    }
                    if matches!(task.state.as_str(), "watching" | "ready") {
                        queue::reserve(&mut task, &page, now);
                    }
                    task
                } else if task.planning
                    && page
                        .reply
                        .as_ref()
                        .is_some_and(|reply| reply.terminal && reply.user_id == task.expected_user)
                {
                    let reply = page.reply.as_ref().ok_or(Error::Conflict)?;
                    if queue::accept_plan(&mut task, &reply.text).is_err() {
                        task.pause("plan_invalid", now);
                    } else {
                        task.last_reply = Some(reply.id.clone());
                        record(&mut task, &reply.id, "assistant", &reply.text)?;
                        queue::reserve(&mut task, &page, now);
                    }
                    task
                } else if queue::reserve(&mut task, &page, now)
                    || recovery::defer(&mut task, &page, now)
                {
                    task
                } else if let Some(reply) = page.reply.filter(|reply| {
                    reply.terminal
                        && reply.user_id == task.expected_user
                        && !reply.text.trim().is_empty()
                        && task.last_reply.as_ref() != Some(&reply.id)
                }) {
                    if record(&mut task, &reply.id, "assistant", &reply.text).is_err() {
                        task.pause("context_limit", now);
                    } else if decision::repeating(&mut task, &reply.text) {
                        task.pause("no_progress", now);
                    } else {
                        task.recovery.finish_seen = false;
                        task.last_reply = Some(reply.id);
                        task.changed_at = now;
                        if task.options.mode == Mode::Continue {
                            native::revoke(&mut task);
                            task.state = if task.options.policy.continuous {
                                "watching"
                            } else {
                                "done"
                            }
                            .into();
                            task.reason = "turn_complete".into();
                        } else if task.rounds >= task.options.limit
                            && task.options.mode != Mode::Goal
                        {
                            task.pause("round_limit", now);
                        } else {
                            match decision::decide(&task, &reply.text) {
                                Decision::Complete => {
                                    native::revoke(&mut task);
                                    task.state = if task.options.policy.continuous {
                                        "watching"
                                    } else {
                                        "done"
                                    }
                                    .into();
                                    task.reason = "goal_complete".into();
                                }
                                Decision::Continue(_) if task.rounds >= task.options.limit => {
                                    task.pause("round_limit", now)
                                }
                                Decision::Continue(text) => {
                                    task.pending = Some(Pending {
                                        id: Uuid::new_v4().to_string(),
                                        text,
                                        queue_id: None,
                                        wire: None,
                                    });
                                    task.delivery = Delivery::default();
                                    task.state = "ready".into();
                                }
                                Decision::Blocked => task.pause("chat_blocked", now),
                                Decision::Invalid => task.pause("goal_marker_missing", now),
                            }
                        }
                    }
                    task
                } else {
                    task
                }
            }
            // 单阶段旧客户端没有新的发送前授权能力，不能再领取正文。
            Request::Claim { .. } => return Err(Error::Conflict),
            Request::Prepare {
                task_id,
                generation,
                binding,
                delivery_id,
                page,
            } => {
                let mut task = load(&tx, owner, &task_id)?;
                fence(&task, generation, &binding)?;
                binding.validate(&page)?;
                dispatch = delivery::prepare(&mut task, &delivery_id, &page, now)?;
                task
            }
            Request::Authorize {
                task_id,
                generation,
                binding,
                delivery_id,
                page,
            } => {
                let mut task = load(&tx, owner, &task_id)?;
                fence(&task, generation, &binding)?;
                binding.validate(&page)?;
                delivery::authorize(&mut task, &delivery_id, &page, now)?;
                task
            }
            Request::NotSent {
                task_id,
                generation,
                binding,
                delivery_id,
                page,
            } => {
                let mut task = load(&tx, owner, &task_id)?;
                if task.binding != binding || task.delivery.generation != generation {
                    return Err(Error::Conflict);
                }
                binding.validate(&page)?;
                delivery::not_sent(&mut task, &delivery_id, &page, now)?;
                task
            }
        };
        native::confirm_source(&mut task, source_proof.as_ref(), now);
        journal::append(&mut task, event, now);
        save(&tx, owner, &task)?;
        tx.commit()?;
        Ok(Response {
            tasks: vec![task.view()],
            dispatch,
            repair,
        })
    }

    /// 按 deadline 索引处理超时；启动时检查活动任务，保留未知投递并暂停旧协议任务，不改其他数据。
    /// # 参数
    /// - `now`：当前 Unix 毫秒时间。
    /// - `restart`：仅首次打开数据库传 true，不能用于普通定时维护。
    pub fn maintain(&mut self, now: i64, restart: bool) -> Result<(), Error> {
        let tx = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let mut statement = tx.prepare("SELECT owner,body FROM continuation_tasks WHERE (state NOT IN ('paused','done') AND deadline<=?1) OR (?2 AND state NOT IN ('paused','done')) LIMIT ?3")?;
        let rows = statement.query_map(params![now, restart, MAX_TOTAL_TASKS], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
        })?;
        let mut changed = Vec::new();
        for row in rows {
            let (owner, body) = row?;
            let mut task: Task = serde_json::from_str(&body).map_err(|_| Error::Storage)?;
            if task.state == "prepared" && now < task.expires_at {
                // 从未授权的准备可以撤销；已授权投递绝不进入此分支。
                task.state = "ready".into();
                task.delivery.prepared_at = 0;
                task.reason = "prepare_expired".into();
                changed.push((owner, task));
                continue;
            }
            if task.state == "dispatching" && now < task.expires_at {
                task.state = "uncertain".into();
                task.reason = "awaiting_receipt".into();
                changed.push((owner, task));
                continue;
            }
            let reason = if task.state == "deciding"
                || (task.options.mode != Mode::Continue
                    && task.protocol_version != PROTOCOL_VERSION)
            {
                "protocol_updated"
            } else if now >= task.expires_at {
                "expired"
            } else {
                continue;
            };
            task.pause(reason, now);
            changed.push((owner, task));
        }
        drop(statement);
        for (owner, mut task) in changed {
            journal::append(
                &mut task,
                journal::ProgressEvent {
                    kind: "maintenance".into(),
                    ..journal::ProgressEvent::default()
                },
                now,
            );
            save(&tx, &owner, &task)?;
        }
        tx.commit()?;
        Ok(())
    }
}

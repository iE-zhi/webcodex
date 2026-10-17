//! Server 持有的长任务续跑控制器：账户隔离、持久化状态、Goal/Loop 决策和一次性投递。
//! 浏览器只上报公开回合并领取已决消息；本 crate 不执行文件、Shell 或其他本地工具。
mod decision;
mod delivery;
mod journal;
mod model;
mod native;
mod queue;
mod recovery;
mod store;

pub use journal::ProgressEvent;
pub use model::{
    Activity, Binding, Dispatch, Mode, Observation, Options, Policy, QueueView, Reply, Request,
    Response, SourceProof, TaskView,
};
pub use native::{NativeFacts, NativeRequest, NativeResponse};
use std::path::Path;
use std::sync::{Arc, Mutex};
use tokio::sync::Semaphore;

/// 最多 32 个并发控制请求进入账本；超额拒绝，不堆积阻塞任务。
const MAX_OPERATIONS: usize = 32;

/// 对外错误只暴露固定代码；数据库、网络及密钥正文不进入客户端错误。
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// 非法、超限或无法证明身份的输入。
    #[error("invalid_request")]
    Invalid,
    /// 任务不属于账户或已删除。
    #[error("task_not_found")]
    NotFound,
    /// 旧版本、重复发送者、已领取消息或页面状态变化。
    #[error("state_conflict")]
    Conflict,
    /// 有界任务存储容量已满。
    #[error("task_capacity")]
    Capacity,
    /// 公开上下文超过上限，不能静默裁剪用户要求。
    #[error("context_limit")]
    ContextLimit,
    /// 持久化失败；事务未提交时不能发送。
    #[error("storage_error")]
    Storage,
    /// 并发控制请求已满，调用方可稍后重新查询。
    #[error("control_busy")]
    Busy,
}

impl Error {
    /// 返回可持久化及记录日志的固定安全状态码。
    pub fn code(&self) -> &'static str {
        match self {
            Self::Invalid => "invalid_request",
            Self::NotFound => "task_not_found",
            Self::Conflict => "state_conflict",
            Self::Capacity => "task_capacity",
            Self::ContextLimit => "context_limit",
            Self::Storage => "storage_error",
            Self::Busy => "control_busy",
        }
    }
}

impl From<rusqlite::Error> for Error {
    /// 将数据库错误转换为不含 SQL、路径或数据的固定错误。
    /// # 参数
    /// - `_error`：原数据库错误，仅用于类型转换，不向客户端传播正文。
    fn from(_error: rusqlite::Error) -> Self {
        Self::Storage
    }
}

/// 可克隆的控制器；数据库按事务串行，不调用模型网络接口，取消不会回滚已提交预留。
#[derive(Clone)]
pub struct Controller {
    /// 单进程 SQLite 连接，Mutex 只在 spawn_blocking 内持有；中毒时停止操作。
    store: Arc<Mutex<store::Store>>,
    /// 阻塞工作的并发许可；HTTP 取消后仍由阻塞工作持有，不包含维护循环。
    operations: Arc<Semaphore>,
}

impl Controller {
    /// 为已认证且绑定当前进程的 Runner 发行短租约；取消 HTTP 不会撤回已领取的浏览器修复预算。
    /// # 参数
    /// - `owner`：认证层解析出的稳定账户 ID，范围一至二百字节。
    /// - `request`：已重验 client 和活动进程实例的有界请求；不能由浏览器代发。
    /// - `now`：服务端 Unix 毫秒时间，不能延长任务原授权。
    /// # Errors
    /// 非法输入、并发容量、锁中毒或数据库失败返回固定 Error，事务失败不发行动作。
    pub async fn local_lease(
        &self,
        owner: String,
        request: webcodex_core::continuation::LocalLeaseRequest,
        now: i64,
    ) -> Result<webcodex_core::continuation::LocalLease, Error> {
        if owner.is_empty() || owner.len() > 200 || owner.chars().any(char::is_control) {
            return Err(Error::Invalid);
        }
        let operation = self
            .operations
            .clone()
            .try_acquire_owned()
            .map_err(|_| Error::Busy)?;
        let store = self.store.clone();
        // 本次许可和事务交给阻塞线程；请求取消也不能增加实际并发或重放唤醒。
        tokio::task::spawn_blocking(move || {
            let _operation = operation;
            store
                .lock()
                .map_err(|_| Error::Storage)?
                .local_lease(&owner, request, now)
        })
        .await
        .map_err(|_| Error::Storage)?
    }
    /// 由认证 MCP 适配执行同一聊天的原生续跑，取消不会回滚已提交令牌和工具结果。
    /// # 参数
    /// - `owner`：可信认证账户，不可来自模型输入。
    /// - `request`：当前任务的受限原生控制请求。
    /// - `facts`：已重新授权的 Session、可信 Window 及在途工作事实。
    /// - `now`：Unix 毫秒时间；不延长任务授权。
    /// # Errors
    /// 账户、容量、账本或协议不符返回固定 Error；已提交结果只可按同一请求重读。
    pub async fn native(
        &self,
        owner: String,
        request: NativeRequest,
        facts: NativeFacts,
        now: i64,
    ) -> Result<NativeResponse, Error> {
        if owner.is_empty() || owner.len() > 200 || owner.chars().any(char::is_control) {
            return Err(Error::Invalid);
        }
        let operation = self
            .operations
            .clone()
            .try_acquire_owned()
            .map_err(|_| Error::Busy)?;
        let store = self.store.clone();
        // 许可和所有输入归阻塞工作所有；HTTP 取消不能绕过事务串行与并发容量。
        tokio::task::spawn_blocking(move || {
            let _operation = operation;
            store
                .lock()
                .map_err(|_| Error::Storage)?
                .native(&owner, request, facts, now)
        })
        .await
        .map_err(|_| Error::Storage)?
    }
    /// 打开或恢复后端账本；中断投递在启动事务中转入待对账，不兼容的旧任务暂停。
    /// # 参数
    /// - `path`：专用 SQLite 文件，位于 Server 的数据目录。
    /// - `now`：Unix 毫秒启动时间。
    pub fn open(path: &Path, now: i64) -> Result<Self, Error> {
        Ok(Self {
            store: Arc::new(Mutex::new(store::Store::open(path, now)?)),
            operations: Arc::new(Semaphore::new(MAX_OPERATIONS)),
        })
    }

    /// 执行一次认证账户的操作；异步取消后由维护器保留未确认投递，不会重复投递。
    /// # 参数
    /// - `owner`：HTTP 层验证的稳定账户 ID，1～200 字节，不能来自请求 JSON。
    /// - `request`：受限的控制、观测或领取请求。
    /// - `now`：本次操作开始的 Unix 毫秒时间。
    pub async fn handle(
        &self,
        owner: String,
        request: Request,
        now: i64,
    ) -> Result<Response, Error> {
        let operation = self
            .operations
            .clone()
            .try_acquire_owned()
            .map_err(|_| Error::Busy)?;
        if owner.is_empty() || owner.len() > 200 || owner.chars().any(char::is_control) {
            return Err(Error::Invalid);
        }
        let store = self.store.clone();
        // store、owner、request 归阻塞任务所有；取消不会中断已开始的 SQLite 原子事务。
        tokio::task::spawn_blocking(move || {
            // 许可随真正执行的事务释放，调用方取消不能绕过阻塞队列容量。
            let _operation = operation;
            store
                .lock()
                .map_err(|_| Error::Storage)?
                .apply(&owner, request, now)
        })
        .await
        .map_err(|_| Error::Storage)?
    }

    /// 后端定时处理预算及未确认操作超时；复杂度受全局任务容量限制。
    /// # 参数
    /// - `now`：当前 Unix 毫秒时间；不刷新任务授权时限。
    pub async fn maintain(&self, now: i64) -> Result<(), Error> {
        let store = self.store.clone();
        // 连接的临时独占借用只在本阻塞工作中生效，不跨异步网络请求。
        tokio::task::spawn_blocking(move || {
            store
                .lock()
                .map_err(|_| Error::Storage)?
                .maintain(now, false)
        })
        .await
        .map_err(|_| Error::Storage)?
    }
}

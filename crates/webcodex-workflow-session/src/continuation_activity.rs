//! 续跑读取的在途调用身份；与历史事件保留窗口独立，不返回工具正文，进程重启后重新观察。
use crate::SessionStore;
use std::collections::BTreeSet;
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc, Mutex, Weak,
};

/// 最多跟踪 4,096 个同时在途的调用，超出后保守声明观察不完整，不能误判空闲。
const MAX_ACTIVE_CALLS: usize = 4096;

/// 进程内调用身份集合；SessionStore 克隆共享，不序列化，不将重启前已销毁的 Future 恢复为活跃调用。
#[derive(Debug, Default)]
pub(super) struct CurrentCalls {
    /// 确切 Session 和调用 ID；结束事件只移除自己的身份，历史淘汰不影响本集合。
    calls: BTreeSet<(String, String)>,
    /// 调用 Future 被取消且没有确切结束事件的身份；仅影响所属 Session，不当成已完成。
    unknown: BTreeSet<(String, String)>,
    /// 容量溢出或锁中毒后保持 true，直到进程重启；此时禁止把缺记录当成空闲。
    pub(super) incomplete: bool,
}

impl CurrentCalls {
    /// 记录真实开始/结束边界，控制检查本身不阻塞自己的 finish。
    /// # 参数
    /// - `event`：已确认 Session 存在的账本事件，只读取类型及调用身份，不保留正文。
    pub(super) fn observe(&mut self, event: &crate::SessionEvent) {
        if event.tool_name == "update_task_continuation" {
            return;
        }
        let Some(id) = event.call_id.as_ref() else {
            if event.kind == "tool_call_started" {
                self.incomplete = true;
            }
            return;
        };
        let key = (event.session_id.clone(), id.clone());
        if event.kind == "tool_call_finished" {
            self.calls.remove(&key);
            self.unknown.remove(&key);
        } else if event.kind == "tool_call_started" {
            if self.calls.len() + self.unknown.len() >= MAX_ACTIVE_CALLS {
                self.incomplete = true;
            } else {
                self.calls.insert(key);
            }
        }
    }
}

/// 一个真实调用的共享生命周期；最后一个 Token 被丢弃且未完成时，只标记所属调用结果未知。
#[derive(Debug)]
pub(super) struct CallLifetime {
    /// 弱引用避免 Token 延长整个 SessionStore 生命周期，锁只在同步 Drop 中短暂取得。
    calls: Weak<Mutex<CurrentCalls>>,
    /// 所属 Session 和调用 ID，不保留工具参数、结果或私有内容。
    key: (String, String),
    /// 真实结束事件构造前发布；Release/Acquire 让最后一个 Token 看见明确结束。
    finished: AtomicBool,
}

impl CallLifetime {
    /// 为一个由 SessionStore 创建的真实调用建立共享生命周期，所有 Token 克隆共用它。
    /// # 参数
    /// - `calls`：原 Store 及其克隆共享的有界调用集合，只保存弱引用。
    /// - `session`：已验证存在的 Session ID。
    /// - `id`：本次新生成的确切调用 ID。
    pub(super) fn new(calls: &Arc<Mutex<CurrentCalls>>, session: &str, id: &str) -> Arc<Self> {
        Arc::new(Self {
            calls: Arc::downgrade(calls),
            key: (session.into(), id.into()),
            finished: AtomicBool::new(false),
        })
    }

    /// 真实完成路径发布结束，后续 Token Drop 不把它误标为取消；事件仍由原账本路径保存。
    pub(super) fn complete(&self) {
        self.finished.store(true, Ordering::Release);
    }
}

impl Drop for CallLifetime {
    /// 调用 Future 取消不等于操作已回滚；只标记结果未知，等待确切结束事件或人工核对。
    fn drop(&mut self) {
        if self.finished.load(Ordering::Acquire) {
            return;
        }
        let Some(calls) = self.calls.upgrade() else {
            return;
        };
        let mut calls = calls.lock().unwrap_or_else(|poison| {
            let mut calls = poison.into_inner();
            calls.incomplete = true;
            tracing::error!("continuation call observation poisoned");
            calls
        });
        if calls.calls.remove(&self.key) {
            calls.unknown.insert(self.key.clone());
        }
    }
}

/// 单个 Session 的续跑只读事实；只含调用状态及成功 finish 身份，可跨线程传递，不自行持久化。
#[derive(Clone, Debug)]
pub struct ContinuationActivity {
    /// 至少一个已开始调用未结束，或保留记录存在无法确认的缺口。
    pub busy: bool,
    /// 完整保留账本中最近成功的 finish 事件 ID；没有事件时为 None。
    pub finish_id: Option<String>,
    /// 在途身份观察是否完整；false 时调用方应报告不可确认，不能自动发送。
    pub complete: bool,
}

impl SessionStore {
    /// 在记录锁内读取与历史尾窗独立的在途身份，最近成功 finish 仅供兼容的事件触发使用。
    /// # 参数
    /// - `session_id`：调用方已完成账户/项目授权的确切 Session ID，只借用本次查询。
    /// # 返回
    /// Session 不存在时 None；容量或锁异常返回 complete=false。进程重启销毁的调用不再标为在途，持久 Job 仍由运行时单独核对。
    /// 锁顺序固定为 Session 记录再在途集合，不跨 await；扫描受事件和同时在途调用容量限制。
    pub fn continuation_activity(&self, session_id: &str) -> Option<ContinuationActivity> {
        // record 仅在查询锁内借用；集合引用不能逃出此回调。
        self.with_record_for_query(session_id, |record, _cold| {
            let (busy, complete) = self
                .continuation_calls
                .lock()
                .map(|calls| {
                    // 未知操作只阻止其所属 Session；不能因其他聊天取消而暂停本任务。
                    let unknown = calls
                        .unknown
                        .iter()
                        .any(|(session, _)| session == session_id);
                    (
                        calls.incomplete
                            || unknown
                            || calls.calls.iter().any(|(session, _)| session == session_id),
                        !calls.incomplete && !unknown,
                    )
                })
                .unwrap_or((true, false));
            let finish_id = record
                .events
                .iter()
                .rev()
                .find(|event| {
                    event.kind == "tool_call_finished"
                        && event.tool_name == "finish_coding_task"
                        && event.status.as_deref() == Some("succeeded")
                })
                .map(|event| event.event_id.clone());
            ContinuationActivity {
                busy,
                finish_id,
                complete,
            }
        })
    }
}

//! Bounded MCP execution lifetimes, shared by HTTP admission and the one Pipe supervisor.
use std::{
    collections::BTreeMap,
    time::{Duration, Instant},
};
use tokio::sync::oneshot;
use webtools_ipc::{
    mcp::{McpCallReply, McpCallRequest, MAX_CALLS},
    HostOwnership,
};
pub fn acceptance_cooldown(isolated: bool, acceptance: bool, value: Option<&str>) -> Duration {
    if isolated && acceptance {
        if let Some(ms) = value
            .and_then(|v| v.parse::<u64>().ok())
            .filter(|v| (100..=30_000).contains(v))
        {
            return Duration::from_millis(ms);
        }
    }
    Duration::from_secs(15)
}
struct Call {
    request: McpCallRequest,
    reply: Option<oneshot::Sender<McpCallReply>>,
    submitted: bool,
    cancelled: bool,
    cancellation_sent: bool,
    deadline: Instant,
}
pub struct McpDispatch {
    ownership: HostOwnership,
    calls: BTreeMap<String, Call>,
    cooldown: Option<Instant>,
    cooldown_duration: Duration,
    sync_waiters: Vec<oneshot::Sender<Result<(), String>>>,
    sync_deadline: Option<Instant>,
    sync_sent: bool,
    ownership_changed: bool,
}
impl McpDispatch {
    pub fn new(ownership: HostOwnership, cooldown_duration: Duration) -> Self {
        Self {
            ownership,
            calls: BTreeMap::new(),
            cooldown: None,
            cooldown_duration,
            sync_waiters: vec![],
            sync_deadline: None,
            sync_sent: false,
            ownership_changed: false,
        }
    }
    pub fn admit(
        &mut self,
        request: McpCallRequest,
        now: Instant,
        budget: Duration,
    ) -> Result<oneshot::Receiver<McpCallReply>, String> {
        request.validate().map_err(|e| e.to_string())?;
        if self.calls.len() >= MAX_CALLS || self.calls.contains_key(&request.call_id) {
            return Err("工具繁忙，请稍后重试。".into());
        }
        let (reply, received) = oneshot::channel();
        self.ownership
            .begin_session(&format!("mcp-active-{}", request.call_id));
        self.ownership.end_session("mcp-cooldown");
        self.cooldown = None;
        self.ownership_changed = true;
        self.calls.insert(
            request.call_id.clone(),
            Call {
                request,
                reply: Some(reply),
                submitted: false,
                cancelled: false,
                cancellation_sent: false,
                deadline: now + budget,
            },
        );
        Ok(received)
    }
    pub fn active_calls(&self) -> usize {
        self.calls.len()
    }
    pub fn has_work(&self) -> bool {
        !self.calls.is_empty() || self.sync_deadline.is_some()
    }
    pub fn take_ownership_changed(&mut self) -> bool {
        std::mem::take(&mut self.ownership_changed)
    }
    pub fn pending(&self) -> Vec<McpCallRequest> {
        self.calls
            .values()
            .filter(|v| !v.submitted && !v.cancelled)
            .map(|v| v.request.clone())
            .collect()
    }
    pub fn mark_submitted(&mut self, id: &str) {
        if let Some(call) = self.calls.get_mut(id) {
            call.submitted = true;
        }
    }
    pub fn cancel(&mut self, id: &str) {
        if let Some(call) = self.calls.get_mut(id) {
            call.cancelled = true;
        }
    }
    pub fn cancel_all(&mut self) {
        for call in self.calls.values_mut() {
            call.cancelled = true;
        }
    }
    pub fn complete(&mut self, reply: McpCallReply, now: Instant) {
        let Some(call) = self.calls.remove(&reply.call_id) else {
            return;
        };
        // Preserve an owner while the final request is released, including a simultaneous new admission.
        if call.submitted && self.calls.is_empty() && !self.cooldown_duration.is_zero() {
            self.ownership.begin_session("mcp-cooldown");
            self.cooldown = Some(now + self.cooldown_duration);
        }
        self.ownership
            .end_session(&format!("mcp-active-{}", reply.call_id));
        self.ownership_changed = true;
        if let Some(sender) = call.reply {
            let _ = sender.send(reply);
        }
    }
    pub fn tick(&mut self, now: Instant) -> Vec<String> {
        let mut cancel = vec![];
        let mut finish = vec![];
        for (id, call) in self.calls.iter_mut() {
            if now >= call.deadline {
                call.cancelled = true;
                if let Some(sender) = call.reply.take() {
                    let _ = sender.send(McpCallReply::failed(
                        id,
                        "failed",
                        "工具调用超时；已请求停止，已发生的操作不会回滚。",
                    ));
                }
            }
            if call.cancelled && !call.cancellation_sent {
                if call.submitted {
                    call.cancellation_sent = true;
                    cancel.push(id.clone());
                } else {
                    finish.push(id.clone());
                }
            }
        }
        for id in finish {
            self.complete(
                McpCallReply::failed(&id, "cancelled", "工具调用已取消。"),
                now,
            );
        }
        if self.cooldown.is_some_and(|deadline| now >= deadline) {
            self.cooldown = None;
            self.ownership.end_session("mcp-cooldown");
            self.ownership_changed = true;
        }
        if self.sync_deadline.is_some_and(|deadline| now >= deadline) {
            self.catalog_synced(Err("插件工具同步超时。".into()));
        }
        cancel
    }
    pub fn disconnect(&mut self) {
        self.disconnect_inner(false);
    }
    /// A cooperative exit may race with admission. Only calls whose tool frame
    /// was never attempted can continue on the next host, using the same budget.
    pub fn idle_exit(&mut self, undelivered: &[String]) {
        for id in undelivered {
            if let Some(call) = self.calls.get_mut(id) { call.submitted = false; }
        }
        self.disconnect_inner(true);
    }
    fn disconnect_inner(&mut self, preserve_undelivered: bool) {
        for (id, call) in std::mem::take(&mut self.calls) {
            if preserve_undelivered && !call.submitted {
                self.calls.insert(id,call);
                continue;
            }
            self.ownership.end_session(&format!("mcp-active-{id}"));
            if let Some(sender) = call.reply {
                let status = if call.submitted {
                    "outcome-unknown"
                } else {
                    "failed"
                };
                let _ = sender.send(McpCallReply::failed(
                    &id,
                    status,
                    if call.submitted {
                        "调用中断，执行结果未知。"
                    } else {
                        "执行引擎未就绪。"
                    },
                ));
            }
        }
        self.cooldown = None;
        self.ownership.end_session("mcp-cooldown");
        if preserve_undelivered { self.sync_sent=false; }
        else { self.catalog_synced(Err("执行引擎已断开。".into())); }
        self.ownership_changed=true;
    }
    pub fn await_catalog(
        &mut self,
        now: Instant,
    ) -> Result<oneshot::Receiver<Result<(), String>>, String> {
        if self.sync_waiters.len() >= MAX_CALLS {
            return Err("工具同步繁忙，请稍后重试。".into());
        }
        let (tx, rx) = oneshot::channel();
        if self.sync_deadline.is_none() {
            self.sync_deadline = Some(now + Duration::from_secs(60));
            self.ownership.begin_session("mcp-sync");
            self.sync_sent = false;
            self.ownership_changed = true;
        }
        self.sync_waiters.push(tx);
        Ok(rx)
    }
    pub fn needs_sync(&mut self) -> bool {
        if self.sync_deadline.is_some() && !self.sync_sent {
            self.sync_sent = true;
            true
        } else {
            false
        }
    }
    pub fn catalog_synced(&mut self, result: Result<(), String>) {
        for sender in std::mem::take(&mut self.sync_waiters) {
            let _ = sender.send(result.clone());
        }
        self.sync_deadline = None;
        self.sync_sent = false;
        self.ownership.end_session("mcp-sync");
        self.ownership_changed = true;
    }
}
#[cfg(test)]
#[path = "mcp_dispatch_tests.rs"]
mod tests;

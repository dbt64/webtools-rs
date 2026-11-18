//! Tray-owned schedules. Cron uses local wall time; missed ticks are coalesced.
use chrono::{Datelike, Local, TimeZone, Timelike};
use serde::{Deserialize, Serialize};
use std::{collections::HashSet, path::{Path, PathBuf}};
pub fn now_ms() -> u64 { std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default().as_millis() as u64 }
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(tag = "kind", rename_all = "camelCase", deny_unknown_fields)]
pub enum Schedule {
    Once { #[serde(rename = "atMs")] at_ms: u64 },
    Cron { expression: String, #[serde(default, rename = "startMs")] start_ms: Option<u64>, #[serde(default, rename = "endMs")] end_ms: Option<u64> },
}
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct TaskRequest { pub code: String, pub description: String, pub schedule: Schedule }
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TaskExecutionLog {
    pub execute_at_ms: u64, pub duration_ms: u64, pub success: bool, pub error: Option<String>,
}
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ScheduledTask {
    pub plugin_id: String, pub feature_id: String, pub code: String, pub description: String,
    pub schedule: Schedule, pub next_run_ms: Option<u64>, pub created_at_ms: u64,
    pub enabled: bool, pub running: bool, pub successes: u64, pub failures: u64,
    pub last_run_ms: Option<u64>, pub last_error: Option<String>,
    #[serde(default)] pub logs: Vec<TaskExecutionLog>,
}
#[derive(Clone, Debug, Default, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ScheduledTasks { tasks: Vec<ScheduledTask>, #[serde(skip)] path: Option<PathBuf>, #[serde(skip)] in_flight: HashSet<(String, String)> }
// Numeric five-field cron: lists, ranges and steps; Sunday is 0 or 7.
fn field(value: &str, min: u32, max: u32) -> Result<HashSet<u32>, String> {
    let mut values = HashSet::new();
    for part in value.split(',') {
        let pieces: Vec<_> = part.split('/').collect();
        if pieces.len() > 2 { return Err("Invalid cron step".into()); }
        let step = if pieces.len() == 2 { pieces[1].parse::<u32>().map_err(|_| "Invalid cron step")? } else { 1 };
        if step == 0 || step > max { return Err("Invalid cron step".into()); }
        let bounds: Vec<_> = pieces[0].split('-').collect();
        let (a,b) = if pieces[0] == "*" { (min,max) } else if bounds.len() == 2 {
            (bounds[0].parse().map_err(|_| "Invalid cron range")?,bounds[1].parse().map_err(|_| "Invalid cron range")?)
        } else if bounds.len() == 1 { let n = bounds[0].parse().map_err(|_| "Invalid cron field")?; (n, if pieces.len() == 2 { max } else { n }) }
        else { return Err("Invalid cron field".into()); };
        if a < min || b > max || a > b { return Err("Cron value out of range".into()); }
        for n in (a..=b).step_by(step as usize) { values.insert(n); }
    } Ok(values)
}
fn cron_fields(expression: &str) -> Result<Vec<HashSet<u32>>, String> {
    let parts: Vec<_> = expression.split_whitespace().collect();
    if parts.len() != 5 || expression.len() > 120 { return Err("Use a standard five-field cron expression".into()); }
    [(0,59),(0,23),(1,31),(1,12),(0,7)].iter().enumerate().map(|(i,(a,b))| {
        let mut normalized = parts[i].to_ascii_uppercase();
        let names: &[&str] = if i == 3 { &["JAN","FEB","MAR","APR","MAY","JUN","JUL","AUG","SEP","OCT","NOV","DEC"] }
            else if i == 4 { &["SUN","MON","TUE","WED","THU","FRI","SAT"] } else { &[] };
        for (n,name) in names.iter().enumerate() { normalized = normalized.replace(name, &(n + if i == 3 { 1 } else { 0 }).to_string()); }
        field(&normalized,*a,*b)
    }).collect()
}
impl Schedule {
    pub fn next_after(&self, now: u64) -> Result<Option<u64>, String> {
        // IPC accepts u64, but chrono uses signed, bounded calendar dates.
        // Reject unrepresentable values before casts or minute rounding.
        let validate_date = |value: u64| -> Result<(), String> {
            let signed = i64::try_from(value).map_err(|_| "Schedule date out of range")?;
            Local.timestamp_millis_opt(signed).single().ok_or("Schedule date out of range")?;
            Ok(())
        };
        validate_date(now)?;
        match self {
            Self::Once { at_ms } => { validate_date(*at_ms)?; Ok((*at_ms > now).then_some(*at_ms)) },
            Self::Cron { expression, start_ms, end_ms } => {
                for value in [start_ms, end_ms].into_iter().flatten() { validate_date(*value)?; }
                if start_ms.zip(*end_ms).is_some_and(|(a,b)| a >= b) { return Err("Schedule start must precede end".into()); }
                let fields = cron_fields(expression)?; let parts: Vec<_> = expression.split_whitespace().collect();
                let lower = now.saturating_add(1).max(start_ms.unwrap_or(0));
                let mut tick = lower.saturating_add(59999) / 60000 * 60000;
                let limit = tick.saturating_add(8 * 366 * 24 * 60 * 60000).min(end_ms.unwrap_or(u64::MAX));
                while tick <= limit {
                    let Some(date) = Local.timestamp_millis_opt(tick as i64).single() else { return Err("Schedule date out of range".into()); };
                    let dow = date.weekday().num_days_from_sunday();
                    let day = fields[2].contains(&date.day()); let week = fields[4].contains(&dow) || dow == 0 && fields[4].contains(&7);
                    let day_matches = if parts[2] != "*" && parts[4] != "*" { day || week } else { day && week };
                    if !fields[3].contains(&date.month()) || !day_matches {
                        // Skip a calendar day, respecting local DST boundaries;
                        // sparse/unsatisfiable schedules need thousands of checks,
                        // rather than millions of per-minute timezone conversions.
                        let midnight = date.date_naive().succ_opt().and_then(|d| d.and_hms_opt(0,0,0))
                            .and_then(|d| Local.from_local_datetime(&d).earliest()).map(|d| d.timestamp_millis() as u64);
                        tick = midnight.filter(|n| *n > tick).unwrap_or(tick + 3600000); continue;
                    }
                    if !fields[1].contains(&date.hour()) { tick += (60 - date.minute() as u64) * 60000; continue; }
                    if fields[0].contains(&date.minute()) { return Ok(Some(tick)); }
                    tick += 60000;
                } Ok(None)
            }
        }
    }
}
impl ScheduledTasks {
    pub fn load(path: &Path, now: u64) -> Result<Self, String> {
        let mut store: Self = match std::fs::read(path) {
            Ok(bytes) => serde_json::from_slice(&bytes).map_err(|e| format!("Cannot read schedule store: {e}"))?,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Self::default(), Err(e) => return Err(e.to_string()),
        }; store.path = Some(path.to_owned());
        for task in &mut store.tasks {
            if task.running { task.failures += 1; task.last_error = Some("Host stopped during execution".into()); task.running = false; }
            if !task.enabled { task.next_run_ms = None; }
            else if matches!(task.schedule, Schedule::Cron { .. }) {
                task.next_run_ms = task.schedule.next_after(now)?;
                task.enabled = task.next_run_ms.is_some();
            }
        } store.save()?; Ok(store)
    }
    pub fn save(&self) -> Result<(), String> {
        if let Some(path) = &self.path {
            std::fs::create_dir_all(path.parent().ok_or("Invalid schedule path")?).map_err(|e| e.to_string())?;
            let tmp = path.with_extension("json.tmp");
            std::fs::write(&tmp, serde_json::to_vec_pretty(self).map_err(|e| e.to_string())?).map_err(|e| e.to_string())?;
            std::fs::rename(&tmp, path).map_err(|e| e.to_string())?;
        } Ok(())
    }
    pub fn list(&self) -> &[ScheduledTask] { &self.tasks }
    pub fn suspend_plugin(&mut self, plugin: &str, reason: &str) -> Result<(), String> {
        for task in &mut self.tasks { if task.plugin_id == plugin && task.enabled { task.enabled = false; task.next_run_ms = None; task.last_error = Some(reason.into()); } }
        self.save()
    }
    pub fn request(&mut self, plugin: &str, feature: &str, request: TaskRequest, now: u64) -> Result<ScheduledTask, String> {
        if request.code.is_empty() || request.code.len() > 64 || !request.code.bytes().all(|c| c.is_ascii_alphanumeric() || b"_-".contains(&c)) || request.description.trim().is_empty() || request.description.len() > 1024 { return Err("Invalid task code or description".into()); }
        if self.in_flight.contains(&(plugin.into(),request.code.clone())) || self.tasks.iter().any(|t| t.plugin_id == plugin && t.code == request.code) { return Err("Task code already exists or is still running; remove it after execution before recreating".into()); }
        if self.tasks.iter().filter(|t| t.plugin_id == plugin).count() >= 64 { return Err("Task quota exceeded".into()); }
        let next = request.schedule.next_after(now)?.ok_or("Schedule has no future execution")?;
        let task = ScheduledTask { plugin_id: plugin.into(), feature_id: feature.into(), code: request.code, description: request.description, schedule: request.schedule,
            next_run_ms: Some(next), created_at_ms: now, enabled: true, running: false, successes: 0, failures: 0, last_run_ms: None, last_error: None, logs: vec![] };
        self.tasks.push(task.clone()); if let Err(e) = self.save() { self.tasks.pop(); return Err(e); } Ok(task)
    }
    pub fn remove(&mut self, plugin: &str, code: &str) -> Result<bool, String> {
        let old = self.tasks.clone(); self.tasks.retain(|t| t.plugin_id != plugin || t.code != code);
        if let Err(e) = self.save() { self.tasks = old; return Err(e); } Ok(old.len() != self.tasks.len())
    }
    pub fn take_due(&mut self, now: u64) -> Result<Vec<ScheduledTask>, String> {
        let old = self.tasks.clone(); let mut due = vec![]; let mut changed = false;
        // Compute first so an invalid entry cannot leave earlier entries running.
        let next: Result<Vec<_>,_> = self.tasks.iter().map(|t| if t.enabled && !t.running && t.next_run_ms.is_some_and(|n| n <= now) { t.schedule.next_after(now) } else { Ok(t.next_run_ms) }).collect();
        let next = next?;
        for (task,next_run) in self.tasks.iter_mut().zip(next) {
            if matches!(&task.schedule, Schedule::Cron { end_ms: Some(end), .. } if now > *end) {
                if task.enabled { task.enabled = false; task.next_run_ms = None; changed = true; }
                continue;
            }
            if task.enabled && !task.running && task.next_run_ms.is_some_and(|n| n <= now) {
                task.running = true; task.last_run_ms = Some(now); task.next_run_ms = next_run;
                if task.next_run_ms.is_none() { task.enabled = false; } due.push(task.clone());
            }
        } if changed || !due.is_empty() { if let Err(e) = self.save() { self.tasks = old; return Err(e); } }
        for task in &due { self.in_flight.insert((task.plugin_id.clone(),task.code.clone())); } Ok(due)
    }
    pub fn record_result(&mut self, plugin: &str, code: &str, error: Option<String>) -> Result<(), String> {
        self.in_flight.remove(&(plugin.into(),code.into()));
        if let Some(task) = self.tasks.iter_mut().find(|t| t.plugin_id == plugin && t.code == code) {
            task.running = false; if error.is_some() { task.failures += 1; } else { task.successes += 1; }
            task.last_error = error.map(|s| s.chars().take(1024).collect());
            let finished = now_ms();
            task.logs.insert(0, TaskExecutionLog { execute_at_ms: task.last_run_ms.unwrap_or(finished),
                duration_ms: finished.saturating_sub(task.last_run_ms.unwrap_or(finished)),
                success: task.last_error.is_none(), error: task.last_error.clone() });
            task.logs.truncate(30);
        } self.save()
    }
    pub fn operation(&mut self, method: &str, payload: &serde_json::Value) -> Result<serde_json::Value, String> {
        // These operations are routed only by the trusted Manager IPC surface.
        // Plugin SDK list/remove retain their plugin-scoped identity below.
        if method == "plugin-tasks-admin-list" {
            let offset = match payload.get("offset") {
                Some(value) => usize::try_from(value.as_u64().ok_or("Invalid task offset")?).map_err(|_|"Invalid task offset")?,
                None => 0,
            };
            // Sixteen worst-case bounded error histories fit the existing 4 MiB
            // pipe frame. Manager joins pages; SDK keeps its summary-only list.
            return Ok(serde_json::json!(self.tasks.iter().skip(offset).take(16).collect::<Vec<_>>()));
        }
        let plugin = payload["pluginId"].as_str().ok_or("Missing plugin identity")?;
        match method {
            "plugin-tasks-suspend" => { self.suspend_plugin(plugin, "Plugin disabled, removed or schedule permission revoked; recreate task to resume")?; Ok(serde_json::Value::Null) },
            "plugin-tasks-list" => {
                let mut rows = serde_json::json!(self.tasks.iter().filter(|t| t.plugin_id == plugin).collect::<Vec<_>>());
                // Detailed history belongs to the Manager view. Keep the SDK's
                // bounded list response small even when every task has errors.
                for row in rows.as_array_mut().unwrap() { row.as_object_mut().unwrap().remove("logs"); }
                Ok(rows)
            },
            "plugin-tasks-remove" | "plugin-tasks-admin-remove" => Ok(serde_json::json!(self.remove(plugin, payload["code"].as_str().ok_or("Missing task code")?)?)),
            "plugin-tasks-request" => {
                let request = serde_json::from_value(payload["request"].clone()).map_err(|e| e.to_string())?;
                Ok(serde_json::json!(self.request(plugin, payload["featureId"].as_str().ok_or("Missing feature")?, request, now_ms())?))
            }, _ => Err("Unknown schedule operation".into()),
        }
    }
}
#[cfg(test)] mod tests {
    use super::*;
    fn request(schedule: Schedule) -> TaskRequest { TaskRequest { code:"stable".into(),description:"test".into(),schedule } }
    #[test] fn once_no_overlap_and_stats() {
        let mut s = ScheduledTasks::default(); s.request("p","f",request(Schedule::Once {at_ms:2000}),1000).unwrap();
        assert!(s.take_due(1999).unwrap().is_empty()); assert_eq!(s.take_due(2000).unwrap().len(),1);
        assert!(s.take_due(9000).unwrap().is_empty()); s.record_result("p","stable",Some("intentional".into())).unwrap();
        assert_eq!(s.list()[0].failures,1); assert!(!s.remove("other","stable").unwrap()); assert!(s.remove("p","stable").unwrap());
    }
    #[test] fn execution_log_is_bounded_and_old_stores_remain_readable() {
        let mut s = ScheduledTasks::default();
        s.request("p","f",request(Schedule::Cron{expression:"* * * * *".into(),start_ms:None,end_ms:None}),1000).unwrap();
        for i in 1..=35 {
            assert_eq!(s.take_due(i * 60000).unwrap().len(),1);
            s.record_result("p","stable",(i % 2 == 0).then(||"test failure".to_string())).unwrap();
        }
        assert_eq!(s.list()[0].logs.len(),30);
        assert!(s.list()[0].logs[0].success);
        assert_eq!(s.list()[0].logs[1].error.as_deref(),Some("test failure"));
        let summary = s.operation("plugin-tasks-list", &serde_json::json!({"pluginId":"p"})).unwrap();
        assert!(summary[0].get("logs").is_none());
        let detail = s.operation("plugin-tasks-admin-list", &serde_json::json!({})).unwrap();
        assert_eq!(detail[0]["logs"].as_array().unwrap().len(),30);
        let mut old = serde_json::to_value(&s).unwrap();
        old["tasks"][0].as_object_mut().unwrap().remove("logs");
        let restored: ScheduledTasks = serde_json::from_value(old).unwrap();
        assert!(restored.list()[0].logs.is_empty());
    }
    #[test] fn manager_history_pages_fit_pipe_and_preserve_every_task() {
        let mut s = ScheduledTasks::default();
        for i in 0..33 {
            let mut req = request(Schedule::Once { at_ms: 2000 }); req.code = format!("task-{i}");
            s.request("p","f",req,1000).unwrap();
        }
        for task in &mut s.tasks {
            task.logs = vec![TaskExecutionLog { execute_at_ms: 1000, duration_ms: 1,
                success: false, error: Some("\0".repeat(1024)) };30];
        }
        let mut count = 0;
        for offset in [0,16,32] {
            let page = s.operation("plugin-tasks-admin-list", &serde_json::json!({"offset":offset})).unwrap();
            assert!(serde_json::to_vec(&page).unwrap().len() < crate::MAX_MESSAGE_BYTES - 1024);
            count += page.as_array().unwrap().len();
        }
        assert_eq!(count,33);
        assert!(s.operation("plugin-tasks-admin-list", &serde_json::json!({"offset":-1})).is_err());
    }
    #[test] fn cron_syntax_bounds_and_coalescing() {
        for expression in ["* * * * *","*/5 9-17 * * 1-5","0,30 * * * 0,7","0 0 29 2 *"] {
            assert!(Schedule::Cron{expression:expression.into(),start_ms:None,end_ms:None}.next_after(1700000000000).unwrap().is_some());
        }
        for expression in ["* * * *","60 * * * *","*/0 * * * *","* * 0 * *"] { assert!(cron_fields(expression).is_err()); }
        let mut s = ScheduledTasks::default(); s.request("p","f",request(Schedule::Cron{expression:"* * * * *".into(),start_ms:None,end_ms:None}),1000).unwrap();
        assert_eq!(s.take_due(600000).unwrap().len(),1); assert_eq!(s.list()[0].next_run_ms,Some(660000)); assert!(s.take_due(700000).unwrap().is_empty());
        s.record_result("p","stable",None).unwrap(); assert_eq!(s.take_due(700000).unwrap().len(),1);
    }
    #[test] fn persistence_restart_skips_cron_and_records_interruption() {
        let path = std::env::temp_dir().join(format!("wt-tasks-{}.json",now_ms()));
        let mut s=ScheduledTasks::load(&path,1000).unwrap(); s.request("p","f",request(Schedule::Cron{expression:"* * * * *".into(),start_ms:None,end_ms:None}),1000).unwrap();
        s.take_due(60000).unwrap(); let restored=ScheduledTasks::load(&path,600000).unwrap();
        assert_eq!(restored.list()[0].next_run_ms,Some(660000)); assert_eq!(restored.list()[0].failures,1); assert!(!restored.list()[0].running);
        std::fs::remove_file(path).unwrap();
    }
    #[test] fn expired_window_never_executes_overdue_tick_and_removed_running_code_cannot_overlap() {
        let mut s=ScheduledTasks::default();
        s.request("p","f",request(Schedule::Cron{expression:"* * * * *".into(),start_ms:None,end_ms:Some(120000)}),1000).unwrap();
        assert!(s.take_due(180000).unwrap().is_empty()); assert!(!s.list()[0].enabled);
        s.remove("p","stable").unwrap(); s.request("p","f",request(Schedule::Once{at_ms:2000}),1000).unwrap();
        s.take_due(2000).unwrap(); s.remove("p","stable").unwrap();
        assert!(s.request("p","f",request(Schedule::Once{at_ms:9000}),3000).is_err());
        s.record_result("p","stable",None).unwrap(); assert!(s.request("p","f",request(Schedule::Once{at_ms:9000}),3000).is_ok());
    }
    #[test] fn suspended_plugin_has_no_future_tick() {
        let mut s=ScheduledTasks::default(); s.request("p","f",request(Schedule::Cron{expression:"* * * * *".into(),start_ms:None,end_ms:None}),1000).unwrap();
        s.suspend_plugin("p","disabled").unwrap(); assert!(s.take_due(600000).unwrap().is_empty()); assert_eq!(s.list()[0].next_run_ms,None);
    }
    #[test] fn manager_lists_all_plugins_and_removes_only_the_selected_identity() {
        let mut s = ScheduledTasks::default();
        for plugin in ["p", "other"] { s.request(plugin,"f",request(Schedule::Once { at_ms: 2000 }),1000).unwrap(); }
        assert_eq!(s.operation("plugin-tasks-admin-list", &serde_json::json!({})).unwrap().as_array().unwrap().len(), 2);
        assert_eq!(s.operation("plugin-tasks-list", &serde_json::json!({"pluginId":"p"})).unwrap().as_array().unwrap().len(), 1);
        assert_eq!(s.operation("plugin-tasks-admin-remove", &serde_json::json!({"pluginId":"p","code":"stable"})).unwrap(), true);
        assert_eq!(s.list().len(), 1); assert_eq!(s.list()[0].plugin_id, "other");
        assert!(s.operation("plugin-tasks-admin-remove", &serde_json::json!({"code":"stable"})).is_err());
    }
    #[test] fn restart_keeps_paused_tasks_without_a_next_run() {
        let path = std::env::temp_dir().join(format!("wt-paused-tasks-{}.json",now_ms()));
        let mut s = ScheduledTasks::load(&path,1000).unwrap();
        s.request("p","f",request(Schedule::Cron { expression: "* * * * *".into(), start_ms:None, end_ms:None }),1000).unwrap();
        s.suspend_plugin("p","disabled").unwrap();
        let restored = ScheduledTasks::load(&path,100000).unwrap();
        assert!(!restored.list()[0].enabled); assert_eq!(restored.list()[0].next_run_ms,None);
        std::fs::remove_file(path).unwrap();
    }
    #[test] fn final_cron_tick_finishes_and_extreme_dates_are_rejected() {
        let mut s = ScheduledTasks::default();
        s.request("p","f",request(Schedule::Cron { expression:"* * * * *".into(), start_ms:None, end_ms:Some(60000) }),1000).unwrap();
        assert_eq!(s.take_due(60000).unwrap().len(),1);
        s.record_result("p","stable",None).unwrap();
        assert!(!s.list()[0].enabled); assert_eq!(s.list()[0].successes,1);
        assert!(Schedule::Once { at_ms:u64::MAX }.next_after(1000).is_err());
        assert!(Schedule::Cron { expression:"* * * * *".into(), start_ms:Some(u64::MAX), end_ms:None }.next_after(1000).is_err());
        assert!(Schedule::Cron { expression:"* * * * *".into(), start_ms:None, end_ms:None }.next_after(u64::MAX).is_err());
    }
}

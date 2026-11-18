//! Rust-owned MCP listener and settings; execution stays in the existing Electron host.
use crate::{host_runtime::HostCommand, mcp_dispatch::McpDispatch};
use rmcp::{model::*, service::RequestContext, ErrorData, RoleServer, ServerHandler};
use serde_json::{json, Value};
use std::future::IntoFuture;
use std::{
    borrow::Cow,
    path::Path,
    sync::{
        atomic::{AtomicBool, Ordering},
        mpsc::Sender,
        Arc, Mutex,
    },
    time::{Duration, Instant},
};
use tokio::sync::oneshot;
use webtools_ipc::{
    mcp::{McpCallRequest, McpToolsProjection},
    mcp_store::{McpProjectionStore, McpSettingsStore},
};

struct State {
    settings: Mutex<McpSettingsStore>,
    catalog: Mutex<McpProjectionStore>,
    generation: Mutex<u64>,
    dispatch: Arc<Mutex<McpDispatch>>,
    wake: Sender<HostCommand>,
    running: AtomicBool,
    error: Mutex<Option<String>>,
    #[cfg(test)]
    panic_on_start: AtomicBool,
}
struct Listener {
    stop: Option<oneshot::Sender<()>>,
    thread: Option<std::thread::JoinHandle<()>>,
}
pub struct McpServerHandle {
    state: Arc<State>,
    listener: Mutex<Listener>,
}
impl McpServerHandle {
    pub fn new(
        root: &Path,
        profile: &Path,
        dispatch: Arc<Mutex<McpDispatch>>,
        wake: Sender<HostCommand>,
    ) -> Result<Arc<Self>, String> {
        let settings = McpSettingsStore::load_at_profile(root, profile)
            .map_err(|_| "无法读取本地 MCP 服务配置，原文件已保留。".to_string())?;
        let catalog = McpProjectionStore::load(profile.join("mcp-tools-projection.json"))
            .map_err(|e| e.to_string())?;
        Ok(Arc::new(Self {
            state: Arc::new(State {
                settings: Mutex::new(settings),
                catalog: Mutex::new(catalog),
                generation: Mutex::new(0),
                dispatch,
                wake,
                running: AtomicBool::new(false),
                error: Mutex::new(None),
                #[cfg(test)]
                panic_on_start: AtomicBool::new(false),
            }),
            listener: Mutex::new(Listener {
                stop: None,
                thread: None,
            }),
        }))
    }
    pub fn status(&self) -> Value {
        let config = self.state.settings.lock().unwrap().value.clone();
        let running = self.state.running.load(Ordering::SeqCst);
        let error = self.state.error.lock().unwrap().clone();
        let mut result = json!({"enabled":config.enabled,"running":running,"phase":if error.is_some(){"error"}else if running{"running"}else{"disabled"},"catalogReady":self.state.catalog.lock().unwrap().snapshot().is_some(),"activeCalls":self.state.dispatch.lock().unwrap().active_calls()});
        if running {
            result["url"] = json!(format!("http://127.0.0.1:{}/mcp", config.port));
            result["token"] = json!(config.token);
        }
        if let Some(message) = error {
            result["error"] = json!({"code":"MCP_START_FAILED","message":message});
        }
        result
    }
    pub fn start_saved(self: &Arc<Self>) -> Result<(), String> {
        if self.state.settings.lock().unwrap().value.enabled {
            self.enable(false)?;
        }
        Ok(())
    }
    pub fn set_enabled(self: &Arc<Self>, enabled: bool) -> Result<Value, String> {
        if enabled {
            self.enable(true)?;
        } else {
            self.stop();
            self.state
                .settings
                .lock()
                .unwrap()
                .set_enabled(false)
                .map_err(|_| "无法保存 MCP 设置。".to_string())?;
            *self.state.error.lock().unwrap() = None;
        }
        Ok(self.status())
    }
    fn enable(self: &Arc<Self>, persist: bool) -> Result<(), String> {
        let mut control = self.listener.lock().unwrap();
        if !self.state.running.load(Ordering::SeqCst) {
            stop_listener(&mut control);
            let config = self.state.settings.lock().unwrap().value.clone();
            let state = self.state.clone();
            let (tx, rx) = std::sync::mpsc::sync_channel(1);
            let (stop, stopped) = oneshot::channel();
            *self.state.error.lock().unwrap() = None;
            control.stop = Some(stop);
            control.thread = Some(std::thread::spawn(move || {
                let worker = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    let runtime = tokio::runtime::Builder::new_current_thread()
                        .enable_all()
                        .build()
                        .map_err(|e| e.to_string())?;
                    runtime.block_on(async {
                        let listener=match tokio::net::TcpListener::bind(("127.0.0.1",config.port)).await {
                            Ok(listener)=>listener,Err(_)=>{let error=format!("无法启动本地 MCP 服务，请检查端口 {} 是否被占用。",config.port);let _=tx.send(Err(error.clone()));return Err(error);}
                        };
                        let router=crate::mcp_http::router(Handler(state.clone()),config.token,format!("127.0.0.1:{}",config.port));
                        state.running.store(true,Ordering::SeqCst); let _=tx.send(Ok(()));
                        #[cfg(test)]
                        if state.panic_on_start.swap(false,Ordering::SeqCst) { panic!("injected MCP worker unwind"); }
                        tokio::select! { result=axum::serve(listener,router).into_future()=>result.map_err(|e|e.to_string()), _=stopped=>Ok(()) }
                    })
                }));
                state.running.store(false, Ordering::SeqCst);
                let error = match worker {
                    Ok(Ok(())) => None,
                    Ok(Err(error)) => Some(error),
                    Err(_) => Some("本地 MCP 服务异常停止，请重试。".into()),
                };
                if let Some(error) = error {
                    *state.error.lock().unwrap() = Some(error);
                    state.dispatch.lock().unwrap().cancel_all();
                    let _ = state.wake.send(HostCommand::McpWake);
                }
            }));
            if let Err(error) = rx
                .recv_timeout(Duration::from_secs(5))
                .map_err(|_| "本地 MCP 服务启动超时。".to_string())
                .and_then(|r| r)
            {
                stop_listener(&mut control);
                *self.state.error.lock().unwrap() = Some(error.clone());
                return Err(error);
            }
        }
        if persist {
            if self
                .state
                .settings
                .lock()
                .unwrap()
                .set_enabled(true)
                .is_err()
            {
                stop_listener(&mut control);
                let error = "无法保存 MCP 设置，服务未开启。".to_string();
                *self.state.error.lock().unwrap() = Some(error.clone());
                return Err(error);
            }
        }
        Ok(())
    }
    pub fn stop(&self) {
        stop_listener(&mut self.listener.lock().unwrap());
        self.state.running.store(false, Ordering::SeqCst);
        self.state.dispatch.lock().unwrap().cancel_all();
        let _ = self.state.wake.send(HostCommand::McpWake);
    }
    pub fn operation(self: &Arc<Self>, kind: &str, payload: &Value) -> Result<Value, String> {
        match kind {
            "mcp-gateway-negotiate-v1" => {
                let mut generation = self.state.generation.lock().unwrap();
                *generation += 1;
                self.state
                    .catalog
                    .lock()
                    .unwrap()
                    .begin_generation(*generation);
                Ok(json!({}))
            }
            "mcp-tools-update-v1" => {
                let next: McpToolsProjection =
                    serde_json::from_value(payload.clone()).map_err(|e| e.to_string())?;
                let generation = *self.state.generation.lock().unwrap();
                let result = self
                    .state
                    .catalog
                    .lock()
                    .unwrap()
                    .replace_from(generation, next)
                    .map_err(|e| e.to_string());
                self.state
                    .dispatch
                    .lock()
                    .unwrap()
                    .catalog_synced(result.clone());
                result.map(|_| json!({"saved":true}))
            }
            "mcp-admin-get-v1" => Ok(self.status()),
            "mcp-admin-preferences-v1" => Ok(
                json!({"disabledPlugins":self.state.settings.lock().unwrap().value.disabled_plugins}),
            ),
            "mcp-admin-set-enabled-v1" => {
                self.set_enabled(payload["enabled"].as_bool().ok_or("无效开关。")?)
            }
            "mcp-admin-set-plugin-enabled-v1" => {
                let id = payload["pluginId"].as_str().ok_or("无效插件。")?;
                let enabled = payload["enabled"].as_bool().ok_or("无效开关。")?;
                self.state
                    .settings
                    .lock()
                    .unwrap()
                    .set_plugin_enabled(id, enabled)
                    .map_err(|e| e.to_string())?;
                Ok(Value::Null)
            }
            _ => Err("Unknown MCP operation".into()),
        }
    }
}
fn stop_listener(listener: &mut Listener) {
    if let Some(stop) = listener.stop.take() {
        let _ = stop.send(());
    }
    if let Some(thread) = listener.thread.take() {
        let _ = thread.join();
    }
}
impl Drop for McpServerHandle {
    fn drop(&mut self) {
        self.stop();
    }
}

#[derive(Clone)]
struct Handler(Arc<State>);
impl Handler {
    async fn ensure_catalog(&self) -> Result<(), ErrorData> {
        if self.0.catalog.lock().unwrap().snapshot().is_some() {
            return Ok(());
        }
        let receive = self
            .0
            .dispatch
            .lock()
            .unwrap()
            .await_catalog(Instant::now())
            .map_err(|e| ErrorData::internal_error(e, None))?;
        if self.0.wake.send(HostCommand::McpWake).is_err() {
            self.0
                .dispatch
                .lock()
                .unwrap()
                .catalog_synced(Err("执行引擎不可用。".into()));
            return Err(ErrorData::internal_error("执行引擎不可用。", None));
        }
        receive
            .await
            .map_err(|_| ErrorData::internal_error("工具同步已中断。", None))?
            .map_err(|e| ErrorData::internal_error(e, None))
    }
    fn tools(&self) -> Vec<Value> {
        let disabled = self
            .0
            .settings
            .lock()
            .unwrap()
            .value
            .disabled_plugins
            .clone();
        self.0.catalog.lock().unwrap().snapshot().map(|snapshot|snapshot.tools.into_iter().filter(|t|!disabled.contains(&t.plugin_id)).map(|t| {
            let mut tool=json!({"name":t.name(),"description":t.description,"inputSchema":t.input_schema}); if let Some(output)=t.output_schema{tool["outputSchema"]=output;} tool
        }).collect()).unwrap_or_default()
    }
}
struct CancelOnDrop {
    state: Arc<State>,
    id: String,
}
impl Drop for CancelOnDrop {
    fn drop(&mut self) {
        self.state.dispatch.lock().unwrap().cancel(&self.id);
        let _ = self.state.wake.send(HostCommand::McpWake);
    }
}
impl ServerHandler for Handler {
    fn get_info(&self) -> ServerConfig {
        let mut info = ServerConfig::default();
        info.capabilities = ServerCapabilities::builder().enable_tools().build();
        info
    }
    fn supported_protocol_versions(&self) -> Cow<'static, [ProtocolVersion]> {
        Cow::Owned(vec![
            ProtocolVersion::V_2025_11_25,
            ProtocolVersion::V_2025_06_18,
            ProtocolVersion::V_2025_03_26,
        ])
    }
    async fn list_tools(
        &self,
        _: Option<PaginatedRequestParams>,
        _: RequestContext<RoleServer>,
    ) -> Result<ListToolsResult, ErrorData> {
        self.ensure_catalog().await?;
        serde_json::from_value(json!({"tools":self.tools()}))
            .map_err(|_| ErrorData::internal_error("工具列表无效。", None))
    }
    async fn call_tool(
        &self,
        request: CallToolRequestParams,
        context: RequestContext<RoleServer>,
    ) -> Result<CallToolResponse, ErrorData> {
        self.ensure_catalog().await?;
        if !self
            .tools()
            .iter()
            .any(|tool| tool["name"].as_str() == Some(request.name.as_ref()))
        {
            return Ok(tool_error("该工具不存在，或所属插件当前不可用。"));
        }
        let id = webtools_ipc::new_request_id();
        let request = McpCallRequest {
            call_id: id.clone(),
            tool_name: request.name.to_string(),
            arguments: json!(request.arguments.unwrap_or_default()),
            deadline_unix_ms: webtools_ipc::scheduled_tasks::now_ms() as u64 + 60_000,
        };
        let receive = match self.0.dispatch.lock().unwrap().admit(
            request,
            Instant::now(),
            Duration::from_secs(60),
        ) {
            Ok(receive) => receive,
            Err(error) => return Ok(tool_error(&error)),
        };
        let _guard = CancelOnDrop {
            state: self.0.clone(),
            id: id.clone(),
        };
        if self.0.wake.send(HostCommand::McpWake).is_err() {
            self.0.dispatch.lock().unwrap().complete(
                webtools_ipc::mcp::McpCallReply::failed(&id, "failed", "执行引擎不可用。"),
                Instant::now(),
            );
            return Ok(tool_error("执行引擎不可用。"));
        }
        tokio::select! {
            reply=receive=>{
                let reply=reply.map_err(|_|ErrorData::internal_error("调用连接已中断。",None))?;
                if let Some(result)=reply.result {return serde_json::from_value::<CallToolResult>(result).map(Into::into).map_err(|_|ErrorData::internal_error("插件返回值无效。",None));}
                Ok(tool_error(reply.error.as_deref().unwrap_or("工具调用已取消。")))
            },
            _=context.ct.cancelled()=>Ok(tool_error("工具调用已取消。")),
        }
    }
}
fn tool_error(message: &str) -> CallToolResponse {
    CallToolResult::error(vec![ContentBlock::text(message)]).into()
}
#[cfg(test)]
#[path = "mcp_server_tests.rs"]
mod tests;

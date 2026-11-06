//! 外部 A2A 1.0 JSON-RPC 执行器与 Agent Card 探测。
//!
//! 行为对齐来源仓库的 A2A adapter：
//!
//! - 只接受与 Card 同 origin 的 `JSONRPC` 1.0 接口，输入/输出模式必须包含 `text/plain`；
//! - 每次请求携带 `A2A-Version: 1.0`，Card 与 RPC 共用可选 Bearer 凭据；
//! - Card 声明 streaming 时消费 SSE，否则发送 `SendMessage` 并按 `GetTask` 轮询；
//! - 只接受文本 Part；Artifact 按 `artifactId` 与 `append` 语义组装；
//! - 取消通过 `CancelTask`，只有远端返回 `TASK_STATE_CANCELED` 才算确认停止。
//!
//! 当前 adapter 只发送文本，用户资源不会转发给远端，存在资源时 fail closed。
//! HTTP 请求、任务轮询与 SSE 读取都有时限，外部端点不能无限占用 Run。

use std::collections::BTreeMap;
use std::future::Future;
use std::sync::Arc;
use std::time::{Duration, Instant};

use async_trait::async_trait;
use reqwest::header::{ACCEPT, CONTENT_TYPE};
use reqwest::{Client, Response, Url};
use serde::Deserialize;
use serde_json::{Value, json};
use tpl_agent_core::error::AgentError;
use tpl_agent_core::ports::{
    CredentialResolver, ExecutionOutcome, ExecutionRequest, RunExecutor, RuntimeProbe,
    RuntimeProbeResult,
};
use tpl_agent_core::run_events::message_delta;
use tpl_agent_core::time_utils::now;
use uuid::Uuid;

use crate::skills;

const A2A_PROTOCOL_VERSION: &str = "1.0";
const JSONRPC_BINDING: &str = "JSONRPC";
const TEXT_PLAIN: &str = "text/plain";

const METHOD_SEND_MESSAGE: &str = "SendMessage";
const METHOD_SEND_STREAMING_MESSAGE: &str = "SendStreamingMessage";
const METHOD_GET_TASK: &str = "GetTask";
const METHOD_CANCEL_TASK: &str = "CancelTask";

const HTTP_TIMEOUT: Duration = Duration::from_secs(60);
const CONNECT_TIMEOUT: Duration = Duration::from_secs(5);
const CANCEL_POLL_INTERVAL: Duration = Duration::from_millis(250);
const TASK_POLL_INTERVAL: Duration = Duration::from_secs(1);
const TASK_POLL_TIMEOUT: Duration = Duration::from_secs(300);

const MAX_CARD_BYTES: usize = 1024 * 1024;
const MAX_RPC_RESPONSE_BYTES: usize = 4 * 1024 * 1024;
const MAX_STREAMED_TEXT_BYTES: usize = 8 * 1024 * 1024;

const TASK_STATE_UNSPECIFIED: &str = "TASK_STATE_UNSPECIFIED";
const TASK_STATE_SUBMITTED: &str = "TASK_STATE_SUBMITTED";
const TASK_STATE_WORKING: &str = "TASK_STATE_WORKING";
const TASK_STATE_COMPLETED: &str = "TASK_STATE_COMPLETED";
const TASK_STATE_FAILED: &str = "TASK_STATE_FAILED";
const TASK_STATE_CANCELED: &str = "TASK_STATE_CANCELED";
const TASK_STATE_INPUT_REQUIRED: &str = "TASK_STATE_INPUT_REQUIRED";
const TASK_STATE_REJECTED: &str = "TASK_STATE_REJECTED";
const TASK_STATE_AUTH_REQUIRED: &str = "TASK_STATE_AUTH_REQUIRED";

const TASK_STATE_NAMES: [&str; 9] = [
    TASK_STATE_UNSPECIFIED,
    TASK_STATE_SUBMITTED,
    TASK_STATE_WORKING,
    TASK_STATE_COMPLETED,
    TASK_STATE_FAILED,
    TASK_STATE_CANCELED,
    TASK_STATE_INPUT_REQUIRED,
    TASK_STATE_REJECTED,
    TASK_STATE_AUTH_REQUIRED,
];

const TASK_STATES_WITHOUT_CONTINUATION: [&str; 6] = [
    TASK_STATE_COMPLETED,
    TASK_STATE_FAILED,
    TASK_STATE_CANCELED,
    TASK_STATE_REJECTED,
    TASK_STATE_INPUT_REQUIRED,
    TASK_STATE_AUTH_REQUIRED,
];

fn request_failed() -> AgentError {
    AgentError::Upstream("a2a_request_failed")
}

fn authentication_required() -> AgentError {
    AgentError::Upstream("a2a_authentication_required")
}

fn credential_unavailable() -> AgentError {
    AgentError::Upstream("a2a_credential_unavailable")
}

/// 外部 A2A 执行器；按 Run 快照中的 endpoint 与凭据别名调用远端。
pub struct A2aExecutor {
    client: A2aClient,
}

impl A2aExecutor {
    /// 创建执行器并初始化受限 HTTP 客户端。
    ///
    /// # Errors
    ///
    /// TLS 等客户端初始化失败时返回内部错误。
    pub fn new(credentials: Arc<dyn CredentialResolver>) -> Result<Self, AgentError> {
        Ok(Self {
            client: A2aClient::new(credentials)?,
        })
    }
}

/// 外部 A2A Runtime 的 Agent Card 探针。
pub struct A2aRuntimeProbe {
    client: A2aClient,
}

impl A2aRuntimeProbe {
    /// 创建探针并初始化受限 HTTP 客户端。
    ///
    /// # Errors
    ///
    /// TLS 等客户端初始化失败时返回内部错误。
    pub fn new(credentials: Arc<dyn CredentialResolver>) -> Result<Self, AgentError> {
        Ok(Self {
            client: A2aClient::new(credentials)?,
        })
    }
}

struct A2aClient {
    http: Client,
    credentials: Arc<dyn CredentialResolver>,
}

impl A2aClient {
    fn new(credentials: Arc<dyn CredentialResolver>) -> Result<Self, AgentError> {
        let http = Client::builder()
            .timeout(HTTP_TIMEOUT)
            .connect_timeout(CONNECT_TIMEOUT)
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .map_err(|error| {
                tracing::error!(error = %error, "构建 A2A HTTP 客户端失败");
                AgentError::Internal
            })?;
        Ok(Self { http, credentials })
    }

    fn resolve_token(&self, alias: Option<&str>) -> Result<Option<String>, AgentError> {
        match alias {
            None => Ok(None),
            Some(alias) => self
                .credentials
                .resolve(alias)
                .map(Some)
                .ok_or_else(credential_unavailable),
        }
    }

    async fn load_card(
        &self,
        card_url: &str,
        token: Option<&str>,
    ) -> Result<LoadedCard, AgentError> {
        let card_url = parse_card_url(card_url)?;
        let mut request = self
            .http
            .get(card_url.clone())
            .header("A2A-Version", A2A_PROTOCOL_VERSION);
        if let Some(token) = token {
            request = request.bearer_auth(token);
        }
        let response = request.send().await.map_err(|_| request_failed())?;
        match response.status().as_u16() {
            401 | 403 => return Err(authentication_required()),
            status if !(200..300).contains(&status) => return Err(request_failed()),
            _ => {}
        }
        let body = read_body_limited(response, MAX_CARD_BYTES).await?;
        let card: A2aCard = serde_json::from_slice(&body).map_err(|_| request_failed())?;
        card.into_loaded(&card_url, token.is_some())
    }

    async fn post_jsonrpc(
        &self,
        endpoint: &Url,
        method: &str,
        params: Value,
        token: Option<&str>,
        accept: &str,
    ) -> Result<Response, AgentError> {
        let payload = json!({
            "jsonrpc": "2.0",
            "id": Uuid::new_v4().to_string(),
            "method": method,
            "params": params,
        });
        let mut request = self
            .http
            .post(endpoint.clone())
            .header("A2A-Version", A2A_PROTOCOL_VERSION)
            .header(CONTENT_TYPE, "application/json")
            .header(ACCEPT, accept)
            .json(&payload);
        if let Some(token) = token {
            request = request.bearer_auth(token);
        }
        let response = request.send().await.map_err(|_| request_failed())?;
        if !response.status().is_success() {
            tracing::debug!(
                method,
                status = %response.status(),
                "A2A RPC 返回非成功状态"
            );
            return Err(request_failed());
        }
        Ok(response)
    }

    async fn cancel_task(
        &self,
        endpoint: &Url,
        token: Option<&str>,
        task_id: Option<&str>,
    ) -> bool {
        let Some(task_id) = task_id else {
            return false;
        };
        let params = json!({"id": task_id});
        let Ok(response) = self
            .post_jsonrpc(
                endpoint,
                METHOD_CANCEL_TASK,
                params,
                token,
                "application/json",
            )
            .await
        else {
            return false;
        };
        let Ok(task) = read_rpc_response_json(response, MAX_RPC_RESPONSE_BYTES).await else {
            return false;
        };
        task.get("status")
            .and_then(|status| task_state_value(status.get("state")))
            == Some(TASK_STATE_CANCELED)
    }
}

#[async_trait]
impl RunExecutor for A2aExecutor {
    async fn execute(&self, request: ExecutionRequest) -> Result<ExecutionOutcome, AgentError> {
        let runtime = request.run.snapshot.runtime();
        let card_url = runtime
            .endpoint_url
            .as_deref()
            .ok_or(AgentError::Internal)?;
        if !request.resources.is_empty() {
            return Err(AgentError::Execution("a2a_resources_unsupported"));
        }
        let token = self
            .client
            .resolve_token(runtime.credential_alias.as_deref())?;
        let skill_context = skills::assemble_skill_context(
            &request.run.snapshot.agent.prompt,
            &request.run.snapshot.skills,
            &request.skill_assets,
        )?;
        let message_text = compose_message_text(
            &skill_context.effective_prompt,
            &request.run.snapshot.input.content,
        );
        let message = json!({
            "messageId": request.assistant_message_id.to_string(),
            "role": "ROLE_USER",
            "parts": [{"text": message_text}],
        });
        let card = self.client.load_card(card_url, token.as_deref()).await?;
        let outcome = if card.streaming {
            self.run_streaming(&card.endpoint, token.as_deref(), &request, &message)
                .await?
        } else {
            self.run_unary(&card.endpoint, token.as_deref(), &request, &message)
                .await?
        };
        match outcome {
            A2aRunOutcome::Completed(content) => Ok(ExecutionOutcome {
                assistant_content: content,
                artifacts: Vec::new(),
                external_stop_confirmed: false,
            }),
            A2aRunOutcome::Cancelled {
                task_id,
                remote_canceled,
            } => {
                let confirmed = if remote_canceled {
                    true
                } else {
                    self.client
                        .cancel_task(&card.endpoint, token.as_deref(), task_id.as_deref())
                        .await
                };
                Ok(ExecutionOutcome {
                    assistant_content: String::new(),
                    artifacts: Vec::new(),
                    external_stop_confirmed: confirmed,
                })
            }
        }
    }
}

#[async_trait]
impl RuntimeProbe for A2aRuntimeProbe {
    async fn probe(
        &self,
        endpoint_url: &str,
        credential_alias: Option<&str>,
    ) -> Result<RuntimeProbeResult, AgentError> {
        let token = match self.client.resolve_token(credential_alias) {
            Ok(token) => token,
            Err(_) => return Ok(probe_unavailable("凭据别名对应的密钥未配置。")),
        };
        match self.client.load_card(endpoint_url, token.as_deref()).await {
            Ok(card) => Ok(RuntimeProbeResult {
                ready: true,
                status_detail: None,
                card_name: Some(card.name),
            }),
            Err(error) => Ok(probe_unavailable(probe_detail(&error))),
        }
    }
}

impl A2aExecutor {
    async fn run_unary(
        &self,
        endpoint: &Url,
        token: Option<&str>,
        request: &ExecutionRequest,
        message: &Value,
    ) -> Result<A2aRunOutcome, AgentError> {
        let params = send_message_params(message);
        let future = self.client.post_jsonrpc(
            endpoint,
            METHOD_SEND_MESSAGE,
            params,
            token,
            "application/json",
        );
        let Some(response) = await_with_cancellation(future, request).await? else {
            return Ok(A2aRunOutcome::Cancelled {
                task_id: None,
                remote_canceled: false,
            });
        };
        let result = read_rpc_response_json(response, MAX_RPC_RESPONSE_BYTES).await?;
        let mut accumulator = RunAccumulator::default();
        accumulator.apply(&result, request).await?;
        self.finish(endpoint, token, request, accumulator).await
    }

    async fn run_streaming(
        &self,
        endpoint: &Url,
        token: Option<&str>,
        request: &ExecutionRequest,
        message: &Value,
    ) -> Result<A2aRunOutcome, AgentError> {
        let params = send_message_params(message);
        let future = self.client.post_jsonrpc(
            endpoint,
            METHOD_SEND_STREAMING_MESSAGE,
            params,
            token,
            "text/event-stream",
        );
        let Some(response) = await_with_cancellation(future, request).await? else {
            return Ok(A2aRunOutcome::Cancelled {
                task_id: None,
                remote_canceled: false,
            });
        };
        let mut reader = SseReader::new(response).await?;
        let mut accumulator = RunAccumulator::default();
        loop {
            let next_event = await_with_cancellation(reader.next_event(), request).await?;
            match next_event {
                None => {
                    return Ok(A2aRunOutcome::Cancelled {
                        task_id: accumulator.task_id.clone(),
                        remote_canceled: false,
                    });
                }
                Some(None) => break,
                Some(Some(event)) => {
                    accumulator.apply(&event, request).await?;
                    if accumulator.is_terminal() {
                        break;
                    }
                }
            }
        }
        self.finish(endpoint, token, request, accumulator).await
    }

    async fn finish(
        &self,
        endpoint: &Url,
        token: Option<&str>,
        request: &ExecutionRequest,
        mut accumulator: RunAccumulator,
    ) -> Result<A2aRunOutcome, AgentError> {
        if request.cancellation.is_cancel_requested().await? {
            return Ok(A2aRunOutcome::Cancelled {
                task_id: accumulator.task_id.clone(),
                remote_canceled: false,
            });
        }
        if !accumulator.is_terminal() {
            match accumulator.completion_plan() {
                CompletionPlan::DirectMessage => {
                    return Ok(A2aRunOutcome::Completed(accumulator.output));
                }
                CompletionPlan::PollTask => {
                    let task_id = accumulator.task_id.clone();
                    let reached_terminal = self
                        .poll_until_terminal(endpoint, token, request, &mut accumulator)
                        .await?;
                    if !reached_terminal {
                        return Ok(A2aRunOutcome::Cancelled {
                            task_id,
                            remote_canceled: false,
                        });
                    }
                }
                CompletionPlan::Complete => {}
                CompletionPlan::Incomplete => return Err(request_failed()),
            }
        }
        let state = accumulator.state.ok_or_else(request_failed)?;
        if state == TASK_STATE_COMPLETED {
            let content = complete_text(&mut accumulator, request).await?;
            return Ok(A2aRunOutcome::Completed(content));
        }
        if state == TASK_STATE_CANCELED && request.cancellation.is_cancel_requested().await? {
            return Ok(A2aRunOutcome::Cancelled {
                task_id: accumulator.task_id,
                remote_canceled: true,
            });
        }
        Err(AgentError::Upstream(terminal_failure_code(state)))
    }

    async fn poll_until_terminal(
        &self,
        endpoint: &Url,
        token: Option<&str>,
        request: &ExecutionRequest,
        accumulator: &mut RunAccumulator,
    ) -> Result<bool, AgentError> {
        let deadline = Instant::now() + TASK_POLL_TIMEOUT;
        loop {
            if Instant::now() >= deadline {
                return Err(request_failed());
            }
            let task_id = accumulator.task_id.clone().ok_or_else(request_failed)?;
            let future = self.client.post_jsonrpc(
                endpoint,
                METHOD_GET_TASK,
                json!({"id": task_id}),
                token,
                "application/json",
            );
            let Some(response) = await_with_cancellation(future, request).await? else {
                return Ok(false);
            };
            let task = read_rpc_response_json(response, MAX_RPC_RESPONSE_BYTES).await?;
            accumulator.apply_task(&task)?;
            if accumulator.is_terminal() {
                return Ok(true);
            }
            if sleep_with_cancellation(request, TASK_POLL_INTERVAL).await? {
                return Ok(false);
            }
        }
    }
}

enum A2aRunOutcome {
    Completed(String),
    Cancelled {
        task_id: Option<String>,
        remote_canceled: bool,
    },
}

#[derive(Debug, PartialEq, Eq)]
struct LoadedCard {
    name: String,
    streaming: bool,
    endpoint: Url,
}

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
struct A2aCard {
    name: String,
    #[serde(alias = "supported_interfaces")]
    supported_interfaces: Vec<A2aInterface>,
    capabilities: A2aCapabilities,
    #[serde(alias = "default_input_modes")]
    default_input_modes: Vec<String>,
    #[serde(alias = "default_output_modes")]
    default_output_modes: Vec<String>,
    #[serde(alias = "security_schemes")]
    security_schemes: BTreeMap<String, A2aSecurityScheme>,
    #[serde(alias = "security_requirements")]
    security_requirements: Vec<A2aSecurityRequirement>,
    skills: Vec<A2aSkill>,
}

impl A2aCard {
    fn into_loaded(self, card_url: &Url, has_token: bool) -> Result<LoadedCard, AgentError> {
        if !self
            .default_input_modes
            .iter()
            .any(|mode| mode == TEXT_PLAIN)
            || !self
                .default_output_modes
                .iter()
                .any(|mode| mode == TEXT_PLAIN)
        {
            return Err(request_failed());
        }
        self.validate_security(has_token)?;
        let endpoint = self.select_endpoint(card_url)?;
        let name = if self.name.trim().is_empty() {
            "A2A Agent".to_owned()
        } else {
            self.name.trim().to_owned()
        };
        Ok(LoadedCard {
            name,
            streaming: self.capabilities.streaming,
            endpoint,
        })
    }

    fn validate_security(&self, has_token: bool) -> Result<(), AgentError> {
        let requirements = self
            .security_requirements
            .iter()
            .chain(
                self.skills
                    .iter()
                    .flat_map(|skill| skill.security_requirements.iter()),
            )
            .collect::<Vec<_>>();
        if requirements.is_empty() {
            return Ok(());
        }
        if !has_token {
            return Err(authentication_required());
        }
        for requirement in requirements {
            if requirement.schemes.is_empty() {
                continue;
            }
            if requirement
                .schemes
                .keys()
                .all(|name| self.is_bearer_scheme(name))
            {
                return Ok(());
            }
        }
        Err(request_failed())
    }

    fn is_bearer_scheme(&self, name: &str) -> bool {
        self.security_schemes
            .get(name)
            .and_then(|scheme| scheme.http_auth_security_scheme.as_ref())
            .is_some_and(|scheme| scheme.scheme.eq_ignore_ascii_case("bearer"))
    }

    fn select_endpoint(&self, card_url: &Url) -> Result<Url, AgentError> {
        let card_origin = url_origin(card_url)?;
        for interface in &self.supported_interfaces {
            if interface.protocol_binding != JSONRPC_BINDING {
                continue;
            }
            if !matches!(interface.protocol_version.as_str(), "1.0" | "1.0.0") {
                continue;
            }
            let Ok(endpoint) = Url::parse(&interface.url) else {
                continue;
            };
            let Ok(origin) = url_origin(&endpoint) else {
                continue;
            };
            if origin == card_origin {
                return Ok(endpoint);
            }
        }
        Err(request_failed())
    }
}

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
struct A2aInterface {
    url: String,
    #[serde(alias = "protocol_binding")]
    protocol_binding: String,
    #[serde(alias = "protocol_version")]
    protocol_version: String,
}

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
struct A2aCapabilities {
    streaming: bool,
}

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
struct A2aSecurityScheme {
    #[serde(alias = "http_auth_security_scheme")]
    http_auth_security_scheme: Option<A2aHttpAuthScheme>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
struct A2aHttpAuthScheme {
    scheme: String,
}

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
struct A2aSecurityRequirement {
    schemes: BTreeMap<String, A2aStringList>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
struct A2aStringList {
    list: Vec<String>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
struct A2aSkill {
    #[serde(alias = "security_requirements")]
    security_requirements: Vec<A2aSecurityRequirement>,
}

#[derive(Default)]
struct RunAccumulator {
    task_id: Option<String>,
    latest_task: Option<Value>,
    latest_status: Option<Value>,
    state: Option<&'static str>,
    output: String,
    artifacts: ArtifactAccumulator,
    direct_message_received: bool,
}

/// 一次 A2A 调用结束后如何收口。
#[derive(Debug, PartialEq, Eq)]
enum CompletionPlan {
    /// 已到达终态，直接检查结果。
    Complete,
    /// 对端直接返回了一条完整的 Agent Message；即使带 taskId 也不再轮询。
    DirectMessage,
    /// 对端返回了非终态 Task，需要按 id 轮询。
    PollTask,
    /// 既没有终态、也没有可轮询的 Task 或完整 Message。
    Incomplete,
}

impl RunAccumulator {
    fn completion_plan(&self) -> CompletionPlan {
        if self.is_terminal() {
            return CompletionPlan::Complete;
        }
        if self.direct_message_received {
            return CompletionPlan::DirectMessage;
        }
        if self.task_id.is_some() {
            return CompletionPlan::PollTask;
        }
        CompletionPlan::Incomplete
    }

    async fn apply(
        &mut self,
        response: &Value,
        request: &ExecutionRequest,
    ) -> Result<(), AgentError> {
        let object = response.as_object().ok_or_else(request_failed)?;
        if let Some(task) = object.get("task") {
            self.apply_task(task)?;
            return Ok(());
        }
        if let Some(message) = object.get("message") {
            self.apply_message(message, request).await?;
            return Ok(());
        }
        if let Some(status_update) = object
            .get("statusUpdate")
            .or_else(|| object.get("status_update"))
        {
            self.apply_status_update(status_update, request).await?;
            return Ok(());
        }
        if let Some(artifact_update) = object
            .get("artifactUpdate")
            .or_else(|| object.get("artifact_update"))
        {
            self.apply_artifact_update(artifact_update)?;
            return Ok(());
        }
        Err(request_failed())
    }

    fn apply_task(&mut self, task: &Value) -> Result<(), AgentError> {
        let id = task.get("id").and_then(Value::as_str).unwrap_or_default();
        if !id.is_empty() {
            self.task_id = Some(id.to_owned());
        }
        let status = task.get("status").ok_or_else(request_failed)?;
        let state = task_state_name(status.get("state"))?;
        self.latest_status = Some(status.clone());
        self.latest_task = Some(task.clone());
        self.state = Some(state);
        Ok(())
    }

    async fn apply_message(
        &mut self,
        message: &Value,
        request: &ExecutionRequest,
    ) -> Result<(), AgentError> {
        let task_id = message
            .get("taskId")
            .or_else(|| message.get("task_id"))
            .and_then(Value::as_str)
            .filter(|task_id| !task_id.is_empty());
        if let Some(task_id) = task_id {
            self.task_id = Some(task_id.to_owned());
        }
        if role_is_agent(message.get("role")) {
            self.direct_message_received = true;
            let text = text_from_parts(parts_of(message)?)?;
            self.append_text(&text, request).await?;
        }
        Ok(())
    }

    async fn apply_status_update(
        &mut self,
        update: &Value,
        request: &ExecutionRequest,
    ) -> Result<(), AgentError> {
        let task_id = update
            .get("taskId")
            .or_else(|| update.get("task_id"))
            .and_then(Value::as_str)
            .filter(|task_id| !task_id.is_empty());
        if let Some(task_id) = task_id {
            self.task_id = Some(task_id.to_owned());
        }
        let status = update.get("status").ok_or_else(request_failed)?;
        let state = task_state_name(status.get("state"))?;
        if state == TASK_STATE_COMPLETED
            && let Some(message) = status.get("message")
        {
            let text = text_from_message(message)?;
            self.append_text(&text, request).await?;
        }
        self.latest_status = Some(status.clone());
        self.latest_task = None;
        self.state = Some(state);
        Ok(())
    }

    fn apply_artifact_update(&mut self, update: &Value) -> Result<(), AgentError> {
        let task_id = update
            .get("taskId")
            .or_else(|| update.get("task_id"))
            .and_then(Value::as_str)
            .filter(|task_id| !task_id.is_empty());
        if let Some(task_id) = task_id {
            self.task_id = Some(task_id.to_owned());
        }
        let artifact = update.get("artifact").ok_or_else(request_failed)?;
        let append = update
            .get("append")
            .and_then(Value::as_bool)
            .unwrap_or(false);
        self.artifacts.apply(artifact, append)
    }

    async fn append_text(
        &mut self,
        text: &str,
        request: &ExecutionRequest,
    ) -> Result<(), AgentError> {
        if text.is_empty() {
            return Ok(());
        }
        if self.output.len().saturating_add(text.len()) > MAX_STREAMED_TEXT_BYTES {
            return Err(request_failed());
        }
        self.output.push_str(text);
        request
            .observer
            .observe(message_delta(now(), request.assistant_message_id, 0))
            .await
    }

    fn is_terminal(&self) -> bool {
        self.state
            .is_some_and(|state| TASK_STATES_WITHOUT_CONTINUATION.contains(&state))
    }
}

#[derive(Default)]
struct ArtifactAccumulator {
    entries: Vec<(String, String)>,
}

impl ArtifactAccumulator {
    fn apply(&mut self, artifact: &Value, append: bool) -> Result<(), AgentError> {
        let artifact_id = artifact
            .get("artifactId")
            .or_else(|| artifact.get("artifact_id"))
            .and_then(Value::as_str)
            .filter(|artifact_id| !artifact_id.is_empty())
            .ok_or_else(request_failed)?;
        let text = text_from_parts(parts_of(artifact)?)?;
        if append {
            let entry = self
                .entries
                .iter_mut()
                .find(|(existing_id, _)| existing_id == artifact_id)
                .ok_or_else(request_failed)?;
            entry.1.push_str(&text);
        } else if let Some(entry) = self
            .entries
            .iter_mut()
            .find(|(existing_id, _)| existing_id == artifact_id)
        {
            entry.1 = text;
        } else {
            self.entries.push((artifact_id.to_owned(), text));
        }
        Ok(())
    }

    fn render(&self) -> String {
        self.entries
            .iter()
            .map(|(_, text)| text.as_str())
            .filter(|text| !text.is_empty())
            .collect::<Vec<_>>()
            .join("\n\n")
    }
}

async fn complete_text(
    accumulator: &mut RunAccumulator,
    request: &ExecutionRequest,
) -> Result<String, AgentError> {
    let final_text = final_task_text(accumulator)?;
    if !final_text.is_empty() {
        if !final_text.starts_with(&accumulator.output) {
            return Err(request_failed());
        }
        let remainder = final_text[accumulator.output.len()..].to_owned();
        accumulator.append_text(&remainder, request).await?;
    }
    Ok(accumulator.output.clone())
}

fn final_task_text(accumulator: &RunAccumulator) -> Result<String, AgentError> {
    if let Some(status) = &accumulator.latest_status
        && let Some(message) = status.get("message").filter(|message| !message.is_null())
    {
        let text = text_from_message(message)?;
        if !text.is_empty() {
            return Ok(text);
        }
    }
    if let Some(task) = &accumulator.latest_task {
        if let Some(history) = task.get("history").and_then(Value::as_array)
            && let Some(message) = history
                .iter()
                .rev()
                .find(|message| role_is_agent(message.get("role")))
        {
            let text = text_from_message(message)?;
            if !text.is_empty() {
                return Ok(text);
            }
        }
        if let Some(artifacts) = task.get("artifacts").and_then(Value::as_array) {
            let mut texts = Vec::new();
            for artifact in artifacts {
                let text = text_from_parts(parts_of(artifact)?)?;
                if !text.is_empty() {
                    texts.push(text);
                }
            }
            if !texts.is_empty() {
                return Ok(texts.join("\n\n"));
            }
        }
    }
    Ok(accumulator.artifacts.render())
}

fn compose_message_text(effective_prompt: &str, input: &str) -> String {
    if effective_prompt.is_empty() {
        input.to_owned()
    } else {
        format!(
            "Follow these instructions when responding to the user.\n\n\
             {effective_prompt}\n\nUser message:\n{input}"
        )
    }
}

fn send_message_params(message: &Value) -> Value {
    json!({
        "message": message,
        "configuration": {"acceptedOutputModes": [TEXT_PLAIN]},
    })
}

async fn await_with_cancellation<T, F>(
    future: F,
    request: &ExecutionRequest,
) -> Result<Option<T>, AgentError>
where
    F: Future<Output = Result<T, AgentError>>,
{
    tokio::pin!(future);
    loop {
        tokio::select! {
            result = &mut future => return result.map(Some),
            () = tokio::time::sleep(CANCEL_POLL_INTERVAL) => {
                if request.cancellation.is_cancel_requested().await? {
                    return Ok(None);
                }
            }
        }
    }
}

async fn sleep_with_cancellation(
    request: &ExecutionRequest,
    duration: Duration,
) -> Result<bool, AgentError> {
    let deadline = Instant::now() + duration;
    loop {
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            return Ok(false);
        }
        tokio::time::sleep(remaining.min(CANCEL_POLL_INTERVAL)).await;
        if request.cancellation.is_cancel_requested().await? {
            return Ok(true);
        }
    }
}

async fn read_body_limited(mut response: Response, limit: usize) -> Result<Vec<u8>, AgentError> {
    let mut body = Vec::new();
    loop {
        match response.chunk().await {
            Ok(Some(chunk)) => {
                if body.len().saturating_add(chunk.len()) > limit {
                    return Err(request_failed());
                }
                body.extend_from_slice(&chunk);
            }
            Ok(None) => return Ok(body),
            Err(_) => return Err(request_failed()),
        }
    }
}

async fn read_rpc_response_json(response: Response, limit: usize) -> Result<Value, AgentError> {
    let body = read_body_limited(response, limit).await?;
    parse_rpc_payload(&body)
}

fn parse_rpc_payload(body: &[u8]) -> Result<Value, AgentError> {
    let envelope: Value = serde_json::from_slice(body).map_err(|_| request_failed())?;
    rpc_result(&envelope)
}

fn rpc_result(envelope: &Value) -> Result<Value, AgentError> {
    let object = envelope.as_object().ok_or_else(request_failed)?;
    if object.get("error").is_some_and(|error| !error.is_null()) {
        return Err(request_failed());
    }
    object
        .get("result")
        .filter(|result| !result.is_null())
        .cloned()
        .ok_or_else(request_failed)
}

struct SseReader {
    response: Option<Response>,
    buffer: Vec<u8>,
    queued: Option<Vec<u8>>,
}

impl SseReader {
    async fn new(response: Response) -> Result<Self, AgentError> {
        let is_event_stream = response
            .headers()
            .get(CONTENT_TYPE)
            .and_then(|value| value.to_str().ok())
            .is_some_and(|content_type| content_type.contains("text/event-stream"));
        if is_event_stream {
            return Ok(Self {
                response: Some(response),
                buffer: Vec::new(),
                queued: None,
            });
        }
        let body = read_body_limited(response, MAX_RPC_RESPONSE_BYTES).await?;
        Ok(Self {
            response: None,
            buffer: Vec::new(),
            queued: Some(body),
        })
    }

    async fn next_event(&mut self) -> Result<Option<Value>, AgentError> {
        loop {
            if let Some(body) = self.queued.take() {
                return parse_rpc_payload(&body).map(Some);
            }
            if let Some(payload) = take_data_frame(&mut self.buffer)? {
                if payload.is_empty() {
                    continue;
                }
                return parse_rpc_payload(payload.as_bytes()).map(Some);
            }
            let Some(response) = self.response.as_mut() else {
                return Ok(None);
            };
            match response.chunk().await {
                Ok(Some(chunk)) => {
                    if self.buffer.len().saturating_add(chunk.len()) > MAX_RPC_RESPONSE_BYTES {
                        return Err(request_failed());
                    }
                    self.buffer.extend_from_slice(&chunk);
                }
                Ok(None) => self.response = None,
                Err(_) => return Err(request_failed()),
            }
        }
    }
}

fn take_data_frame(buffer: &mut Vec<u8>) -> Result<Option<String>, AgentError> {
    let Some((index, separator_len)) = find_frame_boundary(buffer) else {
        return Ok(None);
    };
    let frame_bytes = buffer
        .drain(..index.saturating_add(separator_len))
        .collect::<Vec<_>>();
    let frame_text = std::str::from_utf8(&frame_bytes[..index]).map_err(|_| request_failed())?;
    let mut data_lines = Vec::new();
    for line in frame_text.lines() {
        if line.starts_with(':') {
            continue;
        }
        if let Some(value) = line.strip_prefix("data:") {
            data_lines.push(value.strip_prefix(' ').unwrap_or(value));
        }
    }
    Ok(Some(data_lines.join("\n")))
}

fn find_frame_boundary(buffer: &[u8]) -> Option<(usize, usize)> {
    let lf = find_bytes(buffer, b"\n\n").map(|index| (index, 2));
    let crlf = find_bytes(buffer, b"\r\n\r\n").map(|index| (index, 4));
    match (lf, crlf) {
        (Some(left), Some(right)) => Some(if left.0 <= right.0 { left } else { right }),
        (Some(boundary), None) | (None, Some(boundary)) => Some(boundary),
        (None, None) => None,
    }
}

fn find_bytes(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack
        .windows(needle.len())
        .position(|window| window == needle)
}

fn parse_card_url(card_url: &str) -> Result<Url, AgentError> {
    let url = Url::parse(card_url).map_err(|_| request_failed())?;
    url_origin(&url)?;
    Ok(url)
}

fn url_origin(url: &Url) -> Result<(String, String, u16), AgentError> {
    if !matches!(url.scheme(), "http" | "https")
        || url.host_str().is_none()
        || !url.username().is_empty()
        || url.password().is_some()
    {
        return Err(request_failed());
    }
    let port = url.port_or_known_default().ok_or_else(request_failed)?;
    Ok((
        url.scheme().to_owned(),
        url.host_str().unwrap_or_default().to_ascii_lowercase(),
        port,
    ))
}

fn role_is_agent(value: Option<&Value>) -> bool {
    match value {
        Some(Value::String(role)) => role == "ROLE_AGENT",
        Some(Value::Number(number)) => number.as_i64() == Some(2),
        _ => false,
    }
}

fn task_state_value(value: Option<&Value>) -> Option<&'static str> {
    match value? {
        Value::String(name) => TASK_STATE_NAMES.iter().copied().find(|known| known == name),
        Value::Number(number) => state_name_from_number(number.as_i64()?),
        _ => None,
    }
}

fn state_name_from_number(value: i64) -> Option<&'static str> {
    TASK_STATE_NAMES.get(usize::try_from(value).ok()?).copied()
}

fn task_state_name(value: Option<&Value>) -> Result<&'static str, AgentError> {
    match task_state_value(value) {
        None | Some(TASK_STATE_UNSPECIFIED) => Err(request_failed()),
        Some(state) => Ok(state),
    }
}

fn terminal_failure_code(state: &'static str) -> &'static str {
    match state {
        TASK_STATE_INPUT_REQUIRED => "a2a_input_required",
        TASK_STATE_AUTH_REQUIRED => "a2a_auth_required",
        TASK_STATE_CANCELED => "a2a_canceled",
        TASK_STATE_REJECTED => "a2a_rejected",
        _ => "a2a_failed",
    }
}

fn text_from_message(message: &Value) -> Result<String, AgentError> {
    if !role_is_agent(message.get("role")) {
        return Ok(String::new());
    }
    text_from_parts(parts_of(message)?)
}

fn parts_of(value: &Value) -> Result<&[Value], AgentError> {
    match value.get("parts") {
        None => Ok(&[]),
        Some(Value::Array(parts)) => Ok(parts),
        Some(_) => Err(request_failed()),
    }
}

fn text_from_parts(parts: &[Value]) -> Result<String, AgentError> {
    let mut text = String::new();
    for part in parts {
        match part.get("text") {
            Some(Value::String(part_text)) => text.push_str(part_text),
            _ => return Err(request_failed()),
        }
    }
    Ok(text)
}

fn probe_unavailable(detail: &'static str) -> RuntimeProbeResult {
    RuntimeProbeResult {
        ready: false,
        status_detail: Some(detail.to_owned()),
        card_name: None,
    }
}

fn probe_detail(error: &AgentError) -> &'static str {
    match error.code() {
        "a2a_authentication_required" => "Agent Card 要求认证；请为该 Runtime 配置 Bearer 凭据。",
        "a2a_credential_unavailable" => "凭据别名对应的密钥未配置。",
        _ => "Agent Card 无法读取或不符合 A2A 1.0 JSON-RPC 要求。",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn card(document: Value) -> A2aCard {
        serde_json::from_value(document).unwrap()
    }

    fn base_card() -> Value {
        json!({
            "name": "Remote Agent",
            "version": "1.0.0",
            "supportedInterfaces": [{
                "url": "https://agent.example.com/a2a",
                "protocolBinding": "JSONRPC",
                "protocolVersion": "1.0.0"
            }],
            "capabilities": {"streaming": true},
            "defaultInputModes": ["text/plain"],
            "defaultOutputModes": ["text/plain"],
            "skills": []
        })
    }

    #[test]
    fn card_requires_text_input_and_output_modes() {
        let card_url = Url::parse("https://agent.example.com/card").unwrap();
        let mut document = base_card();
        document["defaultOutputModes"] = json!(["application/json"]);
        assert_eq!(
            card(document).into_loaded(&card_url, false),
            Err(request_failed())
        );
    }

    #[test]
    fn card_selects_first_same_origin_jsonrpc_interface() {
        let card_url = Url::parse("https://agent.example.com/card").unwrap();
        let mut document = base_card();
        document["supportedInterfaces"] = json!([
            {"url": "https://other.example.com/a2a", "protocolBinding": "JSONRPC", "protocolVersion": "1.0"},
            {"url": "https://agent.example.com/a2a-http", "protocolBinding": "HTTP+JSON", "protocolVersion": "1.0"},
            {"url": "https://agent.example.com/a2a", "protocolBinding": "JSONRPC", "protocolVersion": "1.0.0"}
        ]);
        let loaded = card(document).into_loaded(&card_url, false).unwrap();
        assert_eq!(loaded.endpoint.as_str(), "https://agent.example.com/a2a");
        assert!(loaded.streaming);
    }

    #[test]
    fn card_rejects_missing_jsonrpc_interface() {
        let card_url = Url::parse("https://agent.example.com/card").unwrap();
        let mut document = base_card();
        document["supportedInterfaces"] = json!([
            {"url": "https://agent.example.com/a2a", "protocolBinding": "HTTP+JSON", "protocolVersion": "1.0"}
        ]);
        assert_eq!(
            card(document).into_loaded(&card_url, false),
            Err(request_failed())
        );
    }

    #[test]
    fn bearer_security_requires_a_token_and_bearer_scheme() {
        let card_url = Url::parse("https://agent.example.com/card").unwrap();
        let mut document = base_card();
        document["securitySchemes"] = json!({
            "bearer": {"httpAuthSecurityScheme": {"scheme": "bearer"}}
        });
        document["securityRequirements"] = json!([{"schemes": {"bearer": {"list": []}}}]);
        let secured = card(document.clone());
        assert_eq!(
            secured.into_loaded(&card_url, false),
            Err(authentication_required())
        );
        assert!(card(document).into_loaded(&card_url, true).is_ok());

        let mut unsupported = base_card();
        unsupported["securitySchemes"] = json!({
            "key": {"apiKeySecurityScheme": {"location": "header", "name": "X-Key"}}
        });
        unsupported["securityRequirements"] = json!([{"schemes": {"key": {"list": []}}}]);
        assert_eq!(
            card(unsupported).into_loaded(&card_url, true),
            Err(request_failed())
        );
    }

    #[test]
    fn skill_security_requirements_are_enforced() {
        let card_url = Url::parse("https://agent.example.com/card").unwrap();
        let mut document = base_card();
        document["securitySchemes"] = json!({
            "bearer": {"httpAuthSecurityScheme": {"scheme": "bearer"}}
        });
        document["skills"] = json!([{
            "id": "s1",
            "name": "s1",
            "description": "s1",
            "tags": [],
            "securityRequirements": [{"schemes": {"bearer": {"list": []}}}]
        }]);
        assert_eq!(
            card(document).into_loaded(&card_url, false),
            Err(authentication_required())
        );
    }

    #[test]
    fn message_text_uses_the_source_template() {
        assert_eq!(compose_message_text("", "hello"), "hello");
        assert_eq!(
            compose_message_text("Be brief.", "hello"),
            "Follow these instructions when responding to the user.\n\nBe brief.\n\nUser message:\nhello"
        );
    }

    #[test]
    fn artifact_updates_append_to_the_same_artifact() {
        let mut artifacts = ArtifactAccumulator::default();
        artifacts
            .apply(
                &json!({"artifactId": "a1", "parts": [{"text": "hello"}]}),
                false,
            )
            .unwrap();
        artifacts
            .apply(
                &json!({"artifactId": "a1", "parts": [{"text": " world"}]}),
                true,
            )
            .unwrap();
        artifacts
            .apply(
                &json!({"artifactId": "a2", "parts": [{"text": "second"}]}),
                false,
            )
            .unwrap();
        assert_eq!(artifacts.render(), "hello world\n\nsecond");

        let mut unknown_append = ArtifactAccumulator::default();
        assert_eq!(
            unknown_append.apply(&json!({"artifactId": "a1", "parts": [{"text": "x"}]}), true),
            Err(request_failed())
        );
        assert_eq!(
            artifacts.apply(&json!({"parts": [{"text": "x"}]}), false),
            Err(request_failed())
        );
    }

    #[test]
    fn non_text_parts_are_rejected() {
        assert_eq!(
            text_from_parts(&[json!({"raw": "aGVsbG8="})]),
            Err(request_failed())
        );
    }

    #[test]
    fn direct_message_completes_even_when_the_message_carries_a_task_id() {
        let accumulator = RunAccumulator {
            task_id: Some("t1".to_owned()),
            direct_message_received: true,
            output: "complete answer".to_owned(),
            ..RunAccumulator::default()
        };
        assert_eq!(
            accumulator.completion_plan(),
            CompletionPlan::DirectMessage,
            "A2A 1.0 允许 SendMessage 直接返回 Message；其 taskId 只是关联标识，不能据此再轮询"
        );
    }

    #[test]
    fn non_terminal_task_requires_polling() {
        let accumulator = RunAccumulator {
            task_id: Some("t1".to_owned()),
            state: Some(TASK_STATE_WORKING),
            ..RunAccumulator::default()
        };
        assert_eq!(accumulator.completion_plan(), CompletionPlan::PollTask);

        let terminal = RunAccumulator {
            state: Some(TASK_STATE_COMPLETED),
            ..RunAccumulator::default()
        };
        assert_eq!(terminal.completion_plan(), CompletionPlan::Complete);

        assert_eq!(
            RunAccumulator::default().completion_plan(),
            CompletionPlan::Incomplete
        );
    }

    #[test]
    fn final_text_prefers_status_message_then_history_then_artifacts() {
        let mut accumulator = RunAccumulator {
            latest_status: Some(json!({
                "state": "TASK_STATE_COMPLETED",
                "message": {"role": "ROLE_AGENT", "parts": [{"text": "status answer"}]}
            })),
            latest_task: Some(json!({
                "id": "t1",
                "status": {"state": "TASK_STATE_COMPLETED"},
                "history": [{"role": "ROLE_AGENT", "parts": [{"text": "history answer"}]}],
                "artifacts": [{"artifactId": "a1", "parts": [{"text": "artifact answer"}]}]
            })),
            ..RunAccumulator::default()
        };
        assert_eq!(final_task_text(&accumulator).unwrap(), "status answer");

        accumulator.latest_status = Some(json!({"state": "TASK_STATE_COMPLETED"}));
        assert_eq!(final_task_text(&accumulator).unwrap(), "history answer");

        accumulator.latest_task = Some(json!({
            "id": "t1",
            "status": {"state": "TASK_STATE_COMPLETED"},
            "history": [],
            "artifacts": [
                {"artifactId": "a1", "parts": [{"text": "first"}]},
                {"artifactId": "a2", "parts": [{"text": "second"}]}
            ]
        }));
        assert_eq!(final_task_text(&accumulator).unwrap(), "first\n\nsecond");
    }

    #[test]
    fn streamed_artifacts_are_used_when_the_task_has_no_final_text() {
        let mut accumulator = RunAccumulator::default();
        accumulator
            .artifacts
            .apply(
                &json!({"artifactId": "a1", "parts": [{"text": "streamed"}]}),
                false,
            )
            .unwrap();
        assert_eq!(final_task_text(&accumulator).unwrap(), "streamed");
    }

    #[test]
    fn task_states_accept_names_and_numbers() {
        assert_eq!(
            task_state_name(Some(&json!("TASK_STATE_COMPLETED"))).unwrap(),
            TASK_STATE_COMPLETED
        );
        assert_eq!(
            task_state_name(Some(&json!(5))).unwrap(),
            TASK_STATE_CANCELED
        );
        assert_eq!(task_state_name(Some(&json!(0))), Err(request_failed()));
        assert_eq!(task_state_name(Some(&json!(99))), Err(request_failed()));
        assert_eq!(
            task_state_name(Some(&json!("TASK_STATE_BOGUS"))),
            Err(request_failed())
        );
        assert_eq!(task_state_name(None), Err(request_failed()));
    }

    #[test]
    fn terminal_failure_codes_are_stable() {
        assert_eq!(
            terminal_failure_code(TASK_STATE_INPUT_REQUIRED),
            "a2a_input_required"
        );
        assert_eq!(
            terminal_failure_code(TASK_STATE_AUTH_REQUIRED),
            "a2a_auth_required"
        );
        assert_eq!(terminal_failure_code(TASK_STATE_CANCELED), "a2a_canceled");
        assert_eq!(terminal_failure_code(TASK_STATE_REJECTED), "a2a_rejected");
        assert_eq!(terminal_failure_code(TASK_STATE_FAILED), "a2a_failed");
        for state in [
            TASK_STATE_COMPLETED,
            TASK_STATE_FAILED,
            TASK_STATE_CANCELED,
            TASK_STATE_REJECTED,
        ] {
            assert!(TASK_STATES_WITHOUT_CONTINUATION.contains(&state));
        }
    }

    #[test]
    fn rpc_envelopes_require_a_result_and_reject_errors() {
        assert_eq!(
            rpc_result(&json!({"jsonrpc": "2.0", "result": {"task": {"id": "t1"}}})),
            Ok(json!({"task": {"id": "t1"}}))
        );
        assert_eq!(
            rpc_result(&json!({"jsonrpc": "2.0", "error": {"code": -32603, "message": "boom"}})),
            Err(request_failed())
        );
        assert_eq!(
            rpc_result(&json!({"jsonrpc": "2.0"})),
            Err(request_failed())
        );
        assert_eq!(rpc_result(&json!([1, 2])), Err(request_failed()));
    }

    #[test]
    fn sse_frames_are_extracted_with_multiline_data() {
        let mut buffer = Vec::new();
        buffer.extend_from_slice(b": comment\nevent: message\ndata: {\"a\": 1}\n\n");
        assert_eq!(
            take_data_frame(&mut buffer).unwrap(),
            Some("{\"a\": 1}".to_owned())
        );
        assert_eq!(take_data_frame(&mut buffer).unwrap(), None);

        let mut crlf = Vec::new();
        crlf.extend_from_slice(b"data: first\r\ndata: second\r\n\r\n");
        assert_eq!(
            take_data_frame(&mut crlf).unwrap(),
            Some("first\nsecond".to_owned())
        );
    }

    #[test]
    fn partial_sse_frames_wait_for_more_bytes() {
        let mut buffer = b"data: {\"task\":".to_vec();
        assert_eq!(take_data_frame(&mut buffer).unwrap(), None);
        buffer.extend_from_slice(b"{\"id\": \"t1\"}}\n\n");
        assert_eq!(
            take_data_frame(&mut buffer).unwrap(),
            Some("{\"task\":{\"id\": \"t1\"}}".to_owned())
        );
    }
}

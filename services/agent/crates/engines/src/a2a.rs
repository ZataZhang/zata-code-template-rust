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

use std::sync::Arc;
use std::time::{Duration, Instant};

mod card;
mod protocol;

use card::{A2aCard, LoadedCard, parse_card_url};
#[cfg(test)]
use protocol::{
    ArtifactAccumulator, final_task_text, rpc_result, take_data_frame, task_state_name,
    text_from_parts,
};
use protocol::{
    CompletionPlan, RunAccumulator, SseReader, await_with_cancellation, complete_text,
    compose_message_text, probe_detail, probe_unavailable, read_body_limited,
    read_rpc_response_json, send_message_params, sleep_with_cancellation, task_state_value,
    terminal_failure_code,
};

use async_trait::async_trait;
use reqwest::header::{ACCEPT, CONTENT_TYPE};
use reqwest::{Client, Response, Url};
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

#[cfg(test)]
mod tests;

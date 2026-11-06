//! A2A JSON-RPC、任务状态、流式事件与文本组装。

use std::future::Future;
use std::time::Instant;

use super::*;

#[derive(Default)]
pub(super) struct RunAccumulator {
    pub(super) task_id: Option<String>,
    pub(super) latest_task: Option<Value>,
    pub(super) latest_status: Option<Value>,
    pub(super) state: Option<&'static str>,
    pub(super) output: String,
    pub(super) artifacts: ArtifactAccumulator,
    pub(super) direct_message_received: bool,
}

/// 一次 A2A 调用结束后如何收口。
#[derive(Debug, PartialEq, Eq)]
pub(super) enum CompletionPlan {
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
    pub(super) fn completion_plan(&self) -> CompletionPlan {
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

    pub(super) async fn apply(
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

    pub(super) fn apply_task(&mut self, task: &Value) -> Result<(), AgentError> {
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

    pub(super) async fn apply_message(
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

    pub(super) async fn apply_status_update(
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

    pub(super) fn apply_artifact_update(&mut self, update: &Value) -> Result<(), AgentError> {
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

    pub(super) async fn append_text(
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

    pub(super) fn is_terminal(&self) -> bool {
        self.state
            .is_some_and(|state| TASK_STATES_WITHOUT_CONTINUATION.contains(&state))
    }
}

#[derive(Default)]
pub(super) struct ArtifactAccumulator {
    entries: Vec<(String, String)>,
}

impl ArtifactAccumulator {
    pub(super) fn apply(&mut self, artifact: &Value, append: bool) -> Result<(), AgentError> {
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

    pub(super) fn render(&self) -> String {
        self.entries
            .iter()
            .map(|(_, text)| text.as_str())
            .filter(|text| !text.is_empty())
            .collect::<Vec<_>>()
            .join("\n\n")
    }
}

pub(super) async fn complete_text(
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

pub(super) fn final_task_text(accumulator: &RunAccumulator) -> Result<String, AgentError> {
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

pub(super) fn compose_message_text(effective_prompt: &str, input: &str) -> String {
    if effective_prompt.is_empty() {
        input.to_owned()
    } else {
        format!(
            "Follow these instructions when responding to the user.\n\n\
             {effective_prompt}\n\nUser message:\n{input}"
        )
    }
}

pub(super) fn send_message_params(message: &Value) -> Value {
    json!({
        "message": message,
        "configuration": {"acceptedOutputModes": [TEXT_PLAIN]},
    })
}

pub(super) async fn await_with_cancellation<T, F>(
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

pub(super) async fn sleep_with_cancellation(
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

pub(super) async fn read_body_limited(
    mut response: Response,
    limit: usize,
) -> Result<Vec<u8>, AgentError> {
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

pub(super) async fn read_rpc_response_json(
    response: Response,
    limit: usize,
) -> Result<Value, AgentError> {
    let body = read_body_limited(response, limit).await?;
    parse_rpc_payload(&body)
}

pub(super) fn parse_rpc_payload(body: &[u8]) -> Result<Value, AgentError> {
    let envelope: Value = serde_json::from_slice(body).map_err(|_| request_failed())?;
    rpc_result(&envelope)
}

pub(super) fn rpc_result(envelope: &Value) -> Result<Value, AgentError> {
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

pub(super) struct SseReader {
    response: Option<Response>,
    buffer: Vec<u8>,
    queued: Option<Vec<u8>>,
}

impl SseReader {
    pub(super) async fn new(response: Response) -> Result<Self, AgentError> {
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

    pub(super) async fn next_event(&mut self) -> Result<Option<Value>, AgentError> {
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

pub(super) fn take_data_frame(buffer: &mut Vec<u8>) -> Result<Option<String>, AgentError> {
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

pub(super) fn find_frame_boundary(buffer: &[u8]) -> Option<(usize, usize)> {
    let lf = find_bytes(buffer, b"\n\n").map(|index| (index, 2));
    let crlf = find_bytes(buffer, b"\r\n\r\n").map(|index| (index, 4));
    match (lf, crlf) {
        (Some(left), Some(right)) => Some(if left.0 <= right.0 { left } else { right }),
        (Some(boundary), None) | (None, Some(boundary)) => Some(boundary),
        (None, None) => None,
    }
}

pub(super) fn find_bytes(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack
        .windows(needle.len())
        .position(|window| window == needle)
}

pub(super) fn role_is_agent(value: Option<&Value>) -> bool {
    match value {
        Some(Value::String(role)) => role == "ROLE_AGENT",
        Some(Value::Number(number)) => number.as_i64() == Some(2),
        _ => false,
    }
}

pub(super) fn task_state_value(value: Option<&Value>) -> Option<&'static str> {
    match value? {
        Value::String(name) => TASK_STATE_NAMES.iter().copied().find(|known| known == name),
        Value::Number(number) => state_name_from_number(number.as_i64()?),
        _ => None,
    }
}

pub(super) fn state_name_from_number(value: i64) -> Option<&'static str> {
    TASK_STATE_NAMES.get(usize::try_from(value).ok()?).copied()
}

pub(super) fn task_state_name(value: Option<&Value>) -> Result<&'static str, AgentError> {
    match task_state_value(value) {
        None | Some(TASK_STATE_UNSPECIFIED) => Err(request_failed()),
        Some(state) => Ok(state),
    }
}

pub(super) fn terminal_failure_code(state: &'static str) -> &'static str {
    match state {
        TASK_STATE_INPUT_REQUIRED => "a2a_input_required",
        TASK_STATE_AUTH_REQUIRED => "a2a_auth_required",
        TASK_STATE_CANCELED => "a2a_canceled",
        TASK_STATE_REJECTED => "a2a_rejected",
        _ => "a2a_failed",
    }
}

pub(super) fn text_from_message(message: &Value) -> Result<String, AgentError> {
    if !role_is_agent(message.get("role")) {
        return Ok(String::new());
    }
    text_from_parts(parts_of(message)?)
}

pub(super) fn parts_of(value: &Value) -> Result<&[Value], AgentError> {
    match value.get("parts") {
        None => Ok(&[]),
        Some(Value::Array(parts)) => Ok(parts),
        Some(_) => Err(request_failed()),
    }
}

pub(super) fn text_from_parts(parts: &[Value]) -> Result<String, AgentError> {
    let mut text = String::new();
    for part in parts {
        match part.get("text") {
            Some(Value::String(part_text)) => text.push_str(part_text),
            _ => return Err(request_failed()),
        }
    }
    Ok(text)
}

pub(super) fn probe_unavailable(detail: &'static str) -> RuntimeProbeResult {
    RuntimeProbeResult {
        ready: false,
        status_detail: Some(detail.to_owned()),
        card_name: None,
    }
}

pub(super) fn probe_detail(error: &AgentError) -> &'static str {
    match error.code() {
        "a2a_authentication_required" => "Agent Card 要求认证；请为该 Runtime 配置 Bearer 凭据。",
        "a2a_credential_unavailable" => "凭据别名对应的密钥未配置。",
        _ => "Agent Card 无法读取或不符合 A2A 1.0 JSON-RPC 要求。",
    }
}

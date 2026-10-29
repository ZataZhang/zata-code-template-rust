//! 可选 OTLP/HTTP JSON exporter，只发送诊断元数据，不发送 canonical 正文。

use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use reqwest::StatusCode;
use serde_json::{Value, json};
use time::OffsetDateTime;
use tpl_agent_tracing_core::model::{
    AppendResult, RunStatus, SpanKind, SpanStatus, TraceDetail, TraceSpan,
};
use tpl_agent_tracing_core::ports::{OtlpExportError, OtlpExporter};
use tpl_agent_tracing_core::projection::project_run_trace;

use crate::config::OtlpSettings;

const MAX_OTLP_RESPONSE_BYTES: usize = 8 * 1024;
const MAX_OTLP_REQUEST_BYTES: usize = 4 * 1024 * 1024;
const INSTRUMENTATION_SCOPE: &str = "backend.agent_run_tracing";

/// OTLP exporter 配置无效。
#[derive(Debug, thiserror::Error)]
#[error("OTLP exporter 配置无效")]
pub struct OtlpConfigError;

/// 已配置 OTLP/HTTP traces 接收端的最佳努力 exporter。
pub struct HttpOtlpExporter {
    client: reqwest::Client,
    endpoint: String,
    service_name: String,
    service_version: String,
    environment: String,
}

impl HttpOtlpExporter {
    /// 构造最终地址、禁止重定向且具有请求超时的 OTLP 客户端。
    ///
    /// # Errors
    ///
    /// endpoint 不是 HTTP(S) URL 或 HTTP 客户端无法配置时返回错误。
    pub fn new(settings: &OtlpSettings) -> Result<Option<Self>, OtlpConfigError> {
        let Some(endpoint) = settings
            .traces_endpoint
            .as_deref()
            .map(str::trim)
            .filter(|endpoint| !endpoint.is_empty())
        else {
            return Ok(None);
        };
        let parsed_endpoint = reqwest::Url::parse(endpoint).map_err(|_url_error| {
            tracing::error!("OTLP traces endpoint 配置无效");
            OtlpConfigError
        })?;
        if !matches!(parsed_endpoint.scheme(), "http" | "https") {
            tracing::error!("OTLP traces endpoint 必须使用 HTTP 或 HTTPS");
            return Err(OtlpConfigError);
        }
        let client = reqwest::Client::builder()
            .timeout(Duration::from_secs(settings.timeout_secs.clamp(1, 30)))
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .map_err(|_client_error| OtlpConfigError)?;
        Ok(Some(Self {
            client,
            endpoint: parsed_endpoint.to_string(),
            service_name: settings.service_name.clone(),
            service_version: settings.service_version.clone(),
            environment: settings.environment.clone(),
        }))
    }
}

#[async_trait]
impl OtlpExporter for HttpOtlpExporter {
    async fn export_committed_events(
        &self,
        append_result: &AppendResult,
    ) -> Result<(), OtlpExportError> {
        if !append_result.current_status.is_terminal() || append_result.run_events.is_empty() {
            return Ok(());
        }
        let trace_detail = project_run_trace(&append_result.run_record, &append_result.run_events);
        let exportable_spans = trace_detail
            .spans
            .iter()
            .filter(|span| matches!(span.kind, SpanKind::Root | SpanKind::Model | SpanKind::Tool))
            .map(|span| otlp_span(span, &trace_detail, &append_result.run_record.status))
            .collect::<Vec<_>>();
        if exportable_spans.is_empty() {
            return Ok(());
        }
        let request_body = otlp_request(
            exportable_spans,
            &self.service_name,
            &self.service_version,
            &self.environment,
        );
        let encoded_request = serde_json::to_vec(&request_body).map_err(|_| OtlpExportError)?;
        if encoded_request.len() > MAX_OTLP_REQUEST_BYTES {
            return Err(OtlpExportError);
        }
        let mut response = self
            .client
            .post(&self.endpoint)
            .header(reqwest::header::CONTENT_TYPE, "application/json")
            .body(encoded_request)
            .send()
            .await
            .map_err(|_| OtlpExportError)?;
        if response.status() != StatusCode::OK
            || response
                .content_length()
                .is_some_and(|response_bytes| response_bytes > MAX_OTLP_RESPONSE_BYTES as u64)
        {
            return Err(OtlpExportError);
        }
        let mut response_body = Vec::new();
        while let Some(response_chunk) = response.chunk().await.map_err(|_| OtlpExportError)? {
            if response_body.len().saturating_add(response_chunk.len()) > MAX_OTLP_RESPONSE_BYTES {
                return Err(OtlpExportError);
            }
            response_body.extend_from_slice(&response_chunk);
        }
        let response_json =
            serde_json::from_slice::<Value>(&response_body).map_err(|_| OtlpExportError)?;
        let rejected_span_count = response_json
            .get("partialSuccess")
            .and_then(|partial_success| partial_success.get("rejectedSpans"))
            .and_then(parse_json_u64)
            .unwrap_or_default();
        if rejected_span_count > 0 {
            return Err(OtlpExportError);
        }
        Ok(())
    }
}

/// 明确关闭 OTLP 时使用的空出口，不执行任何网络请求。
pub struct DisabledOtlpExporter;

#[async_trait]
impl OtlpExporter for DisabledOtlpExporter {
    async fn export_committed_events(
        &self,
        _append_result: &AppendResult,
    ) -> Result<(), OtlpExportError> {
        Ok(())
    }
}

/// 按配置返回启用或关闭状态的共享 exporter。
pub fn build_exporter(settings: &OtlpSettings) -> Result<Arc<dyn OtlpExporter>, OtlpConfigError> {
    match HttpOtlpExporter::new(settings)? {
        Some(exporter) => Ok(Arc::new(exporter)),
        None => Ok(Arc::new(DisabledOtlpExporter)),
    }
}

fn otlp_request(
    spans: Vec<Value>,
    service_name: &str,
    service_version: &str,
    environment: &str,
) -> Value {
    json!({
        "resourceSpans": [{
            "resource": {
                "attributes": [
                    string_attribute("service.name", service_name),
                    string_attribute("service.version", service_version),
                    string_attribute("deployment.environment", environment),
                ]
            },
            "scopeSpans": [{
                "scope": {"name": INSTRUMENTATION_SCOPE, "version": service_version},
                "spans": spans,
            }]
        }]
    })
}

fn otlp_span(span: &TraceSpan, detail: &TraceDetail, run_status: &RunStatus) -> Value {
    let started_at = span.started_at.unwrap_or(
        detail
            .summary
            .started_at
            .unwrap_or(detail.summary.created_at),
    );
    let finished_at = span
        .finished_at
        .or(detail.summary.finished_at)
        .unwrap_or(started_at);
    let mut attributes = match span.kind {
        SpanKind::Root => vec![
            string_attribute("agent.run.id", &detail.summary.run_id),
            string_attribute("agent.id", &detail.summary.agent_id),
            string_attribute("agent.run.status", run_status.as_str()),
        ],
        SpanKind::Model => model_attributes(span),
        SpanKind::Tool => tool_attributes(span, detail),
        SpanKind::Artifact | SpanKind::Decision => Vec::new(),
    };
    if let Some(error_code) = span
        .error_code
        .as_deref()
        .filter(|code| is_safe_error_code(code))
    {
        attributes.push(string_attribute("agent.error.code", error_code));
    }
    let mut span_document = json!({
        "traceId": &detail.summary.trace_id,
        "spanId": &span.span_id,
        "name": span_name(span.kind),
        "kind": 1,
        "startTimeUnixNano": unix_nanos(started_at),
        "endTimeUnixNano": unix_nanos(finished_at),
        "attributes": attributes,
        "status": {"code": otlp_status_code(span.status)},
    });
    if let Some(parent_span_id) = &span.parent_span_id {
        span_document["parentSpanId"] = json!(parent_span_id);
    }
    span_document
}

fn model_attributes(span: &TraceSpan) -> Vec<Value> {
    let mut attributes = vec![string_attribute("gen_ai.operation.name", "chat")];
    if let Some(model_name) = &span.model_name {
        attributes.push(string_attribute("gen_ai.request.model", model_name));
    }
    if let Some(model_call_id) = &span.model_call_id {
        attributes.push(string_attribute("agent.model.call_id", model_call_id));
    }
    if let Some(turn_index) = span.turn_index {
        attributes.push(integer_attribute("agent.model.turn_index", turn_index));
    }
    if let Some(input_count) = span.usage.as_ref().and_then(|usage| usage.get("prompt"))
        && let Ok(input_count) = i64::try_from(*input_count)
    {
        attributes.push(integer_attribute("gen_ai.usage.input_tokens", input_count));
    }
    if let Some(output_count) = span
        .usage
        .as_ref()
        .and_then(|usage| usage.get("completion"))
        && let Ok(output_count) = i64::try_from(*output_count)
    {
        attributes.push(integer_attribute(
            "gen_ai.usage.output_tokens",
            output_count,
        ));
    }
    if let Some(finish_reason) = span.finish_reason.as_deref() {
        attributes.push(string_array_attribute(
            "gen_ai.response.finish_reasons",
            &[finish_reason],
        ));
    }
    attributes
}

fn tool_attributes(span: &TraceSpan, detail: &TraceDetail) -> Vec<Value> {
    let mut attributes = Vec::new();
    if let Some(tool_name) = &span.tool_name {
        attributes.push(string_attribute("gen_ai.tool.name", tool_name));
    }
    if let Some(tool_call_id) = &span.tool_call_id {
        attributes.push(string_attribute("gen_ai.tool.call.id", tool_call_id));
    }
    let parent_is_model = span.parent_span_id.as_deref().is_some_and(|parent_id| {
        detail.spans.iter().any(|candidate| {
            candidate.span_id == parent_id && matches!(candidate.kind, SpanKind::Model)
        })
    });
    attributes.push(string_attribute(
        "agent.tool.parent_source",
        if parent_is_model {
            "model_call"
        } else {
            "run_fallback"
        },
    ));
    attributes
}

fn span_name(span_kind: SpanKind) -> &'static str {
    match span_kind {
        SpanKind::Root => "agent.run",
        SpanKind::Model => "gen_ai.chat",
        SpanKind::Tool => "execute_tool",
        SpanKind::Artifact => "create_artifact",
        SpanKind::Decision => "evaluate_decision",
    }
}

fn otlp_status_code(span_status: SpanStatus) -> u8 {
    match span_status {
        SpanStatus::Running | SpanStatus::Incomplete => 0,
        SpanStatus::Completed => 1,
        SpanStatus::Failed => 2,
    }
}

fn unix_nanos(moment: OffsetDateTime) -> String {
    moment.unix_timestamp_nanos().max(0).to_string()
}

fn string_attribute(attribute_key: &str, string_value: &str) -> Value {
    json!({"key": attribute_key, "value": {"stringValue": string_value}})
}

fn integer_attribute(attribute_key: &str, integer_value: i64) -> Value {
    json!({"key": attribute_key, "value": {"intValue": integer_value.to_string()}})
}

fn string_array_attribute(attribute_key: &str, strings: &[&str]) -> Value {
    json!({
        "key": attribute_key,
        "value": {"arrayValue": {"values": strings.iter().map(|value| json!({"stringValue": value})).collect::<Vec<_>>()}}
    })
}

fn is_safe_error_code(error_code: &str) -> bool {
    !error_code.is_empty()
        && error_code.len() <= 64
        && error_code
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-' | b'.'))
}

fn parse_json_u64(value: &Value) -> Option<u64> {
    value
        .as_u64()
        .or_else(|| value.as_str().and_then(|text| text.parse::<u64>().ok()))
}

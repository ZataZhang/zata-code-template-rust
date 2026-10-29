//! 从已提交事件恢复稳定的 Run 树、摘要、诊断和 thread 账本。

mod summary;
mod thread;

pub use summary::{project_run_summary, summarize_run_events};
pub use thread::project_thread_trace;

use std::collections::{BTreeMap, BTreeSet};

use serde_json::Value;
use sha2::{Digest, Sha256};
use time::OffsetDateTime;

use crate::model::{
    DecisionPayload, RunRecord, RunStatus, SpanKind, SpanStatus, StoredEvent, TraceDetail,
    TraceDiagnostic, TraceSpan, TraceTreeNode,
};

const CONTENT_POLICY: &str = "canonical-event-content";
const ROOT_NAME: &str = "RUN ROOT";

/// 从 Run 标识稳定派生 32 位 trace ID。
pub fn derive_trace_id(run_id: &str) -> String {
    derive_identifier("trace", run_id, 32)
}

/// 从 Run 标识稳定派生根 span ID。
pub fn derive_root_span_id(run_id: &str) -> String {
    derive_identifier("run-root-span", run_id, 16)
}

/// 从 Run、节点类别和来源标识稳定派生 span ID。
pub fn derive_span_id(run_id: &str, span_kind: &str, source_id: &str) -> String {
    derive_identifier(
        &format!("span:{span_kind}"),
        &format!("{run_id}:{source_id}"),
        16,
    )
}

/// 将 Run 事件投影为管理端详情；任何父子关系只使用事件中明确给出的标识。
pub fn project_run_trace(run: &RunRecord, events: &[StoredEvent]) -> TraceDetail {
    let trace_id = events
        .iter()
        .find_map(|event| event.trace_id.clone())
        .unwrap_or_else(|| derive_trace_id(&run.run_id));
    let root_span_id = events
        .iter()
        .find(|event| event.event_type == "run.started")
        .and_then(|event| event.span_id.clone())
        .unwrap_or_else(|| derive_root_span_id(&run.run_id));
    let has_step_events = events.iter().any(|event| {
        matches!(
            event.event_type.as_str(),
            "model.call.started"
                | "model.call.completed"
                | "model.call.failed"
                | "routing.router_invoked"
                | "tool.call.started"
                | "tool.call.completed"
                | "tool.call.failed"
                | "artifact.created"
                | "decision.gate.evaluated"
        )
    });
    let run_summary = project_run_summary(run, &trace_id, &summarize_run_events(events));
    if !has_step_events {
        return TraceDetail {
            summary: run_summary,
            root_span_id: None,
            spans: Vec::new(),
            tree: Vec::new(),
            diagnostics: vec![diagnostic("no_tracing_events", None)],
            content_policy: CONTENT_POLICY.to_owned(),
        };
    }

    let mut span_builders = BTreeMap::<String, SpanBuilder>::new();
    let mut root_builder = SpanBuilder::new(SpanInit {
        span_id: root_span_id.clone(),
        kind: SpanKind::Root,
        name: ROOT_NAME,
        event_seq: events
            .iter()
            .find(|event| event.event_type == "run.started")
            .map_or(0, |event| event.seq),
        started_at: run.started_at.or(Some(run.created_at)),
    });
    root_builder.is_root = true;
    span_builders.insert(root_span_id.clone(), root_builder);

    for event in events {
        match event.event_type.as_str() {
            "model.call.started"
            | "model.call.completed"
            | "model.call.failed"
            | "routing.router_invoked" => apply_model_event(&mut span_builders, event, &run.run_id),
            "tool.call.started" | "tool.call.completed" | "tool.call.failed" => {
                apply_tool_event(&mut span_builders, event, &run.run_id)
            }
            "artifact.created" => apply_artifact_event(&mut span_builders, event, &run.run_id),
            "decision.gate.evaluated" => {
                apply_decision_event(&mut span_builders, event, &run.run_id)
            }
            "run.completed" | "run.failed" | "run.cancelled" | "run.interrupted" => {
                if let Some(root_builder) = span_builders.get_mut(&root_span_id) {
                    let status = root_status_for_event(&event.event_type);
                    root_builder.mark_terminal(status, event.occurred_at);
                }
            }
            _ => {}
        }
    }

    resolve_parents(&mut span_builders, &root_span_id);
    settle_open_spans(&mut span_builders, run.status);
    let mut spans = span_builders
        .into_values()
        .map(SpanBuilder::finish)
        .collect::<Vec<_>>();
    spans.sort_by(|left, right| {
        left.started_at
            .cmp(&right.started_at)
            .then_with(|| left.event_seq.cmp(&right.event_seq))
            .then_with(|| left.span_id.cmp(&right.span_id))
    });
    let mut diagnostics = collect_diagnostics(&spans);
    let tree = build_tree(&spans, &root_span_id);
    diagnostics.sort_by(|left, right| {
        left.code
            .cmp(&right.code)
            .then_with(|| left.span_id.cmp(&right.span_id))
    });
    TraceDetail {
        summary: run_summary,
        root_span_id: Some(root_span_id),
        spans,
        tree,
        diagnostics,
        content_policy: CONTENT_POLICY.to_owned(),
    }
}

#[derive(Clone)]
struct SpanBuilder {
    span_id: String,
    parent_hint: Option<String>,
    parent_model_call_id: Option<String>,
    tool_call_ref: Option<String>,
    is_root: bool,
    terminal_seen: bool,
    span: TraceSpan,
}

struct SpanInit<'a> {
    span_id: String,
    kind: SpanKind,
    name: &'a str,
    event_seq: u64,
    started_at: Option<OffsetDateTime>,
}

impl SpanBuilder {
    fn new(initial: SpanInit<'_>) -> Self {
        let SpanInit {
            span_id,
            kind,
            name,
            event_seq,
            started_at,
        } = initial;
        Self {
            span: TraceSpan {
                span_id: span_id.clone(),
                parent_span_id: None,
                kind,
                name: name.to_owned(),
                status: SpanStatus::Running,
                started_at,
                finished_at: None,
                duration_ms: None,
                event_seq,
                model_call_id: None,
                model_name: None,
                turn_index: None,
                usage: None,
                finish_reason: None,
                tool_call_id: None,
                tool_name: None,
                tool_arguments: None,
                tool_result: None,
                result_checksum: None,
                result_length: None,
                error_code: None,
                error_message: None,
                diagnostics: Vec::new(),
                decision: None,
            },
            span_id,
            parent_hint: None,
            parent_model_call_id: None,
            tool_call_ref: None,
            is_root: false,
            terminal_seen: false,
        }
    }

    fn add_diagnostic(&mut self, diagnostic_code: &str) {
        if !self
            .span
            .diagnostics
            .iter()
            .any(|existing| existing == diagnostic_code)
        {
            self.span.diagnostics.push(diagnostic_code.to_owned());
        }
    }

    fn mark_terminal(&mut self, status: SpanStatus, occurred_at: OffsetDateTime) {
        if self.terminal_seen {
            self.add_diagnostic("duplicate_completion");
            return;
        }
        self.terminal_seen = true;
        self.span.status = status;
        self.span.finished_at = Some(occurred_at);
    }

    fn finish(mut self) -> TraceSpan {
        self.span.duration_ms = elapsed_ms(self.span.started_at, self.span.finished_at);
        self.span.diagnostics.sort();
        self.span
    }
}

fn apply_model_event(
    builders: &mut BTreeMap<String, SpanBuilder>,
    event: &StoredEvent,
    run_id: &str,
) {
    let model_call_id = payload_text(&event.payload, "model_call_id")
        .unwrap_or_else(|| format!("event-{}", event.seq));
    let existing_span_id = builders
        .values()
        .find(|builder| builder.span.model_call_id.as_deref() == Some(model_call_id.as_str()))
        .map(|builder| builder.span_id.clone());
    let span_id = existing_span_id.unwrap_or_else(|| {
        event
            .span_id
            .clone()
            .unwrap_or_else(|| derive_span_id(run_id, "model", &model_call_id))
    });
    let model_name =
        payload_text(&event.payload, "model_name").unwrap_or_else(|| "model".to_owned());
    let builder = builders.entry(span_id.clone()).or_insert_with(|| {
        SpanBuilder::new(SpanInit {
            span_id,
            kind: SpanKind::Model,
            name: &model_name,
            event_seq: event.seq,
            started_at: None,
        })
    });
    if builder.span.model_call_id.is_none() {
        builder.span.event_seq = event.seq;
        builder.span.started_at =
            (event.event_type == "model.call.started").then_some(event.occurred_at);
    }
    builder.span.model_call_id = Some(model_call_id);
    builder.span.model_name = Some(model_name.clone());
    builder.span.name = model_name;
    builder.span.turn_index = event.payload.get("turn_index").and_then(Value::as_i64);
    builder.parent_hint = event
        .parent_span_id
        .clone()
        .or_else(|| builder.parent_hint.clone());
    match event.event_type.as_str() {
        "model.call.started" => {}
        "model.call.completed" | "routing.router_invoked" => {
            if builder.span.started_at.is_none() {
                builder.add_diagnostic("completion_without_start");
            }
            builder.span.usage = normalized_usage(event.payload.get("usage"));
            builder.span.finish_reason = payload_text(&event.payload, "finish_reason");
            builder.mark_terminal(SpanStatus::Completed, event.occurred_at);
        }
        "model.call.failed" => {
            if builder.span.started_at.is_none() {
                builder.add_diagnostic("completion_without_start");
            }
            let (error_code, error_message) = error_details(event.payload.get("error"));
            builder.span.error_code = error_code;
            builder.span.error_message = error_message;
            builder.mark_terminal(SpanStatus::Failed, event.occurred_at);
        }
        _ => {}
    }
}

fn apply_tool_event(
    builders: &mut BTreeMap<String, SpanBuilder>,
    event: &StoredEvent,
    run_id: &str,
) {
    let tool_call_id = payload_text(&event.payload, "tool_call_id")
        .unwrap_or_else(|| format!("event-{}", event.seq));
    let existing_span_id = builders
        .values()
        .find(|builder| builder.span.tool_call_id.as_deref() == Some(tool_call_id.as_str()))
        .map(|builder| builder.span_id.clone());
    let span_id = existing_span_id.unwrap_or_else(|| {
        event
            .span_id
            .clone()
            .unwrap_or_else(|| derive_span_id(run_id, "tool", &tool_call_id))
    });
    let tool_name = payload_text(&event.payload, "tool_name").unwrap_or_else(|| "tool".to_owned());
    let builder = builders.entry(span_id.clone()).or_insert_with(|| {
        SpanBuilder::new(SpanInit {
            span_id,
            kind: SpanKind::Tool,
            name: &tool_name,
            event_seq: event.seq,
            started_at: None,
        })
    });
    if builder.span.tool_call_id.is_none() {
        builder.span.event_seq = event.seq;
        builder.span.started_at =
            (event.event_type == "tool.call.started").then_some(event.occurred_at);
    }
    builder.span.tool_call_id = Some(tool_call_id);
    builder.span.tool_name = Some(tool_name.clone());
    builder.span.name = tool_name;
    builder.span.tool_arguments = event
        .payload
        .get("arguments")
        .cloned()
        .or_else(|| builder.span.tool_arguments.clone());
    builder.parent_hint = event
        .parent_span_id
        .clone()
        .or_else(|| builder.parent_hint.clone());
    builder.parent_model_call_id = payload_text(&event.payload, "parent_model_call_id")
        .or_else(|| builder.parent_model_call_id.clone());
    match event.event_type.as_str() {
        "tool.call.started" => {
            if builder.parent_model_call_id.is_none() && builder.parent_hint.is_none() {
                builder.add_diagnostic("parent_source_unavailable");
            }
        }
        "tool.call.completed" => {
            if builder.span.started_at.is_none() {
                builder.add_diagnostic("completion_without_start");
            }
            builder.span.tool_result = event.payload.get("result").cloned();
            builder.span.result_checksum = payload_text(&event.payload, "result_checksum");
            builder.span.result_length = builder
                .span
                .tool_result
                .as_ref()
                .and_then(value_character_length);
            builder.mark_terminal(SpanStatus::Completed, event.occurred_at);
        }
        "tool.call.failed" => {
            if builder.span.started_at.is_none() {
                builder.add_diagnostic("completion_without_start");
            }
            let (error_code, error_message) = error_details(event.payload.get("error"));
            builder.span.error_code = error_code;
            builder.span.error_message = error_message;
            builder.mark_terminal(SpanStatus::Failed, event.occurred_at);
        }
        _ => {}
    }
}

fn apply_artifact_event(
    builders: &mut BTreeMap<String, SpanBuilder>,
    event: &StoredEvent,
    run_id: &str,
) {
    let artifact_id = payload_text(&event.payload, "artifact_id")
        .unwrap_or_else(|| format!("event-{}", event.seq));
    let span_id = event
        .span_id
        .clone()
        .unwrap_or_else(|| derive_span_id(run_id, "artifact", &artifact_id));
    let artifact_name = payload_text(&event.payload, "filename")
        .or_else(|| payload_text(&event.payload, "type"))
        .unwrap_or_else(|| "artifact".to_owned());
    let builder = builders.entry(span_id.clone()).or_insert_with(|| {
        SpanBuilder::new(SpanInit {
            span_id,
            kind: SpanKind::Artifact,
            name: &artifact_name,
            event_seq: event.seq,
            started_at: Some(event.occurred_at),
        })
    });
    builder.span.result_checksum = payload_text(&event.payload, "checksum");
    builder.span.result_length = event.payload.get("size").and_then(Value::as_u64);
    builder.parent_hint = event.parent_span_id.clone();
    builder.mark_terminal(SpanStatus::Completed, event.occurred_at);
}

fn apply_decision_event(
    builders: &mut BTreeMap<String, SpanBuilder>,
    event: &StoredEvent,
    run_id: &str,
) {
    let decision_payload = parse_decision_payload(&event.payload);
    let decision_id = decision_payload
        .as_ref()
        .map(|decision| decision.decision_id.as_str())
        .unwrap_or("decision");
    let span_id = event
        .span_id
        .clone()
        .unwrap_or_else(|| derive_span_id(run_id, "decision", decision_id));
    let decision_name = decision_payload
        .as_ref()
        .map(|decision| format!("{}: {}", decision.gate, decision.verdict))
        .unwrap_or_else(|| "decision".to_owned());
    let builder = builders.entry(span_id.clone()).or_insert_with(|| {
        SpanBuilder::new(SpanInit {
            span_id,
            kind: SpanKind::Decision,
            name: &decision_name,
            event_seq: event.seq,
            started_at: Some(event.occurred_at),
        })
    });
    builder.span.decision = decision_payload;
    builder.tool_call_ref = payload_text(&event.payload, "tool_call_ref");
    builder.parent_hint = event.parent_span_id.clone();
    let status = if payload_text(&event.payload, "verdict").as_deref() == Some("block") {
        SpanStatus::Failed
    } else {
        SpanStatus::Completed
    };
    builder.mark_terminal(status, event.occurred_at);
}

fn resolve_parents(builders: &mut BTreeMap<String, SpanBuilder>, root_span_id: &str) {
    let model_call_spans = builders
        .values()
        .filter_map(|builder| {
            builder
                .span
                .model_call_id
                .as_ref()
                .map(|model_call_id| (model_call_id.clone(), builder.span_id.clone()))
        })
        .collect::<BTreeMap<_, _>>();
    let tool_call_spans = builders
        .values()
        .filter_map(|builder| {
            builder
                .span
                .tool_call_id
                .as_ref()
                .map(|tool_call_id| (tool_call_id.clone(), builder.span_id.clone()))
        })
        .collect::<BTreeMap<_, _>>();
    let known_span_ids = builders.keys().cloned().collect::<BTreeSet<_>>();
    for builder in builders.values_mut().filter(|builder| !builder.is_root) {
        let resolved_parent = builder
            .parent_hint
            .as_ref()
            .filter(|parent_id| known_span_ids.contains(*parent_id))
            .cloned()
            .or_else(|| {
                builder
                    .parent_model_call_id
                    .as_ref()
                    .and_then(|model_call_id| model_call_spans.get(model_call_id).cloned())
            })
            .or_else(|| {
                builder
                    .tool_call_ref
                    .as_ref()
                    .and_then(|tool_call_id| tool_call_spans.get(tool_call_id).cloned())
            });
        if resolved_parent.as_deref() == Some(builder.span_id.as_str()) {
            builder.add_diagnostic("self_parent");
            builder.span.parent_span_id = Some(root_span_id.to_owned());
        } else if let Some(parent_id) = resolved_parent {
            builder.span.parent_span_id = Some(parent_id);
        } else {
            if builder.tool_call_ref.is_some() {
                builder.add_diagnostic("decision_tool_ref_unresolved");
            } else if builder.parent_hint.is_some() || builder.parent_model_call_id.is_some() {
                builder.add_diagnostic("unknown_parent");
            }
            builder.span.parent_span_id = Some(root_span_id.to_owned());
        }
    }
    let parent_edges = builders
        .iter()
        .filter_map(|(span_id, builder)| {
            builder
                .span
                .parent_span_id
                .as_ref()
                .map(|parent_span_id| (span_id.clone(), parent_span_id.clone()))
        })
        .collect::<Vec<_>>();
    for (span_id, parent_span_id) in parent_edges {
        if parent_chain_contains(builders, &span_id, &parent_span_id)
            && let Some(builder) = builders.get_mut(&span_id)
        {
            builder.span.parent_span_id = Some(root_span_id.to_owned());
            builder.add_diagnostic("cyclic_parent");
        }
    }
}

fn parent_chain_contains(
    builders: &BTreeMap<String, SpanBuilder>,
    child_span_id: &str,
    parent_span_id: &str,
) -> bool {
    let mut current_span_id = parent_span_id;
    let mut visited_span_ids = BTreeSet::new();
    loop {
        if current_span_id == child_span_id || !visited_span_ids.insert(current_span_id.to_owned())
        {
            return true;
        }
        let Some(next_parent_id) = builders
            .get(current_span_id)
            .and_then(|builder| builder.span.parent_span_id.as_deref())
        else {
            return false;
        };
        current_span_id = next_parent_id;
    }
}

fn settle_open_spans(builders: &mut BTreeMap<String, SpanBuilder>, run_status: RunStatus) {
    if !run_status.is_terminal() {
        return;
    }
    for builder in builders.values_mut() {
        if builder.span.status == SpanStatus::Running {
            builder.span.status = SpanStatus::Incomplete;
            builder.add_diagnostic("started_without_completion");
        }
    }
}

fn collect_diagnostics(spans: &[TraceSpan]) -> Vec<TraceDiagnostic> {
    let mut unique_diagnostics = BTreeSet::new();
    for span in spans {
        for diagnostic_code in &span.diagnostics {
            unique_diagnostics.insert((diagnostic_code.clone(), Some(span.span_id.clone())));
        }
    }
    unique_diagnostics
        .into_iter()
        .map(|(diagnostic_code, span_id)| diagnostic(&diagnostic_code, span_id))
        .collect()
}

fn build_tree(spans: &[TraceSpan], root_span_id: &str) -> Vec<TraceTreeNode> {
    let span_by_id = spans
        .iter()
        .cloned()
        .map(|span| (span.span_id.clone(), span))
        .collect::<BTreeMap<_, _>>();
    let mut children_by_parent = BTreeMap::<String, Vec<String>>::new();
    for span in spans.iter().filter(|span| span.span_id != root_span_id) {
        if let Some(parent_span_id) = &span.parent_span_id {
            children_by_parent
                .entry(parent_span_id.clone())
                .or_default()
                .push(span.span_id.clone());
        }
    }
    let mut built_nodes = BTreeMap::<String, TraceTreeNode>::new();
    let mut ordered_ids = spans
        .iter()
        .map(|span| span.span_id.clone())
        .collect::<Vec<_>>();
    ordered_ids.sort_by(|left_id, right_id| {
        let left_span = &span_by_id[left_id];
        let right_span = &span_by_id[right_id];
        right_span
            .started_at
            .cmp(&left_span.started_at)
            .then_with(|| right_span.event_seq.cmp(&left_span.event_seq))
    });
    for span_id in ordered_ids {
        let Some(span) = span_by_id.get(&span_id).cloned() else {
            continue;
        };
        let child_ids = children_by_parent
            .get(&span_id)
            .cloned()
            .unwrap_or_default();
        let children = child_ids
            .into_iter()
            .filter_map(|child_id| built_nodes.remove(&child_id))
            .collect();
        built_nodes.insert(span_id, TraceTreeNode { span, children });
    }
    built_nodes
        .remove(root_span_id)
        .map(|root_node| vec![root_node])
        .unwrap_or_default()
}

fn normalized_usage(raw_usage: Option<&Value>) -> Option<BTreeMap<String, u64>> {
    let usage_values = raw_usage?.as_object()?;
    let normalized = usage_values
        .iter()
        .filter_map(|(key, value)| value.as_u64().map(|count| (key.clone(), count)))
        .collect::<BTreeMap<_, _>>();
    (!normalized.is_empty()).then_some(normalized)
}

fn parse_decision_payload(payload: &Value) -> Option<DecisionPayload> {
    let probabilities = payload
        .get("probabilities")
        .and_then(Value::as_object)
        .map(|values| {
            values
                .iter()
                .filter_map(|(label, value)| {
                    value
                        .as_f64()
                        .map(|probability| (label.clone(), probability))
                })
                .collect::<BTreeMap<_, _>>()
        })
        .unwrap_or_default();
    Some(DecisionPayload {
        decision_id: payload_text(payload, "decision_id")?,
        gate: payload_text(payload, "gate")?,
        tool_name: payload_text(payload, "tool_name")?,
        tool_call_ref: payload_text(payload, "tool_call_ref").unwrap_or_default(),
        verdict: payload_text(payload, "verdict")?,
        answer: payload_text(payload, "answer")?,
        probability: payload.get("probability").and_then(Value::as_f64),
        probabilities,
        irreversible: payload.get("irreversible").and_then(Value::as_bool),
        fail_open: payload
            .get("fail_open")
            .and_then(Value::as_bool)
            .unwrap_or(false),
        policy_fingerprint: payload_text(payload, "policy_fingerprint")?,
        elapsed_ms: payload
            .get("elapsed_ms")
            .and_then(Value::as_u64)
            .unwrap_or(0),
        judgement_state: payload_text(payload, "judgement_state").unwrap_or_default(),
    })
}

fn error_details(error_value: Option<&Value>) -> (Option<String>, Option<String>) {
    match error_value {
        Some(Value::Object(error_object)) => (
            error_object
                .get("code")
                .and_then(Value::as_str)
                .map(str::to_owned),
            error_object
                .get("message")
                .or_else(|| error_object.get("detail"))
                .map(value_display),
        ),
        Some(error_text @ Value::String(_)) => (None, Some(value_display(error_text))),
        Some(error_document) => (None, Some(value_display(error_document))),
        None => (None, None),
    }
}

fn value_display(value: &Value) -> String {
    match value {
        Value::String(text_value) => text_value.clone(),
        _ => serde_json::to_string(value).unwrap_or_default(),
    }
}

fn value_character_length(value: &Value) -> Option<u64> {
    match value {
        Value::String(text_value) => Some(text_value.chars().count() as u64),
        _ => Some(serde_json::to_string(value).ok()?.chars().count() as u64),
    }
}

fn payload_text(payload: &Value, field_name: &str) -> Option<String> {
    payload
        .get(field_name)
        .and_then(Value::as_str)
        .map(str::to_owned)
}

/// 计算两个可选时间点间的非负毫秒数。
pub(super) fn elapsed_ms(
    started_at: Option<OffsetDateTime>,
    finished_at: Option<OffsetDateTime>,
) -> Option<i64> {
    let elapsed = finished_at?.unix_timestamp_nanos() - started_at?.unix_timestamp_nanos();
    Some((elapsed / 1_000_000).clamp(0, i64::MAX as i128) as i64)
}

fn root_status_for_event(event_type: &str) -> SpanStatus {
    match event_type {
        "run.completed" => SpanStatus::Completed,
        "run.failed" => SpanStatus::Failed,
        "run.cancelled" | "run.interrupted" => SpanStatus::Incomplete,
        _ => SpanStatus::Running,
    }
}

fn diagnostic(code: &str, span_id: Option<String>) -> TraceDiagnostic {
    let message = match code {
        "parent_source_unavailable" => "工具调用的父级来源不可确认，已挂到 Run 根节点。",
        "unknown_parent" => "事件声明的父级节点不存在，已降级挂到 Run 根节点。",
        "self_parent" => "节点被声明为自身的父级，已降级挂到 Run 根节点。",
        "cyclic_parent" => "节点父子关系存在环，已降级挂到 Run 根节点。",
        "started_without_completion" => "Run 已终结但该步骤没有收到完成事件。",
        "completion_without_start" => "只收到完成事件，没有对应的开始事件。",
        "duplicate_completion" => "同一节点收到多次终态事件，保留首次结果。",
        "missing_span_id" => "事件没有携带 span 标识，已按来源调用 ID 派生。",
        "no_tracing_events" => "该 Run 没有模型或工具 tracing 事件，诊断视图受限。",
        "decision_tool_ref_unresolved" => {
            "判定事件无法按 tool_call_ref 对账到工具节点，已挂 Run 根节点。"
        }
        _ => "事件存在无法确认的父级关系，已挂到 Run 根节点。",
    };
    TraceDiagnostic {
        code: code.to_owned(),
        message: message.to_owned(),
        span_id,
    }
}

fn derive_identifier(discriminator: &str, source_identifier: &str, hex_length: usize) -> String {
    let canonical_input = format!("v1:{discriminator}:{source_identifier}");
    let digest = Sha256::digest(canonical_input.as_bytes());
    let mut hex_digest = digest
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect::<String>();
    hex_digest.truncate(hex_length);
    if hex_digest.bytes().all(|byte| byte == b'0') {
        hex_digest.pop();
        hex_digest.push('1');
    }
    hex_digest
}

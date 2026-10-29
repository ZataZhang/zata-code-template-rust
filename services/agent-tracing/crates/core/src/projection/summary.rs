//! Run 列表摘要与低敏活动指标投影。

use std::collections::{BTreeMap, BTreeSet};

use super::{elapsed_ms, payload_text};
use crate::model::{RunActivity, RunRecord, StoredEvent, TraceSummary, TraceVisibility};
use serde_json::Value;

/// 从一组事件统计列表和详情共用的低敏活动指标。
pub fn summarize_run_events(events: &[StoredEvent]) -> RunActivity {
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
    let model_call_count = events
        .iter()
        .filter(|event| {
            matches!(
                event.event_type.as_str(),
                "model.call.started" | "routing.router_invoked"
            )
        })
        .filter_map(|event| payload_text(&event.payload, "model_call_id"))
        .collect::<BTreeSet<_>>()
        .len() as u32;
    let tool_call_count = events
        .iter()
        .filter(|event| event.event_type == "tool.call.started")
        .filter_map(|event| payload_text(&event.payload, "tool_call_id"))
        .collect::<BTreeSet<_>>()
        .len() as u32;
    let mut usage_totals = BTreeMap::<String, u64>::new();
    for event in events.iter().filter(|event| {
        matches!(
            event.event_type.as_str(),
            "model.call.completed" | "routing.router_invoked"
        )
    }) {
        if let Some(usage_object) = event.payload.get("usage").and_then(Value::as_object) {
            for (metric_name, raw_metric) in usage_object {
                if let Some(metric_value) = raw_metric.as_u64() {
                    *usage_totals.entry(metric_name.clone()).or_default() += metric_value;
                }
            }
        }
    }
    let error_count = events
        .iter()
        .filter(|event| {
            matches!(
                event.event_type.as_str(),
                "model.call.failed" | "tool.call.failed"
            ) || (event.event_type == "decision.gate.evaluated"
                && payload_text(&event.payload, "verdict").as_deref() == Some("block"))
        })
        .count() as u32;
    RunActivity {
        has_step_events,
        model_call_count,
        tool_call_count,
        error_count,
        usage: usage_totals,
    }
}

/// 从 Run 投影与低敏活动汇总构建列表摘要。
pub fn project_run_summary(
    run: &RunRecord,
    trace_id: &str,
    activity: &RunActivity,
) -> TraceSummary {
    TraceSummary {
        run_id: run.run_id.clone(),
        thread_id: run.thread_id.clone(),
        source_id: run.source_id.clone(),
        project_id: run.project_id.clone(),
        agent_id: run.agent_id.clone(),
        agent_name: run.agent_id.clone(),
        status: run.status,
        created_at: run.created_at,
        started_at: run.started_at,
        finished_at: run.finished_at,
        duration_ms: elapsed_ms(run.started_at, run.finished_at),
        trace_id: trace_id.to_owned(),
        visibility: if activity.has_step_events {
            TraceVisibility::Full
        } else {
            TraceVisibility::Limited
        },
        model_call_count: activity.model_call_count,
        tool_call_count: activity.tool_call_count,
        usage: (!activity.usage.is_empty()).then_some(activity.usage.clone()),
        error_count: activity.error_count,
    }
}

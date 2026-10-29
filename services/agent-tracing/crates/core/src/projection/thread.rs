//! Thread 会话账本投影。

use std::collections::{BTreeMap, BTreeSet};

use serde_json::Value;

use super::{payload_text, project_run_trace};
use crate::model::{
    RunRecord, StoredEvent, ThreadSummary, ThreadTraceDetail, ThreadTraceRecord, TraceDetail,
    TraceVisibility,
};

/// 将 thread 下的 Run 详情、消息、委派与步骤合并成按发生时间排列的账本。
pub fn project_thread_trace(
    thread_id: &str,
    run_details: Vec<TraceDetail>,
    source_runs: &[(RunRecord, Vec<StoredEvent>)],
) -> Option<ThreadTraceDetail> {
    let (first_run, _) = source_runs.first()?;
    let mut status_counts = BTreeMap::new();
    let mut agent_names = BTreeSet::new();
    let mut records = Vec::new();
    let mut limited_run_count = 0_u64;
    let mut created_at = first_run.created_at;
    let mut last_activity_at = first_run.last_activity_at;

    for (run, events) in source_runs {
        created_at = created_at.min(run.created_at);
        last_activity_at = last_activity_at.max(run.last_activity_at);
        agent_names.insert(run.agent_id.clone());
        *status_counts
            .entry(run.status.as_str().to_owned())
            .or_insert(0) += 1;
        let detail = project_run_trace(run, events);
        if matches!(detail.summary.visibility, TraceVisibility::Limited) {
            limited_run_count += 1;
        }
        records.push(ThreadTraceRecord {
            record_id: format!("run:{}:0", run.run_id),
            kind: "run".to_owned(),
            thread_id: thread_id.to_owned(),
            run_id: run.run_id.clone(),
            seq: 0,
            occurred_at: Some(run.created_at),
            role: None,
            content: Some(serde_json::json!({"status": run.status, "agent_id": run.agent_id})),
            span: None,
            child_run_id: None,
        });
        let spans_by_sequence = detail
            .spans
            .iter()
            .map(|span| (span.event_seq, span.clone()))
            .collect::<BTreeMap<_, _>>();
        let message_roles = events
            .iter()
            .filter(|event| event.event_type == "message.started")
            .filter_map(|event| {
                Some((
                    event.payload.get("message_id")?.as_str()?.to_owned(),
                    event.payload.get("role")?.as_str()?.to_owned(),
                ))
            })
            .collect::<BTreeMap<_, _>>();
        for event in events {
            match event.event_type.as_str() {
                "input.accepted" => records.push(message_record(
                    thread_id,
                    run,
                    event,
                    "user",
                    event.payload.get("content").cloned(),
                )),
                "message.completed" => {
                    let message_id = payload_text(&event.payload, "message_id");
                    let role = message_roles
                        .get(message_id.as_deref().unwrap_or_default())
                        .map(String::as_str)
                        .unwrap_or("assistant");
                    records.push(message_record(
                        thread_id,
                        run,
                        event,
                        role,
                        event.payload.get("content").cloned(),
                    ));
                }
                event_type if event_type.starts_with("delegation.") => {
                    records.push(ThreadTraceRecord {
                        record_id: format!("delegation:{}:{}", run.run_id, event.seq),
                        kind: "delegation".to_owned(),
                        thread_id: thread_id.to_owned(),
                        run_id: run.run_id.clone(),
                        seq: event.seq,
                        occurred_at: Some(event.occurred_at),
                        role: None,
                        content: Some(event.payload.clone()),
                        span: None,
                        child_run_id: event
                            .payload
                            .get("child_run_id")
                            .and_then(Value::as_str)
                            .map(str::to_owned),
                    });
                }
                _ => {}
            }
            if let Some(span) = spans_by_sequence.get(&event.seq) {
                records.push(ThreadTraceRecord {
                    record_id: format!("span:{}:{}", run.run_id, event.seq),
                    kind: "span".to_owned(),
                    thread_id: thread_id.to_owned(),
                    run_id: run.run_id.clone(),
                    seq: event.seq,
                    occurred_at: Some(event.occurred_at),
                    role: None,
                    content: None,
                    span: Some(span.clone()),
                    child_run_id: None,
                });
            }
        }
    }
    records.sort_by(|left, right| {
        left.occurred_at
            .cmp(&right.occurred_at)
            .then_with(|| left.run_id.cmp(&right.run_id))
            .then_with(|| left.seq.cmp(&right.seq))
    });
    let summary = ThreadSummary {
        thread_id: thread_id.to_owned(),
        source_id: first_run.source_id.clone(),
        project_id: first_run.project_id.clone(),
        run_count: source_runs.len() as u64,
        limited_run_count,
        agent_names: agent_names.into_iter().collect(),
        status_counts,
        created_at,
        last_activity_at,
    };
    Some(ThreadTraceDetail {
        summary,
        runs: run_details,
        records,
    })
}

fn message_record(
    thread_id: &str,
    run: &RunRecord,
    event: &StoredEvent,
    role: &str,
    content: Option<Value>,
) -> ThreadTraceRecord {
    ThreadTraceRecord {
        record_id: format!("message:{}:{}", run.run_id, event.seq),
        kind: "message".to_owned(),
        thread_id: thread_id.to_owned(),
        run_id: run.run_id.clone(),
        seq: event.seq,
        occurred_at: Some(event.occurred_at),
        role: Some(role.to_owned()),
        content,
        span: None,
        child_run_id: None,
    }
}

//! tracing producer HTTP 投递与 outbox 后台 worker。
//!
//! Run 事件先与 outbox 同事务落库；本模块按序、幂等、有界退避地投递，
//! 只保存 ack 检查点。tracing 不可用只让事件停留在 pending，不影响 Run 结果。

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use reqwest::StatusCode;
use serde::Deserialize;
use serde_json::json;
use time::OffsetDateTime;
use tpl_agent_core::error::AgentError;
use tpl_agent_core::event_contract::MAX_TRACE_BATCH_EVENTS;
use tpl_agent_core::ports::RunRepository;
use tpl_agent_core::run::PendingTraceEvent;
use tpl_agent_core::time_utils::now;
use uuid::Uuid;

/// tracing ingest 单请求体上限。
const MAX_INGEST_BODY_BYTES: usize = 2 * 1024 * 1024;
/// ack 响应体读取上限。
const MAX_ACK_BODY_BYTES: usize = 8 * 1024;
/// producer token 允许的字节范围。
const MIN_PRODUCER_TOKEN_BYTES: usize = 32;
const MAX_PRODUCER_TOKEN_BYTES: usize = 4096;
/// event envelope 之外的固定 JSON 包装估算。
const BATCH_ENVELOPE_OVERHEAD_BYTES: usize = 256;

/// tracing 投递错误类别；只用于日志与 outbox 的 `last_error_code`。
#[derive(Clone, Copy, Debug, PartialEq, Eq, thiserror::Error)]
pub enum TracingDeliveryError {
    /// 客户端配置不允许投递。
    #[error("tracing ingest 配置无效")]
    InvalidConfiguration,
    /// producer 凭据被拒绝。
    #[error("tracing ingest 拒绝 producer 凭据")]
    Unauthorized,
    /// 远端已存在相同序号的不同事件。
    #[error("tracing ingest 序号冲突")]
    SequenceConflict,
    /// 批次超过远端限制。
    #[error("tracing ingest 批次超过上限")]
    BatchTooLarge,
    /// 远端响应不可解析或缺少检查点。
    #[error("tracing ingest 响应无效")]
    InvalidResponse,
    /// 网络、超时或远端 5xx。
    #[error("tracing ingest 暂时不可用")]
    Unavailable,
}

impl TracingDeliveryError {
    /// 写入 outbox 的稳定错误码。
    pub fn code(self) -> &'static str {
        match self {
            Self::InvalidConfiguration => "tracing_invalid_configuration",
            Self::Unauthorized => "producer_unauthorized",
            Self::SequenceConflict => "sequence_conflict",
            Self::BatchTooLarge => "ingest_limit_exceeded",
            Self::InvalidResponse => "tracing_invalid_response",
            Self::Unavailable => "tracing_unavailable",
        }
    }
}

/// tracing producer ack 的最小契约。
#[derive(Deserialize)]
struct TracingAck {
    accepted_through_seq: i64,
}

/// 调用 agent-tracing producer API 的客户端。
pub struct TracingIngestClient {
    client: reqwest::Client,
    ingest_url: String,
    producer_token: String,
}

impl TracingIngestClient {
    /// 创建带超时与禁用重定向的 ingest 客户端。
    ///
    /// # Errors
    ///
    /// token 长度/字符非法、URL 为空或 HTTP 客户端无法建立时返回错误。
    pub fn new(
        ingest_url: String,
        producer_token: String,
        request_timeout: Duration,
    ) -> Result<Self, TracingDeliveryError> {
        let ingest_url = ingest_url.trim().trim_end_matches('/').to_owned();
        if ingest_url.is_empty()
            || ingest_url.len() > 2048
            || ingest_url.chars().any(char::is_control)
        {
            return Err(TracingDeliveryError::InvalidConfiguration);
        }
        if producer_token.len() < MIN_PRODUCER_TOKEN_BYTES
            || producer_token.len() > MAX_PRODUCER_TOKEN_BYTES
            || producer_token.chars().any(char::is_control)
        {
            return Err(TracingDeliveryError::InvalidConfiguration);
        }
        let client = reqwest::Client::builder()
            .timeout(request_timeout)
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .map_err(|_| TracingDeliveryError::InvalidConfiguration)?;
        Ok(Self {
            client,
            ingest_url,
            producer_token,
        })
    }

    /// 发送一个连续事件批次并返回远端检查点。
    ///
    /// # Errors
    ///
    /// 批次为空/超限、凭据被拒绝、序号冲突或远端不可用时返回错误。
    pub async fn send_batch(
        &self,
        run_id: Uuid,
        events: &[PendingTraceEvent],
    ) -> Result<i64, TracingDeliveryError> {
        if events.is_empty() || events.len() > MAX_TRACE_BATCH_EVENTS {
            return Err(TracingDeliveryError::BatchTooLarge);
        }
        let envelopes = events
            .iter()
            .map(PendingTraceEvent::to_envelope)
            .collect::<Result<Vec<_>, AgentError>>()
            .map_err(|_| TracingDeliveryError::InvalidResponse)?;
        let request_body = serde_json::to_vec(&json!({
            "schema_version": 1,
            "events": envelopes,
        }))
        .map_err(|_| TracingDeliveryError::InvalidResponse)?;
        if request_body.len() > MAX_INGEST_BODY_BYTES {
            return Err(TracingDeliveryError::BatchTooLarge);
        }
        let request_url = format!("{}/runs/{run_id}/events", self.ingest_url);
        let response = self
            .client
            .post(request_url)
            .header(
                reqwest::header::AUTHORIZATION,
                format!("Bearer {}", self.producer_token),
            )
            .header(reqwest::header::CONTENT_TYPE, "application/json")
            .body(request_body)
            .send()
            .await
            .map_err(|_| TracingDeliveryError::Unavailable)?;
        match response.status() {
            StatusCode::OK => {}
            StatusCode::UNAUTHORIZED | StatusCode::FORBIDDEN => {
                return Err(TracingDeliveryError::Unauthorized);
            }
            StatusCode::CONFLICT => return Err(TracingDeliveryError::SequenceConflict),
            StatusCode::PAYLOAD_TOO_LARGE => return Err(TracingDeliveryError::BatchTooLarge),
            _ => return Err(TracingDeliveryError::Unavailable),
        }
        if response
            .content_length()
            .is_some_and(|body_size| body_size > MAX_ACK_BODY_BYTES as u64)
        {
            return Err(TracingDeliveryError::InvalidResponse);
        }
        let response_body = response
            .bytes()
            .await
            .map_err(|_| TracingDeliveryError::Unavailable)?;
        if response_body.len() > MAX_ACK_BODY_BYTES {
            return Err(TracingDeliveryError::InvalidResponse);
        }
        let ack: TracingAck = serde_json::from_slice(&response_body)
            .map_err(|_| TracingDeliveryError::InvalidResponse)?;
        let last_sent_seq = events.last().map(|event| event.seq).unwrap_or_default();
        if ack.accepted_through_seq < last_sent_seq {
            return Err(TracingDeliveryError::InvalidResponse);
        }
        Ok(ack.accepted_through_seq)
    }
}

/// 将待投递事件按条数与请求体大小切分为连续批次。
fn split_batches(
    events: &[PendingTraceEvent],
) -> Result<Vec<Vec<PendingTraceEvent>>, TracingDeliveryError> {
    let body_budget = MAX_INGEST_BODY_BYTES.saturating_sub(BATCH_ENVELOPE_OVERHEAD_BYTES);
    let mut batches = Vec::new();
    let mut current_batch: Vec<PendingTraceEvent> = Vec::new();
    let mut current_bytes = 0_usize;
    for event in events {
        let envelope_bytes = serde_json::to_vec(
            &event
                .to_envelope()
                .map_err(|_| TracingDeliveryError::InvalidResponse)?,
        )
        .map_err(|_| TracingDeliveryError::InvalidResponse)?
        .len()
            + 1;
        if envelope_bytes > body_budget {
            return Err(TracingDeliveryError::BatchTooLarge);
        }
        let would_overflow = current_bytes.saturating_add(envelope_bytes) > body_budget;
        if would_overflow || current_batch.len() == MAX_TRACE_BATCH_EVENTS {
            batches.push(std::mem::take(&mut current_batch));
            current_bytes = 0;
        }
        current_bytes = current_bytes.saturating_add(envelope_bytes);
        current_batch.push(event.clone());
    }
    if !current_batch.is_empty() {
        batches.push(current_batch);
    }
    Ok(batches)
}

/// outbox 投递退避：1s 起、指数增长、封顶 60s。
fn retry_delay(failures: u32) -> Duration {
    let exponent = failures.min(6);
    Duration::from_secs((1_u64 << exponent).min(60))
}

/// 持续轮询本地 outbox 并投递 tracing 事件。
pub struct TracingOutboxWorker {
    runs: Arc<dyn RunRepository>,
    client: Arc<TracingIngestClient>,
    poll_interval: Duration,
    run_batch_size: usize,
}

impl TracingOutboxWorker {
    /// 创建 worker；`run_batch_size` 是本轮最多处理的 Run 数。
    pub fn new(
        runs: Arc<dyn RunRepository>,
        client: Arc<TracingIngestClient>,
        poll_interval: Duration,
    ) -> Self {
        Self {
            runs,
            client,
            poll_interval: poll_interval.max(Duration::from_millis(100)),
            run_batch_size: 20,
        }
    }

    /// 后台循环；收到 shutdown 信号后退出。
    pub async fn run(self, mut shutdown: tokio::sync::watch::Receiver<bool>) {
        let mut failure_counts: HashMap<Uuid, u32> = HashMap::new();
        loop {
            if *shutdown.borrow() {
                return;
            }
            match self.drain_once(&mut failure_counts).await {
                Ok(()) => {}
                Err(drain_error) => {
                    tracing::warn!(error = %drain_error, "tracing outbox 轮询失败");
                }
            }
            tokio::select! {
                () = tokio::time::sleep(self.poll_interval) => {}
                changed = shutdown.changed() => {
                    if changed.is_err() || *shutdown.borrow() {
                        return;
                    }
                }
            }
        }
    }

    async fn drain_once(&self, failure_counts: &mut HashMap<Uuid, u32>) -> Result<(), AgentError> {
        let run_ids = self
            .runs
            .list_pending_outbox_run_ids(self.run_batch_size)
            .await?;
        for run_id in run_ids {
            if let Err(delivery_error) = self.deliver_run(run_id, failure_counts).await {
                tracing::warn!(
                    run_id = %run_id,
                    error = %delivery_error,
                    "tracing outbox 投递失败；事件保留待重试"
                );
            }
        }
        Ok(())
    }

    async fn deliver_run(
        &self,
        run_id: Uuid,
        failure_counts: &mut HashMap<Uuid, u32>,
    ) -> Result<(), AgentError> {
        let pending_events = self
            .runs
            .fetch_pending_outbox_for_run(run_id, MAX_TRACE_BATCH_EVENTS)
            .await?;
        if pending_events.is_empty() {
            return Ok(());
        }
        let batches = match split_batches(&pending_events) {
            Ok(batches) => batches,
            Err(batch_error) => {
                self.record_failure(run_id, pending_events[0].seq, batch_error, failure_counts)
                    .await?;
                return Ok(());
            }
        };
        for batch in batches {
            let last_seq = batch.last().map(|event| event.seq).unwrap_or_default();
            match self.client.send_batch(run_id, &batch).await {
                Ok(accepted_through_seq) => {
                    self.runs
                        .mark_trace_delivered(run_id, accepted_through_seq.min(last_seq), now())
                        .await?;
                    failure_counts.remove(&run_id);
                }
                Err(delivery_error) => {
                    self.record_failure(run_id, batch[0].seq, delivery_error, failure_counts)
                        .await?;
                    return Ok(());
                }
            }
        }
        Ok(())
    }

    async fn record_failure(
        &self,
        run_id: Uuid,
        seq: i64,
        delivery_error: TracingDeliveryError,
        failure_counts: &mut HashMap<Uuid, u32>,
    ) -> Result<(), AgentError> {
        let failures = failure_counts.entry(run_id).or_insert(0);
        *failures = failures.saturating_add(1);
        let next_attempt_at = OffsetDateTime::now_utc() + retry_delay(*failures);
        tracing::warn!(
            run_id = %run_id,
            error = delivery_error.code(),
            retry_in_seconds = retry_delay(*failures).as_secs(),
            "tracing 投递失败，按退避重试"
        );
        self.runs
            .mark_trace_failed(run_id, seq, delivery_error.code(), next_attempt_at)
            .await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pending_event(seq: i64, payload_size: usize) -> PendingTraceEvent {
        PendingTraceEvent {
            run_id: Uuid::new_v4(),
            seq,
            event_type: "run.started".to_owned(),
            occurred_at: now(),
            payload: json!({"started_at": "x".repeat(payload_size)}),
        }
    }

    #[test]
    fn client_rejects_short_tokens_and_empty_urls() {
        assert_eq!(
            TracingIngestClient::new(
                "http://127.0.0.1:8082/api/v1/ingest".to_owned(),
                "short".to_owned(),
                Duration::from_secs(1)
            )
            .err(),
            Some(TracingDeliveryError::InvalidConfiguration)
        );
        assert_eq!(
            TracingIngestClient::new(String::new(), "a".repeat(32), Duration::from_secs(1)).err(),
            Some(TracingDeliveryError::InvalidConfiguration)
        );
        assert!(
            TracingIngestClient::new(
                "http://127.0.0.1:8082/api/v1/ingest".to_owned(),
                "a".repeat(32),
                Duration::from_secs(1)
            )
            .is_ok()
        );
    }

    #[test]
    fn batches_follow_body_size_and_event_count_limits() {
        let small_events = (1..=150)
            .map(|seq| pending_event(seq, 4))
            .collect::<Vec<_>>();
        let batches = split_batches(&small_events).unwrap();
        assert_eq!(batches.iter().map(Vec::len).sum::<usize>(), 150);
        assert!(
            batches
                .iter()
                .all(|batch| batch.len() <= MAX_TRACE_BATCH_EVENTS)
        );

        let large_event = pending_event(1, MAX_INGEST_BODY_BYTES);
        assert_eq!(
            split_batches(&[large_event]).err(),
            Some(TracingDeliveryError::BatchTooLarge)
        );
    }

    #[test]
    fn retry_delay_is_bounded() {
        assert_eq!(retry_delay(0), Duration::from_secs(1));
        assert_eq!(retry_delay(1), Duration::from_secs(2));
        assert_eq!(retry_delay(10), Duration::from_secs(60));
    }
}

//! queued Run 的后台执行调度。
//!
//! 多个副本或重复轮询可能领取同一 Run；`mark_run_started` 是幂等闸门，
//! 因此重复调度不会重复执行。

use std::collections::HashSet;
use std::sync::Arc;
use std::time::Duration;

use tokio::sync::{Semaphore, mpsc, watch};
use tokio::task::JoinHandle;
use tpl_agent_core::ports::RunRepository;
use tpl_agent_core::service::AgentService;
use uuid::Uuid;

/// 调度轮询间隔。
const DISPATCH_POLL_INTERVAL: Duration = Duration::from_millis(200);
/// 单轮最多领取的 queued Run 数。
const DISPATCH_BATCH_SIZE: usize = 32;

/// 启动 Run 调度器；收到 shutdown 信号后停止领取新 Run。
pub(crate) fn spawn(
    service: Arc<AgentService>,
    runs: Arc<dyn RunRepository>,
    mut shutdown: watch::Receiver<bool>,
    concurrency: usize,
) -> JoinHandle<()> {
    tokio::spawn(async move {
        let concurrency_slots = Arc::new(Semaphore::new(concurrency.max(1)));
        let (finished_sender, mut finished_receiver) = mpsc::unbounded_channel::<Uuid>();
        let mut in_flight = HashSet::new();
        loop {
            if *shutdown.borrow() {
                return;
            }
            while let Ok(finished_run_id) = finished_receiver.try_recv() {
                in_flight.remove(&finished_run_id);
            }
            match runs.list_queued_run_ids(DISPATCH_BATCH_SIZE).await {
                Ok(queued_run_ids) => {
                    for run_id in queued_run_ids {
                        if in_flight.contains(&run_id) {
                            continue;
                        }
                        let Ok(execution_permit) =
                            Arc::clone(&concurrency_slots).try_acquire_owned()
                        else {
                            break;
                        };
                        in_flight.insert(run_id);
                        let execution_service = Arc::clone(&service);
                        let finished_sender = finished_sender.clone();
                        tokio::spawn(async move {
                            let _execution_permit = execution_permit;
                            if let Err(execution_error) =
                                execution_service.execute_run(run_id).await
                            {
                                tracing::warn!(
                                    run_id = %run_id,
                                    error = %execution_error,
                                    "后台执行 Run 失败"
                                );
                            }
                            let _ = finished_sender.send(run_id);
                        });
                    }
                }
                Err(queue_error) => {
                    tracing::warn!(error = %queue_error, "读取 queued Run 失败");
                }
            }
            tokio::select! {
                () = tokio::time::sleep(DISPATCH_POLL_INTERVAL) => {}
                changed = shutdown.changed() => {
                    if changed.is_err() || *shutdown.borrow() {
                        return;
                    }
                }
            }
        }
    })
}

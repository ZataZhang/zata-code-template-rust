//! Canonical Run 执行内核：资源校验、事件观察、协作取消、受治理委派与终态落盘。
//!
//! 执行器只接触冻结快照与只读资源；所有状态变更、消息、产物与事件都由本模块写回，
//! tracing 投递由持久化层 outbox 异步完成，tracing 不可用不影响 Run 结果。

use std::sync::Arc;

use async_trait::async_trait;
use uuid::Uuid;

use super::{AgentService, AgentServiceDependencies};
use crate::delegation::{DelegationPolicy, DelegationRejection, admit_delegation};
use crate::error::AgentError;
use crate::event_contract::validate_event;
use crate::hash::{sha256_hex, sha256_json};
use crate::ports::{
    CancellationProbe, DelegationHandler, DelegationRequest, DelegationResult, ExecutionObserver,
    ExecutionOutcome, ExecutionRequest, RunRepository, SkillAssetContent, error_code,
};
use crate::resources::{
    Artifact, NewArtifact, ResourceContent, ensure_resource_owned, sanitize_filename,
    validate_artifact,
};
use crate::routing::{RoutingCandidate, RoutingDecision, decide_route};
use crate::run::{
    FinalizeRunCommand, NewRun, RunEventDraft, RunOrigin, RunRecord, RunSnapshot, RunSnapshotInput,
    RunStatus, ensure_terminal_event,
};
use crate::run_events::{
    artifact_created, delegation_cancelled, delegation_completed, delegation_failed,
    delegation_rejected, delegation_started, input_accepted, message_completed, message_started,
    routing_selected, run_cancelled, run_completed, run_created, run_failed,
};
use crate::runtime_catalog::{RuntimeConnection, ensure_runtime_ready};
use crate::session::{ChatMessage, MessageRole};
use crate::time_utils::now;

/// 委派失败时的父 Run 处理策略，写入 `delegation.*` 事件。
const DELEGATION_FAILURE_POLICY: &str = "return_to_parent";

impl AgentService {
    /// 执行并等待一个 Run；可预期的执行错误会落为 Run 终态。
    ///
    /// # Errors
    ///
    /// 状态无法读取或终态无法落盘时返回错误。
    pub async fn execute_run(&self, run_id: Uuid) -> Result<(), AgentError> {
        ExecutionKernel::new(Arc::clone(&self.deps))
            .run(run_id, None)
            .await
    }
}

#[derive(Clone)]
struct ExecutionKernel {
    deps: Arc<AgentServiceDependencies>,
}

impl ExecutionKernel {
    fn new(deps: Arc<AgentServiceDependencies>) -> Self {
        Self { deps }
    }

    async fn run(&self, run_id: Uuid, parent_run_id: Option<Uuid>) -> Result<(), AgentError> {
        let Some(existing_run) = self.deps.runs.get_run(run_id).await? else {
            return Ok(());
        };
        if !existing_run.status.is_active() {
            return Ok(());
        }
        let Some(run) = self.deps.runs.mark_run_started(run_id, now()).await? else {
            return Ok(());
        };
        match self.execute_started(&run, parent_run_id).await {
            Ok(()) => Ok(()),
            Err(execution_error) => {
                let latest_status = self.deps.runs.get_run_status(run_id).await?;
                if latest_status.is_some_and(RunStatus::is_terminal) {
                    return Ok(());
                }
                self.finalize_failed(&run, &execution_error).await
            }
        }
    }

    async fn execute_started(
        &self,
        run: &RunRecord,
        parent_run_id: Option<Uuid>,
    ) -> Result<(), AgentError> {
        let resources = self.load_resources(run).await?;
        let skill_assets = self.load_skill_assets(run).await?;
        let assistant_message_id = Uuid::new_v4();
        let observer: Arc<dyn ExecutionObserver> = Arc::new(RepositoryEventSink {
            runs: Arc::clone(&self.deps.runs),
            run_id: run.run_id,
        });
        observer
            .observe(message_started(
                now(),
                assistant_message_id,
                MessageRole::Assistant,
            ))
            .await?;
        let cancellation: Arc<dyn CancellationProbe> = Arc::new(RunCancellationProbe {
            runs: Arc::clone(&self.deps.runs),
            run_id: run.run_id,
            parent_run_id,
        });
        let delegation: Arc<dyn DelegationHandler> = Arc::new(self.clone());
        let execution = self
            .deps
            .executor
            .execute(ExecutionRequest {
                run: run.clone(),
                assistant_message_id,
                resources,
                skill_assets,
                observer,
                cancellation: Arc::clone(&cancellation),
                delegation,
            })
            .await;
        match execution {
            Ok(outcome) => {
                if cancellation.is_cancel_requested().await? {
                    self.finalize_cancelled(run, outcome.external_stop_confirmed)
                        .await
                } else {
                    self.finalize_succeeded(run, assistant_message_id, outcome)
                        .await
                }
            }
            Err(execution_error) => {
                if cancellation.is_cancel_requested().await? {
                    self.finalize_cancelled(run, false).await
                } else {
                    Err(execution_error)
                }
            }
        }
    }

    async fn load_resources(&self, run: &RunRecord) -> Result<Vec<ResourceContent>, AgentError> {
        let mut contents = Vec::with_capacity(run.snapshot.resources.len());
        for snapshot in &run.snapshot.resources {
            let resource = self
                .deps
                .resources
                .get_resource(snapshot.resource_id)
                .await?
                .ok_or(AgentError::NotFound("resource_not_found"))?;
            ensure_resource_owned(&resource, run.owner_user_id)?;
            if resource.organization_id != run.organization_id {
                return Err(AgentError::NotFound("resource_not_found"));
            }
            let bytes = self
                .deps
                .blobs
                .get(&resource.storage_key)
                .await?
                .ok_or(AgentError::NotFound("resource_not_found"))?;
            if sha256_hex(&bytes) != snapshot.sha256 {
                return Err(AgentError::StorageUnavailable);
            }
            contents.push(ResourceContent {
                snapshot: snapshot.clone(),
                bytes,
            });
        }
        Ok(contents)
    }

    async fn load_skill_assets(
        &self,
        run: &RunRecord,
    ) -> Result<Vec<SkillAssetContent>, AgentError> {
        let skill_ids = run
            .snapshot
            .skills
            .iter()
            .map(|skill| skill.skill_id)
            .collect::<Vec<_>>();
        if skill_ids.is_empty() {
            return Ok(Vec::new());
        }
        let stored_assets = self.deps.catalog.list_skill_assets(&skill_ids).await?;
        let mut contents = Vec::new();
        for skill in &run.snapshot.skills {
            for asset_snapshot in &skill.assets {
                let stored_asset = stored_assets
                    .iter()
                    .find(|stored| {
                        stored.asset.id == asset_snapshot.asset_id
                            && stored.asset.skill_id == skill.skill_id
                    })
                    .ok_or(AgentError::NotFound("skill_asset_not_found"))?;
                let bytes = self
                    .deps
                    .blobs
                    .get(&stored_asset.storage_key)
                    .await?
                    .ok_or(AgentError::NotFound("skill_asset_not_found"))?;
                if sha256_hex(&bytes) != asset_snapshot.sha256 {
                    return Err(AgentError::StorageUnavailable);
                }
                contents.push(SkillAssetContent {
                    skill_id: skill.skill_id,
                    name: asset_snapshot.name.clone(),
                    media_type: asset_snapshot.media_type.clone(),
                    bytes,
                });
            }
        }
        Ok(contents)
    }

    async fn finalize_succeeded(
        &self,
        run: &RunRecord,
        message_id: Uuid,
        outcome: ExecutionOutcome,
    ) -> Result<(), AgentError> {
        let current_time = now();
        let mut artifacts = Vec::with_capacity(outcome.artifacts.len());
        for new_artifact in &outcome.artifacts {
            artifacts.push(self.store_artifact(run, new_artifact, current_time).await?);
        }
        let mut events = Vec::with_capacity(artifacts.len() + 2);
        events.push(message_completed(
            current_time,
            message_id,
            &outcome.assistant_content,
        ));
        for artifact in &artifacts {
            events.push(artifact_created(current_time, artifact));
        }
        events.push(run_completed(current_time, message_id));
        ensure_terminal_event(RunStatus::Succeeded, &events)?;
        let assistant_message = ChatMessage {
            id: message_id,
            session_id: run.session_id,
            run_id: Some(run.run_id),
            role: MessageRole::Assistant,
            content: outcome.assistant_content,
            resource_ids: Vec::new(),
            created_at: current_time,
        };
        self.deps
            .runs
            .finalize_run(&FinalizeRunCommand {
                run_id: run.run_id,
                final_status: RunStatus::Succeeded,
                assistant_message: Some(assistant_message),
                artifacts,
                events,
                error_code: None,
                finished_at: current_time,
            })
            .await?;
        Ok(())
    }

    async fn store_artifact(
        &self,
        run: &RunRecord,
        new_artifact: &NewArtifact,
        created_at: time::OffsetDateTime,
    ) -> Result<Artifact, AgentError> {
        validate_artifact(new_artifact, self.deps.config.max_artifact_bytes)?;
        let filename = sanitize_filename(&new_artifact.filename)?;
        let artifact_id = Uuid::new_v4();
        let storage_key = format!(
            "org/{}/runs/{}/artifacts/{artifact_id}",
            run.organization_id, run.run_id
        );
        let sha256 = sha256_hex(&new_artifact.bytes);
        self.deps
            .blobs
            .put(&storage_key, &new_artifact.bytes)
            .await?;
        Ok(Artifact {
            id: artifact_id,
            run_id: run.run_id,
            owner_user_id: run.owner_user_id,
            filename,
            media_type: new_artifact.media_type.clone(),
            size_bytes: i64::try_from(new_artifact.bytes.len())
                .map_err(|_| AgentError::Internal)?,
            sha256,
            storage_key,
            created_at,
        })
    }

    async fn finalize_failed(
        &self,
        run: &RunRecord,
        failure: &AgentError,
    ) -> Result<(), AgentError> {
        let current_time = now();
        let error_code = error_code(failure);
        let events = vec![run_failed(current_time, &error_code)];
        ensure_terminal_event(RunStatus::Failed, &events)?;
        self.deps
            .runs
            .finalize_run(&FinalizeRunCommand {
                run_id: run.run_id,
                final_status: RunStatus::Failed,
                assistant_message: None,
                artifacts: Vec::new(),
                events,
                error_code: Some(error_code),
                finished_at: current_time,
            })
            .await?;
        Ok(())
    }

    async fn finalize_cancelled(
        &self,
        run: &RunRecord,
        external_stop_confirmed: bool,
    ) -> Result<(), AgentError> {
        let current_time = now();
        match self.deps.runs.get_run_status(run.run_id).await? {
            Some(RunStatus::Cancelled) => return Ok(()),
            Some(RunStatus::Running | RunStatus::Queued) => {
                let outcome = self
                    .deps
                    .runs
                    .request_cancel(run.run_id, run.owner_user_id, "executor", current_time)
                    .await?;
                if outcome.run.status == RunStatus::Cancelled {
                    return Ok(());
                }
            }
            _ => {}
        }
        let events = vec![run_cancelled(current_time, external_stop_confirmed)];
        ensure_terminal_event(RunStatus::Cancelled, &events)?;
        self.deps
            .runs
            .finalize_run(&FinalizeRunCommand {
                run_id: run.run_id,
                final_status: RunStatus::Cancelled,
                assistant_message: None,
                artifacts: Vec::new(),
                events,
                error_code: Some("run_cancelled".to_owned()),
                finished_at: current_time,
            })
            .await?;
        Ok(())
    }

    async fn append(&self, run_id: Uuid, draft: RunEventDraft) -> Result<(), AgentError> {
        validate_event(run_id, &draft)?;
        self.deps
            .runs
            .append_events(run_id, &[draft])
            .await
            .map(|_| ())
    }

    async fn reject_delegation(
        &self,
        request: &DelegationRequest,
        rejection: DelegationRejection,
    ) -> Result<(), AgentError> {
        self.append(
            request.parent_run_id,
            delegation_rejected(now(), rejection.reason_code(), &request.tool_call_id),
        )
        .await
    }

    async fn load_child_runtime(
        &self,
        parent: &RunRecord,
        child_agent_id: Uuid,
    ) -> Result<(crate::agent_catalog::BusinessAgent, RuntimeConnection), AgentError> {
        let child_agent = self
            .deps
            .catalog
            .get_agent(child_agent_id)
            .await?
            .ok_or(AgentError::NotFound("agent_not_found"))?;
        if child_agent.organization_id != parent.organization_id || !child_agent.enabled {
            return Err(AgentError::AgentNotRunnable);
        }
        let child_runtime = self
            .deps
            .catalog
            .get_runtime(child_agent.runtime_connection_id)
            .await?
            .ok_or(AgentError::RuntimeUnavailable)?;
        ensure_runtime_ready(&child_runtime)?;
        Ok((child_agent, child_runtime))
    }
}

#[async_trait]
impl DelegationHandler for ExecutionKernel {
    async fn delegate(&self, request: DelegationRequest) -> Result<DelegationResult, AgentError> {
        let parent = self
            .deps
            .runs
            .get_run(request.parent_run_id)
            .await?
            .ok_or(AgentError::NotFound("run_not_found"))?;
        if self.deps.runs.get_run_status(parent.run_id).await? == Some(RunStatus::Cancelling) {
            return Err(AgentError::RunCancelled);
        }
        let Some(delegation) = parent.snapshot.delegation.clone() else {
            self.reject_delegation(&request, DelegationRejection::NotAllowed)
                .await?;
            return Err(AgentError::Forbidden("delegation_not_allowed"));
        };
        let policy = DelegationPolicy {
            parent_agent_id: parent.agent_id,
            child_agent_ids: delegation
                .child_agents
                .iter()
                .map(|child| child.agent_id)
                .collect(),
            max_depth: delegation.max_depth,
            max_child_runs: delegation.max_child_runs,
        };
        let child_depth = parent.delegation_depth.saturating_add(1);
        let existing_child_runs =
            u8::try_from(self.deps.runs.count_child_runs(parent.run_id).await?).unwrap_or(u8::MAX);
        if let Err(rejection) = admit_delegation(
            &policy,
            child_depth,
            existing_child_runs,
            request.child_agent_id,
        ) {
            self.reject_delegation(&request, rejection).await?;
            return Err(AgentError::Forbidden(rejection.reason_code()));
        }
        let (child_agent, child_runtime) = match self
            .load_child_runtime(&parent, request.child_agent_id)
            .await
        {
            Ok(binding) => binding,
            Err(binding_error) => {
                self.reject_delegation(&request, DelegationRejection::NotAllowed)
                    .await?;
                return Err(binding_error);
            }
        };

        let current_time = now();
        let delegation_id = Uuid::new_v4();
        let input_message_id = Uuid::new_v4();
        let routing: RoutingDecision = decide_route(
            Some(child_agent.id),
            vec![RoutingCandidate {
                agent_id: child_agent.id,
                name: child_agent.name.clone(),
                priority: 0,
            }],
        )?;
        let routing = RoutingDecision {
            entry_source: "delegation".to_owned(),
            reason_code: "delegation_policy".to_owned(),
            policy_checksum: delegation.checksum.clone(),
            capability_checksum: child_agent.descriptor_checksum.clone(),
            ..routing
        };
        let child_delegation = self
            .deps
            .delegation_snapshot_for(child_agent.id, parent.organization_id)
            .await?;
        let snapshot = RunSnapshot::build(RunSnapshotInput {
            agent: child_agent.clone(),
            runtime: child_runtime,
            input_message_id,
            input_content: request.input.clone(),
            resources: Vec::new(),
            skills: Vec::new(),
            routing: routing.clone(),
            access_policy_checksum: parent.snapshot.access_policy_checksum.clone(),
            delegation: child_delegation,
            created_at: current_time,
        })?;
        let child_run_id = Uuid::new_v4();
        let initial_events = vec![
            run_created(
                current_time,
                parent.session_id,
                &child_agent,
                snapshot.context_pack_checksum(),
            ),
            input_accepted(current_time, input_message_id, &request.input, &[]),
            routing_selected(current_time, &routing),
        ];
        let new_run = NewRun {
            run_id: child_run_id,
            organization_id: parent.organization_id,
            session_id: parent.session_id,
            owner_user_id: parent.owner_user_id,
            agent_id: child_agent.id,
            runtime_connection_id: child_agent.runtime_connection_id,
            parent_run_id: Some(parent.run_id),
            delegation_id: Some(delegation_id),
            delegation_depth: child_depth,
            origin: RunOrigin::Delegation,
            idempotency_key: format!("delegation:{delegation_id}"),
            request_digest: sha256_json(&(
                parent.run_id,
                delegation_id,
                child_agent.id,
                &request.input,
            ))?,
            snapshot,
            session_title: None,
            created_at: current_time,
        };
        let outcome = self.deps.runs.create_run(&new_run, &initial_events).await?;
        self.append(
            parent.run_id,
            delegation_started(
                current_time,
                delegation_id,
                child_run_id,
                child_agent.id,
                outcome.run.snapshot.agent_snapshot_checksum(),
                outcome.run.snapshot.context_pack_checksum(),
                delegation.max_child_runs,
                DELEGATION_FAILURE_POLICY,
            ),
        )
        .await?;
        let execution_result = self.run(child_run_id, Some(parent.run_id)).await;
        let child_run = self
            .deps
            .runs
            .get_run(child_run_id)
            .await?
            .ok_or(AgentError::Internal)?;
        if let Err(execution_error) = execution_result {
            if child_run.status.is_active() {
                let code = error_code(&execution_error);
                self.append(
                    parent.run_id,
                    delegation_failed(
                        now(),
                        delegation_id,
                        child_run_id,
                        &code,
                        DELEGATION_FAILURE_POLICY,
                    ),
                )
                .await?;
            }
            return Err(execution_error);
        }
        match child_run.status {
            RunStatus::Succeeded => {
                let content = self
                    .deps
                    .conversations
                    .get_assistant_message(child_run_id)
                    .await?
                    .map(|message| message.content)
                    .unwrap_or_default();
                self.append(
                    parent.run_id,
                    delegation_completed(
                        now(),
                        delegation_id,
                        child_run_id,
                        RunStatus::Succeeded,
                        &sha256_hex(content.as_bytes()),
                        DELEGATION_FAILURE_POLICY,
                    ),
                )
                .await?;
                Ok(DelegationResult {
                    child_run_id,
                    status: RunStatus::Succeeded,
                    content: Some(content),
                    error_code: None,
                })
            }
            RunStatus::Cancelled => {
                self.append(
                    parent.run_id,
                    delegation_cancelled(
                        now(),
                        delegation_id,
                        child_run_id,
                        DELEGATION_FAILURE_POLICY,
                    ),
                )
                .await?;
                Ok(DelegationResult {
                    child_run_id,
                    status: RunStatus::Cancelled,
                    content: None,
                    error_code: Some("run_cancelled".to_owned()),
                })
            }
            other_status => {
                let child_error_code = child_run
                    .error_code
                    .clone()
                    .unwrap_or_else(|| "internal_error".to_owned());
                self.append(
                    parent.run_id,
                    delegation_failed(
                        now(),
                        delegation_id,
                        child_run_id,
                        &child_error_code,
                        DELEGATION_FAILURE_POLICY,
                    ),
                )
                .await?;
                Ok(DelegationResult {
                    child_run_id,
                    status: other_status,
                    content: None,
                    error_code: Some(child_error_code),
                })
            }
        }
    }
}

struct RepositoryEventSink {
    runs: Arc<dyn RunRepository>,
    run_id: Uuid,
}

#[async_trait]
impl ExecutionObserver for RepositoryEventSink {
    async fn observe(&self, draft: RunEventDraft) -> Result<(), AgentError> {
        validate_event(self.run_id, &draft)?;
        self.runs
            .append_events(self.run_id, &[draft])
            .await
            .map(|_| ())
    }
}

struct RunCancellationProbe {
    runs: Arc<dyn RunRepository>,
    run_id: Uuid,
    parent_run_id: Option<Uuid>,
}

#[async_trait]
impl CancellationProbe for RunCancellationProbe {
    async fn is_cancel_requested(&self) -> Result<bool, AgentError> {
        if self.runs.get_run_status(self.run_id).await? == Some(RunStatus::Cancelling) {
            return Ok(true);
        }
        if let Some(parent_run_id) = self.parent_run_id
            && self.runs.get_run_status(parent_run_id).await? == Some(RunStatus::Cancelling)
        {
            return Ok(true);
        }
        Ok(false)
    }
}

//! public 用户用例：授权目录、Session/消息、Canonical Run、SSE 事实源、取消、资源与产物。

use std::collections::HashMap;

use serde_json::json;
use uuid::Uuid;

use super::{AgentService, AuditEntry};
use crate::agent_catalog::{AgentAccessRule, AuthorizationBasis, BusinessAgent, authorize_agent};
use crate::audit::AuditOutcome;
use crate::error::AgentError;
use crate::hash::sha256_hex;
use crate::identity::{ServicePrincipal, UserIdentity};
use crate::organization::{Organization, OrganizationMembership};
use crate::resources::{
    Artifact, DownloadedBlob, UploadedFile, UserResource, ensure_resource_owned, resource_snapshot,
    sanitize_filename, validate_resource_upload,
};
use crate::routing::{EntryPolicy, RoutingCandidate, decide_route};
use crate::run::{
    CancelOutcome, CanonicalRunRequest, CreateRunOutcome, NewRun, RunEvent, RunOrigin, RunRecord,
    RunSnapshot, RunSnapshotInput,
};
use crate::run_events::{input_accepted, routing_selected, run_created};
use crate::runtime_catalog::RuntimeConnection;
use crate::session::{
    ChatMessage, ChatSession, DEFAULT_SESSION_TITLE, default_title, ensure_session_owner,
    validate_message_content, validate_reference_count, validate_title,
};
use crate::skill_access::{Skill, resolve_selected_skills};
use crate::time_utils::now;

/// public Agent 目录项。
#[derive(Clone, Debug)]
pub struct AvailableAgent {
    /// Agent descriptor。
    pub agent: BusinessAgent,
    /// Runtime 展示名。
    pub runtime_name: String,
    /// 是否进入 Auto 候选。
    pub auto_visible: bool,
    /// Auto 优先级。
    pub priority: i32,
}

/// 发送消息请求。
#[derive(Clone, Debug)]
pub struct SendMessageRequest {
    /// 显式 Agent；Session 已固定 Agent 时不得冲突。
    pub agent_id: Option<Uuid>,
    /// 输入正文。
    pub content: String,
    /// 引用的用户资源。
    pub resource_ids: Vec<Uuid>,
    /// 选择的 Skill。
    pub skill_ids: Vec<Uuid>,
}

/// Session 详情。
#[derive(Clone, Debug)]
pub struct SessionDetail {
    /// Session。
    pub session: ChatSession,
    /// 用户可见消息。
    pub messages: Vec<ChatMessage>,
    /// Session 内 Run。
    pub runs: Vec<RunRecord>,
}

struct AuthorizedAgent {
    agent: BusinessAgent,
    runtime: RuntimeConnection,
    basis: AuthorizationBasis,
}

impl AgentService {
    async fn authorized_agent_for(
        &self,
        user_id: i64,
        membership: &OrganizationMembership,
        agent: BusinessAgent,
        rules: &[AgentAccessRule],
        runtime: RuntimeConnection,
    ) -> Result<Option<AuthorizedAgent>, AgentError> {
        match authorize_agent(&agent, &runtime, rules, Some(membership), user_id) {
            Ok(basis) => Ok(Some(AuthorizedAgent {
                agent,
                runtime,
                basis,
            })),
            Err(
                AgentError::Forbidden(_)
                | AgentError::AgentNotRunnable
                | AgentError::RuntimeUnavailable,
            ) => Ok(None),
            Err(other_error) => Err(other_error),
        }
    }

    async fn authorized_agents(
        &self,
        user_id: i64,
        organization_id: Uuid,
        membership: &OrganizationMembership,
    ) -> Result<Vec<AuthorizedAgent>, AgentError> {
        let agents = self.deps.catalog.list_agents(organization_id).await?;
        let mut authorized = Vec::new();
        for agent in agents {
            let Some(runtime) = self
                .deps
                .catalog
                .get_runtime(agent.runtime_connection_id)
                .await?
            else {
                continue;
            };
            let rules = self.deps.catalog.list_access_rules(agent.id).await?;
            if let Some(candidate) = self
                .authorized_agent_for(user_id, membership, agent, &rules, runtime)
                .await?
            {
                authorized.push(candidate);
            }
        }
        Ok(authorized)
    }

    async fn explicit_agent(
        &self,
        user_id: i64,
        organization_id: Uuid,
        membership: &OrganizationMembership,
        agent_id: Uuid,
    ) -> Result<AuthorizedAgent, AgentError> {
        let (agent, runtime, rules) = self.agent_bundle(agent_id).await?;
        if agent.organization_id != organization_id {
            return Err(AgentError::NotFound("agent_not_found"));
        }
        let basis = authorize_agent(&agent, &runtime, &rules, Some(membership), user_id)?;
        Ok(AuthorizedAgent {
            agent,
            runtime,
            basis,
        })
    }

    /// 列出用户加入的组织。
    ///
    /// # Errors
    ///
    /// 存储不可用时返回错误。
    pub async fn list_user_organizations(
        &self,
        user: &UserIdentity,
    ) -> Result<Vec<Organization>, AgentError> {
        let memberships = self
            .deps
            .directory
            .list_memberships_for_user(user.user_id)
            .await?;
        let mut organizations = Vec::with_capacity(memberships.len());
        let mut seen = std::collections::BTreeSet::new();
        for membership in memberships {
            if !seen.insert(membership.organization_id) {
                continue;
            }
            if let Some(organization) = self
                .deps
                .directory
                .get_organization(membership.organization_id)
                .await?
            {
                organizations.push(organization);
            }
        }
        Ok(organizations)
    }

    /// 列出用户在组织内可运行的 Agent。
    ///
    /// # Errors
    ///
    /// 非成员或存储不可用时返回错误。
    pub async fn list_available_agents(
        &self,
        user: &UserIdentity,
        organization_id: Uuid,
    ) -> Result<Vec<AvailableAgent>, AgentError> {
        let membership = self.membership(organization_id, user.user_id).await?;
        let policies = self
            .deps
            .catalog
            .list_entry_policies(organization_id)
            .await?;
        let policy_by_agent: HashMap<Uuid, EntryPolicy> = policies
            .into_iter()
            .map(|policy| (policy.agent_id, policy))
            .collect();
        let authorized = self
            .authorized_agents(user.user_id, organization_id, &membership)
            .await?;
        let mut available = authorized
            .into_iter()
            .map(|candidate| {
                let policy = policy_by_agent.get(&candidate.agent.id);
                AvailableAgent {
                    agent: candidate.agent,
                    runtime_name: candidate.runtime.display_name,
                    auto_visible: policy.is_some_and(|entry| entry.auto_visible),
                    priority: policy.map_or(100, |entry| entry.priority),
                }
            })
            .collect::<Vec<_>>();
        available.sort_by(|left, right| left.agent.name.cmp(&right.agent.name));
        Ok(available)
    }

    /// 读取单个 Agent 的 public 目录项。
    ///
    /// # Errors
    ///
    /// Agent 不存在或用户无权运行时返回错误。
    pub async fn get_available_agent(
        &self,
        user: &UserIdentity,
        agent_id: Uuid,
    ) -> Result<AvailableAgent, AgentError> {
        let (agent, runtime, rules) = self.agent_bundle(agent_id).await?;
        let membership = self.membership(agent.organization_id, user.user_id).await?;
        authorize_agent(&agent, &runtime, &rules, Some(&membership), user.user_id)?;
        let policy = self.deps.catalog.get_entry_policy(agent_id).await?;
        Ok(AvailableAgent {
            runtime_name: runtime.display_name,
            auto_visible: policy.as_ref().is_some_and(|entry| entry.auto_visible),
            priority: policy.map_or(100, |entry| entry.priority),
            agent,
        })
    }

    /// 创建 owner-safe Session。
    ///
    /// # Errors
    ///
    /// 非成员、Agent 不可运行或 Auto 没有候选时返回错误。
    pub async fn create_session(
        &self,
        principal: &ServicePrincipal,
        user: &UserIdentity,
        organization_id: Uuid,
        agent_id: Option<Uuid>,
        title: Option<String>,
    ) -> Result<ChatSession, AgentError> {
        let membership = self.membership(organization_id, user.user_id).await?;
        if let Some(agent_id) = agent_id {
            self.explicit_agent(user.user_id, organization_id, &membership, agent_id)
                .await?;
        } else {
            let authorized = self
                .authorized_agents(user.user_id, organization_id, &membership)
                .await?;
            let policies = self
                .deps
                .catalog
                .list_entry_policies(organization_id)
                .await?;
            let auto_ids = policies
                .iter()
                .filter(|policy| policy.auto_visible)
                .map(|policy| policy.agent_id)
                .collect::<std::collections::BTreeSet<_>>();
            if !authorized
                .iter()
                .any(|candidate| auto_ids.contains(&candidate.agent.id))
            {
                return Err(AgentError::NoRunnableAgent);
            }
        }
        let title = match title {
            Some(title) => {
                validate_title(&title)?;
                title
            }
            None => DEFAULT_SESSION_TITLE.to_owned(),
        };
        let current_time = now();
        let session = ChatSession {
            id: Uuid::new_v4(),
            organization_id,
            owner_user_id: user.user_id,
            agent_id,
            title,
            created_at: current_time,
            last_activity_at: current_time,
        };
        self.deps.conversations.insert_session(&session).await?;
        self.write_audit(
            &principal.service_id,
            AuditEntry {
                admin: None,
                user_id: Some(user.user_id),
                organization_id: Some(organization_id),
                action: "session.create",
                target_id: Some(session.id.to_string()),
                outcome: AuditOutcome::Allowed,
                detail: json!({"agent_id": agent_id}),
            },
        )
        .await;
        Ok(session)
    }

    /// 列出用户在组织内的 Session。
    ///
    /// # Errors
    ///
    /// 非成员或存储不可用时返回错误。
    pub async fn list_sessions(
        &self,
        user: &UserIdentity,
        organization_id: Uuid,
    ) -> Result<Vec<ChatSession>, AgentError> {
        self.membership(organization_id, user.user_id).await?;
        self.deps
            .conversations
            .list_sessions(organization_id, user.user_id)
            .await
    }

    /// 读取 Session、消息与 Run 历史。
    ///
    /// # Errors
    ///
    /// 非所有者按不存在处理。
    pub async fn get_session_detail(
        &self,
        user: &UserIdentity,
        session_id: Uuid,
    ) -> Result<SessionDetail, AgentError> {
        let session = self
            .deps
            .conversations
            .get_session(session_id)
            .await?
            .ok_or(AgentError::NotFound("session_not_found"))?;
        ensure_session_owner(&session, user.user_id)?;
        let messages = self
            .deps
            .conversations
            .list_visible_messages(session_id)
            .await?;
        let runs = self
            .deps
            .conversations
            .list_session_runs(session_id, user.user_id)
            .await?;
        Ok(SessionDetail {
            session,
            messages,
            runs,
        })
    }

    /// 更新 Session 标题。
    ///
    /// # Errors
    ///
    /// 非所有者或标题非法时返回错误。
    pub async fn update_session_title(
        &self,
        user: &UserIdentity,
        session_id: Uuid,
        title: &str,
    ) -> Result<ChatSession, AgentError> {
        validate_title(title)?;
        let session = self
            .deps
            .conversations
            .get_session(session_id)
            .await?
            .ok_or(AgentError::NotFound("session_not_found"))?;
        ensure_session_owner(&session, user.user_id)?;
        self.deps
            .conversations
            .update_session_title(session_id, user.user_id, title)
            .await?;
        let mut updated_session = session;
        updated_session.title = title.to_owned();
        Ok(updated_session)
    }

    /// 发送消息并创建 Canonical Run；相同幂等键与请求重放原 Run。
    ///
    /// # Errors
    ///
    /// 非所有者、非成员、Agent 未授权、Runtime 不可用、幂等键冲突或存储失败时返回错误。
    pub async fn send_message(
        &self,
        principal: &ServicePrincipal,
        user: &UserIdentity,
        session_id: Uuid,
        idempotency_key: &str,
        request: SendMessageRequest,
    ) -> Result<CreateRunOutcome, AgentError> {
        CanonicalRunRequest::validate_idempotency_key(idempotency_key)?;
        validate_message_content(&request.content)?;
        validate_reference_count(request.resource_ids.len(), request.skill_ids.len())?;
        ensure_no_duplicates(&request.resource_ids, "资源引用")?;
        ensure_no_duplicates(&request.skill_ids, "Skill 选择")?;

        let session = self
            .deps
            .conversations
            .get_session(session_id)
            .await?
            .ok_or(AgentError::NotFound("session_not_found"))?;
        ensure_session_owner(&session, user.user_id)?;
        let membership = self
            .membership(session.organization_id, user.user_id)
            .await?;

        let requested_agent_id = match session.agent_id {
            Some(session_agent_id) => {
                if request
                    .agent_id
                    .is_some_and(|agent_id| agent_id != session_agent_id)
                {
                    return Err(AgentError::InvalidRequest(
                        "请求 Agent 与 Session 固定 Agent 不一致".to_owned(),
                    ));
                }
                Some(session_agent_id)
            }
            None => request.agent_id,
        };

        let (selected, decision) = match requested_agent_id {
            Some(agent_id) => {
                let selected = self
                    .explicit_agent(user.user_id, session.organization_id, &membership, agent_id)
                    .await?;
                let decision = decide_route(
                    Some(agent_id),
                    vec![RoutingCandidate {
                        agent_id,
                        name: selected.agent.name.clone(),
                        priority: 0,
                    }],
                )?;
                (selected, decision)
            }
            None => {
                let authorized = self
                    .authorized_agents(user.user_id, session.organization_id, &membership)
                    .await?;
                let policies = self
                    .deps
                    .catalog
                    .list_entry_policies(session.organization_id)
                    .await?;
                let candidates = auto_candidates(&authorized, &policies);
                let decision = decide_route(None, candidates)?;
                let selected = authorized
                    .into_iter()
                    .find(|candidate| candidate.agent.id == decision.selected_agent_id)
                    .ok_or(AgentError::NoRunnableAgent)?;
                (selected, decision)
            }
        };

        let mut resources = Vec::with_capacity(request.resource_ids.len());
        for resource_id in &request.resource_ids {
            let resource = self
                .deps
                .resources
                .get_resource(*resource_id)
                .await?
                .ok_or(AgentError::NotFound("resource_not_found"))?;
            if resource.organization_id != session.organization_id {
                return Err(AgentError::NotFound("resource_not_found"));
            }
            ensure_resource_owned(&resource, user.user_id)?;
            resources.push(resource_snapshot(&resource));
        }

        let assigned_skill_ids = self
            .deps
            .catalog
            .list_skill_assignments(selected.agent.id)
            .await?;
        let mut selected_skills = Vec::with_capacity(request.skill_ids.len());
        for skill_id in &request.skill_ids {
            let skill = self
                .deps
                .catalog
                .get_skill(*skill_id)
                .await?
                .ok_or(AgentError::NotFound("skill_not_found"))?;
            selected_skills.push(skill);
        }
        let skill_assets = if request.skill_ids.is_empty() {
            Vec::new()
        } else {
            self.deps
                .catalog
                .list_skill_assets(&request.skill_ids)
                .await?
                .into_iter()
                .map(|stored| stored.asset)
                .collect()
        };
        let skill_snapshots = resolve_selected_skills(
            session.organization_id,
            &request.skill_ids,
            &assigned_skill_ids,
            &selected_skills,
            &skill_assets,
        )?;

        let delegation_snapshot = self
            .deps
            .delegation_snapshot_for(selected.agent.id, session.organization_id)
            .await?;

        let input_message_id = Uuid::new_v4();
        let current_time = now();
        let snapshot = RunSnapshot::build(RunSnapshotInput {
            agent: selected.agent.clone(),
            runtime: selected.runtime.clone(),
            input_message_id,
            input_content: request.content.clone(),
            resources,
            skills: skill_snapshots,
            routing: decision.clone(),
            access_policy_checksum: selected.basis.policy_checksum.clone(),
            delegation: delegation_snapshot,
            created_at: current_time,
        })?;
        let canonical_request = CanonicalRunRequest {
            session_id,
            requested_agent_id,
            content: request.content.clone(),
            resource_ids: request.resource_ids.clone(),
            skill_ids: request.skill_ids.clone(),
        };
        let request_digest = canonical_request.digest()?;
        let initial_events = vec![
            run_created(
                current_time,
                session.id,
                &selected.agent,
                snapshot.context_pack_checksum(),
            ),
            input_accepted(
                current_time,
                input_message_id,
                &request.content,
                &snapshot.resources,
            ),
            routing_selected(current_time, &decision),
        ];
        let new_run = NewRun {
            run_id: Uuid::new_v4(),
            organization_id: session.organization_id,
            session_id,
            owner_user_id: user.user_id,
            agent_id: selected.agent.id,
            runtime_connection_id: selected.runtime.id,
            parent_run_id: None,
            delegation_id: None,
            delegation_depth: 0,
            origin: if requested_agent_id.is_some() {
                RunOrigin::Direct
            } else {
                RunOrigin::Auto
            },
            idempotency_key: idempotency_key.to_owned(),
            request_digest,
            snapshot,
            session_title: (session.title == DEFAULT_SESSION_TITLE)
                .then(|| default_title(&request.content)),
            created_at: current_time,
        };
        let outcome = self.deps.runs.create_run(&new_run, &initial_events).await?;
        if outcome.created {
            self.write_audit(
                &principal.service_id,
                AuditEntry {
                    admin: None,
                    user_id: Some(user.user_id),
                    organization_id: Some(session.organization_id),
                    action: "run.create",
                    target_id: Some(outcome.run.run_id.to_string()),
                    outcome: AuditOutcome::Allowed,
                    detail: json!({
                        "agent_id": outcome.run.agent_id,
                        "origin": outcome.run.origin.as_str(),
                    }),
                },
            )
            .await;
        }
        Ok(outcome)
    }

    /// 读取 Run；非所有者按不存在处理。
    ///
    /// # Errors
    ///
    /// Run 不存在或不属于当前用户时返回错误。
    pub async fn get_run(
        &self,
        user: &UserIdentity,
        run_id: Uuid,
    ) -> Result<RunRecord, AgentError> {
        let run = self
            .deps
            .runs
            .get_run(run_id)
            .await?
            .ok_or(AgentError::NotFound("run_not_found"))?;
        ensure_run_owner(&run, user.user_id)?;
        Ok(run)
    }

    /// 读取持久事件；SSE 与轮询共用同一事实源。
    ///
    /// # Errors
    ///
    /// Run 不存在、非所有者或参数非法时返回错误。
    pub async fn list_run_events(
        &self,
        user: &UserIdentity,
        run_id: Uuid,
        after_seq: i64,
        limit: usize,
    ) -> Result<Vec<RunEvent>, AgentError> {
        if after_seq < 0 {
            return Err(AgentError::InvalidRequest(
                "after_seq 不能为负数".to_owned(),
            ));
        }
        let run = self.get_run(user, run_id).await?;
        let limit = limit.clamp(1, 200);
        self.deps
            .runs
            .list_run_events(run.run_id, after_seq, limit)
            .await
    }

    /// 幂等请求协作式取消。
    ///
    /// # Errors
    ///
    /// Run 不存在或不属于当前用户时返回错误。
    pub async fn cancel_run(
        &self,
        principal: &ServicePrincipal,
        user: &UserIdentity,
        run_id: Uuid,
    ) -> Result<CancelOutcome, AgentError> {
        let run = self.get_run(user, run_id).await?;
        let outcome = self
            .deps
            .runs
            .request_cancel(run.run_id, user.user_id, "owner", now())
            .await?;
        self.write_audit(
            &principal.service_id,
            AuditEntry {
                admin: None,
                user_id: Some(user.user_id),
                organization_id: Some(run.organization_id),
                action: "run.cancel",
                target_id: Some(run.run_id.to_string()),
                outcome: AuditOutcome::Allowed,
                detail: json!({"immediate": outcome.cancelled_immediately}),
            },
        )
        .await;
        Ok(outcome)
    }

    /// 列出 Run 产物。
    ///
    /// # Errors
    ///
    /// Run 不存在或不属于当前用户时返回错误。
    pub async fn list_artifacts(
        &self,
        user: &UserIdentity,
        run_id: Uuid,
    ) -> Result<Vec<Artifact>, AgentError> {
        let run = self.get_run(user, run_id).await?;
        self.deps.resources.list_artifacts_for_run(run.run_id).await
    }

    /// 下载产物。
    ///
    /// # Errors
    ///
    /// 产物不存在、Run 非所有者或 Blob 缺失时返回错误。
    pub async fn download_artifact(
        &self,
        user: &UserIdentity,
        artifact_id: Uuid,
    ) -> Result<DownloadedBlob, AgentError> {
        let artifact = self
            .deps
            .resources
            .get_artifact(artifact_id)
            .await?
            .ok_or(AgentError::NotFound("artifact_not_found"))?;
        let run = self
            .deps
            .runs
            .get_run(artifact.run_id)
            .await?
            .ok_or(AgentError::NotFound("artifact_not_found"))?;
        ensure_run_owner(&run, user.user_id)?;
        let bytes = self
            .deps
            .blobs
            .get(&artifact.storage_key)
            .await?
            .ok_or(AgentError::NotFound("artifact_not_found"))?;
        Ok(DownloadedBlob {
            filename: artifact.filename,
            media_type: artifact.media_type,
            bytes,
        })
    }

    /// 上传用户资源。
    ///
    /// # Errors
    ///
    /// 非成员、文件类型/大小非法或 Blob 写入失败时返回错误。
    pub async fn upload_resource(
        &self,
        principal: &ServicePrincipal,
        user: &UserIdentity,
        organization_id: Uuid,
        file: UploadedFile,
    ) -> Result<UserResource, AgentError> {
        self.membership(organization_id, user.user_id).await?;
        validate_resource_upload(
            &file.filename,
            &file.media_type,
            file.bytes.len(),
            self.deps.config.max_resource_bytes,
        )?;
        let filename = sanitize_filename(&file.filename)?;
        let resource_id = Uuid::new_v4();
        let storage_key = format!(
            "org/{organization_id}/users/{}/resources/{resource_id}",
            user.user_id
        );
        let sha256 = sha256_hex(&file.bytes);
        self.deps.blobs.put(&storage_key, &file.bytes).await?;
        let resource = UserResource {
            id: resource_id,
            organization_id,
            owner_user_id: user.user_id,
            filename,
            media_type: file.media_type,
            size_bytes: i64::try_from(file.bytes.len()).map_err(|_| AgentError::Internal)?,
            sha256,
            storage_key,
            created_at: now(),
            deleted_at: None,
        };
        if let Err(insert_error) = self.deps.resources.insert_resource(&resource).await {
            let _ = self.deps.blobs.delete(&resource.storage_key).await;
            return Err(insert_error);
        }
        self.write_audit(
            &principal.service_id,
            AuditEntry {
                admin: None,
                user_id: Some(user.user_id),
                organization_id: Some(organization_id),
                action: "resource.upload",
                target_id: Some(resource.id.to_string()),
                outcome: AuditOutcome::Allowed,
                detail: json!({"size": resource.size_bytes, "media_type": resource.media_type}),
            },
        )
        .await;
        Ok(resource)
    }

    /// 列出用户资源。
    ///
    /// # Errors
    ///
    /// 非成员或存储不可用时返回错误。
    pub async fn list_resources(
        &self,
        user: &UserIdentity,
        organization_id: Uuid,
    ) -> Result<Vec<UserResource>, AgentError> {
        self.membership(organization_id, user.user_id).await?;
        self.deps
            .resources
            .list_resources(organization_id, user.user_id)
            .await
    }

    /// 下载用户资源。
    ///
    /// # Errors
    ///
    /// 资源不存在、非所有者或 Blob 缺失时返回错误。
    pub async fn download_resource(
        &self,
        user: &UserIdentity,
        resource_id: Uuid,
    ) -> Result<DownloadedBlob, AgentError> {
        let resource = self
            .deps
            .resources
            .get_resource(resource_id)
            .await?
            .ok_or(AgentError::NotFound("resource_not_found"))?;
        ensure_resource_owned(&resource, user.user_id)?;
        let bytes = self
            .deps
            .blobs
            .get(&resource.storage_key)
            .await?
            .ok_or(AgentError::NotFound("resource_not_found"))?;
        Ok(DownloadedBlob {
            filename: resource.filename,
            media_type: resource.media_type,
            bytes,
        })
    }

    /// 软删除用户资源。
    ///
    /// # Errors
    ///
    /// 资源不存在或非所有者时返回错误。
    pub async fn delete_resource(
        &self,
        principal: &ServicePrincipal,
        user: &UserIdentity,
        resource_id: Uuid,
    ) -> Result<(), AgentError> {
        let resource = self
            .deps
            .resources
            .get_resource(resource_id)
            .await?
            .ok_or(AgentError::NotFound("resource_not_found"))?;
        ensure_resource_owned(&resource, user.user_id)?;
        self.deps
            .resources
            .soft_delete_resource(resource.organization_id, user.user_id, resource.id)
            .await?;
        self.write_audit(
            &principal.service_id,
            AuditEntry {
                admin: None,
                user_id: Some(user.user_id),
                organization_id: Some(resource.organization_id),
                action: "resource.delete",
                target_id: Some(resource.id.to_string()),
                outcome: AuditOutcome::Allowed,
                detail: json!({}),
            },
        )
        .await;
        Ok(())
    }

    /// 列出用户可选择的 Skill。
    ///
    /// # Errors
    ///
    /// Agent 未授权、非成员或存储不可用时返回错误。
    pub async fn list_skills_for_agent(
        &self,
        user: &UserIdentity,
        agent_id: Uuid,
    ) -> Result<Vec<Skill>, AgentError> {
        let (agent, runtime, rules) = self.agent_bundle(agent_id).await?;
        let membership = self.membership(agent.organization_id, user.user_id).await?;
        authorize_agent(&agent, &runtime, &rules, Some(&membership), user.user_id)?;
        let assigned = self.deps.catalog.list_skill_assignments(agent_id).await?;
        let mut skills = Vec::new();
        for skill_id in assigned {
            if let Some(skill) = self.deps.catalog.get_skill(skill_id).await?
                && skill.enabled
                && skill.organization_id == agent.organization_id
            {
                skills.push(skill);
            }
        }
        Ok(skills)
    }

    /// 读取 Agent 授权预览，供工作台展示。
    ///
    /// # Errors
    ///
    /// Agent 不存在或用户无权运行时返回错误。
    pub async fn preview_agent(
        &self,
        user: &UserIdentity,
        agent_id: Uuid,
    ) -> Result<AvailableAgent, AgentError> {
        self.get_available_agent(user, agent_id).await
    }
}

fn ensure_no_duplicates(identifiers: &[Uuid], field_name: &str) -> Result<(), AgentError> {
    let mut unique = identifiers.to_vec();
    unique.sort_unstable();
    unique.dedup();
    if unique.len() != identifiers.len() {
        return Err(AgentError::InvalidRequest(format!(
            "{field_name}包含重复项"
        )));
    }
    Ok(())
}

fn auto_candidates(
    authorized: &[AuthorizedAgent],
    policies: &[EntryPolicy],
) -> Vec<RoutingCandidate> {
    let auto_ids = policies
        .iter()
        .filter(|policy| policy.auto_visible)
        .map(|policy| (policy.agent_id, policy.priority))
        .collect::<HashMap<_, _>>();
    authorized
        .iter()
        .filter_map(|candidate| {
            auto_ids
                .get(&candidate.agent.id)
                .map(|priority| RoutingCandidate {
                    agent_id: candidate.agent.id,
                    name: candidate.agent.name.clone(),
                    priority: *priority,
                })
        })
        .collect()
}

fn ensure_run_owner(run: &RunRecord, user_id: i64) -> Result<(), AgentError> {
    if run.owner_user_id == user_id {
        Ok(())
    } else {
        Err(AgentError::NotFound("run_not_found"))
    }
}

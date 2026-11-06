//! 组织管理路由：组织/部门/成员/scope、Runtime、Agent、策略、Skill 与审计。

use axum::Json;
use axum::Router;
use axum::extract::{DefaultBodyLimit, Path, State};
use axum::http::StatusCode;
use axum::routing::{get, post, put};
use uuid::Uuid;

use tpl_agent_core::agent_catalog::{AgentAccessRule, AgentUpdate, NewAgent};
use tpl_agent_core::delegation::DelegationPolicyUpdate;
use tpl_agent_core::organization::{
    AdminScope, Organization, OrganizationMembership, OrganizationUnit,
};
use tpl_agent_core::routing::EntryPolicy;
use tpl_agent_core::runtime_catalog::{NewExternalRuntime, RuntimeConnection};
use tpl_agent_core::skill_access::{NewSkill, Skill, SkillAsset, SkillUpdate};

use crate::dto::{
    AdminIdentityResponse, AgentOverviewResponse, CreateAgentRequest, CreateOrganizationRequest,
    CreateRuntimeRequest, CreateSkillRequest, CreateUnitRequest, DelegationPolicyRequest,
    EntryPolicyRequest, GrantAdminScopeRequest, ListResponse, ReplaceAccessRulesRequest,
    ReplaceAgentSkillsRequest, UpdateAgentRequest, UpdateSkillRequest, UpsertMembershipRequest,
};
use crate::error::ApiError;
use crate::extract::AdminCall;
use crate::routes::files::{download_response, single_file};
use crate::server::AgentApiState;

pub(crate) fn routes(max_upload_limit: usize) -> Router<AgentApiState> {
    Router::new()
        .route("/admin/api/me", get(current_admin))
        .route(
            "/admin/api/organizations",
            get(list_organizations).post(create_organization),
        )
        .route(
            "/admin/api/organizations/{organization_id}",
            get(get_organization),
        )
        .route(
            "/admin/api/organizations/{organization_id}/units",
            get(list_units).post(create_unit),
        )
        .route(
            "/admin/api/organizations/{organization_id}/units/{unit_id}",
            axum::routing::delete(delete_unit),
        )
        .route(
            "/admin/api/organizations/{organization_id}/memberships",
            get(list_memberships).post(upsert_membership),
        )
        .route(
            "/admin/api/organizations/{organization_id}/memberships/{user_id}",
            axum::routing::delete(remove_membership),
        )
        .route(
            "/admin/api/organizations/{organization_id}/admin-scopes",
            get(list_admin_scopes).post(grant_admin_scope),
        )
        .route(
            "/admin/api/organizations/{organization_id}/admin-scopes/{admin_user_id}",
            axum::routing::delete(revoke_admin_scope),
        )
        .route(
            "/admin/api/organizations/{organization_id}/runtimes",
            get(list_runtimes).post(create_external_runtime),
        )
        .route(
            "/admin/api/organizations/{organization_id}/runtimes/{runtime_id}/refresh",
            post(refresh_runtime),
        )
        .route(
            "/admin/api/organizations/{organization_id}/agents",
            get(list_agents).post(create_agent),
        )
        .route(
            "/admin/api/organizations/{organization_id}/agents/{agent_id}",
            get(get_agent).put(update_agent),
        )
        .route(
            "/admin/api/organizations/{organization_id}/agents/{agent_id}/overview",
            get(get_agent_overview),
        )
        .route(
            "/admin/api/organizations/{organization_id}/agents/{agent_id}/access-rules",
            get(list_access_rules).put(replace_access_rules),
        )
        .route(
            "/admin/api/organizations/{organization_id}/agents/{agent_id}/entry-policy",
            get(get_entry_policy).put(upsert_entry_policy),
        )
        .route(
            "/admin/api/organizations/{organization_id}/agents/{agent_id}/delegation-policy",
            get(get_delegation_policy).put(upsert_delegation_policy),
        )
        .route(
            "/admin/api/organizations/{organization_id}/agents/{agent_id}/skills",
            get(list_agent_skills).put(replace_agent_skills),
        )
        .route(
            "/admin/api/organizations/{organization_id}/skills",
            get(list_skills).post(create_skill),
        )
        .route(
            "/admin/api/organizations/{organization_id}/skills/{skill_id}",
            put(update_skill),
        )
        .route(
            "/admin/api/organizations/{organization_id}/skills/{skill_id}/assets",
            get(list_skill_assets)
                .post(upload_skill_asset)
                .layer(DefaultBodyLimit::max(max_upload_limit)),
        )
        .route(
            "/admin/api/organizations/{organization_id}/skills/{skill_id}/assets/{asset_id}",
            get(download_skill_asset).delete(delete_skill_asset),
        )
        .route(
            "/admin/api/organizations/{organization_id}/audit",
            get(list_audit),
        )
}

async fn current_admin(
    State(state): State<AgentApiState>,
    call: AdminCall,
) -> Json<AdminIdentityResponse> {
    Json(AdminIdentityResponse {
        admin_user_id: call.admin.admin_user_id,
        display_name: call.admin.display_name.clone(),
        platform_admin: state.service.is_platform_admin(&call.admin),
    })
}

async fn create_organization(
    State(state): State<AgentApiState>,
    call: AdminCall,
    Json(request): Json<CreateOrganizationRequest>,
) -> Result<(StatusCode, Json<Organization>), ApiError> {
    let organization = state
        .service
        .create_organization(
            &call.principal,
            &call.admin,
            &request.slug,
            &request.display_name,
        )
        .await?;
    Ok((StatusCode::CREATED, Json(organization)))
}

async fn list_organizations(
    State(state): State<AgentApiState>,
    call: AdminCall,
) -> Result<Json<ListResponse<Organization>>, ApiError> {
    let organizations = state.service.list_organizations(&call.admin).await?;
    Ok(Json(ListResponse::new(organizations)))
}

async fn get_organization(
    State(state): State<AgentApiState>,
    call: AdminCall,
    Path(organization_id): Path<Uuid>,
) -> Result<Json<Organization>, ApiError> {
    let organization = state
        .service
        .get_organization(&call.admin, organization_id)
        .await?;
    Ok(Json(organization))
}

async fn create_unit(
    State(state): State<AgentApiState>,
    call: AdminCall,
    Path(organization_id): Path<Uuid>,
    Json(request): Json<CreateUnitRequest>,
) -> Result<(StatusCode, Json<OrganizationUnit>), ApiError> {
    let unit = state
        .service
        .create_unit(&call.principal, &call.admin, organization_id, &request.name)
        .await?;
    Ok((StatusCode::CREATED, Json(unit)))
}

async fn list_units(
    State(state): State<AgentApiState>,
    call: AdminCall,
    Path(organization_id): Path<Uuid>,
) -> Result<Json<ListResponse<OrganizationUnit>>, ApiError> {
    let units = state
        .service
        .list_units(&call.admin, organization_id)
        .await?;
    Ok(Json(ListResponse::new(units)))
}

async fn delete_unit(
    State(state): State<AgentApiState>,
    call: AdminCall,
    Path((organization_id, unit_id)): Path<(Uuid, Uuid)>,
) -> Result<StatusCode, ApiError> {
    state
        .service
        .delete_unit(&call.principal, &call.admin, organization_id, unit_id)
        .await?;
    Ok(StatusCode::NO_CONTENT)
}

async fn upsert_membership(
    State(state): State<AgentApiState>,
    call: AdminCall,
    Path(organization_id): Path<Uuid>,
    Json(request): Json<UpsertMembershipRequest>,
) -> Result<Json<OrganizationMembership>, ApiError> {
    let membership = state
        .service
        .upsert_membership(
            &call.principal,
            &call.admin,
            organization_id,
            request.user_id,
            request.unit_id,
            request.display_name.as_deref(),
        )
        .await?;
    Ok(Json(membership))
}

async fn list_memberships(
    State(state): State<AgentApiState>,
    call: AdminCall,
    Path(organization_id): Path<Uuid>,
) -> Result<Json<ListResponse<OrganizationMembership>>, ApiError> {
    let memberships = state
        .service
        .list_memberships(&call.admin, organization_id)
        .await?;
    Ok(Json(ListResponse::new(memberships)))
}

async fn remove_membership(
    State(state): State<AgentApiState>,
    call: AdminCall,
    Path((organization_id, user_id)): Path<(Uuid, i64)>,
) -> Result<StatusCode, ApiError> {
    state
        .service
        .remove_membership(&call.principal, &call.admin, organization_id, user_id)
        .await?;
    Ok(StatusCode::NO_CONTENT)
}

async fn list_admin_scopes(
    State(state): State<AgentApiState>,
    call: AdminCall,
    Path(organization_id): Path<Uuid>,
) -> Result<Json<ListResponse<AdminScope>>, ApiError> {
    let scopes = state
        .service
        .list_admin_scopes(&call.admin, organization_id)
        .await?;
    Ok(Json(ListResponse::new(scopes)))
}

async fn grant_admin_scope(
    State(state): State<AgentApiState>,
    call: AdminCall,
    Path(organization_id): Path<Uuid>,
    Json(request): Json<GrantAdminScopeRequest>,
) -> Result<(StatusCode, Json<AdminScope>), ApiError> {
    let scope = state
        .service
        .grant_admin_scope(
            &call.principal,
            &call.admin,
            organization_id,
            request.admin_user_id,
        )
        .await?;
    Ok((StatusCode::CREATED, Json(scope)))
}

async fn revoke_admin_scope(
    State(state): State<AgentApiState>,
    call: AdminCall,
    Path((organization_id, admin_user_id)): Path<(Uuid, i64)>,
) -> Result<StatusCode, ApiError> {
    state
        .service
        .revoke_admin_scope(&call.principal, &call.admin, organization_id, admin_user_id)
        .await?;
    Ok(StatusCode::NO_CONTENT)
}

async fn list_runtimes(
    State(state): State<AgentApiState>,
    call: AdminCall,
    Path(organization_id): Path<Uuid>,
) -> Result<Json<ListResponse<RuntimeConnection>>, ApiError> {
    let runtimes = state
        .service
        .list_runtimes(&call.admin, organization_id)
        .await?;
    Ok(Json(ListResponse::new(runtimes)))
}

async fn create_external_runtime(
    State(state): State<AgentApiState>,
    call: AdminCall,
    Path(organization_id): Path<Uuid>,
    Json(request): Json<CreateRuntimeRequest>,
) -> Result<(StatusCode, Json<RuntimeConnection>), ApiError> {
    let runtime = state
        .service
        .create_external_runtime(
            &call.principal,
            &call.admin,
            organization_id,
            NewExternalRuntime {
                name: request.name,
                display_name: request.display_name,
                endpoint_url: request.endpoint_url,
                credential_alias: request.credential_alias,
            },
        )
        .await?;
    Ok((StatusCode::CREATED, Json(runtime)))
}

async fn refresh_runtime(
    State(state): State<AgentApiState>,
    call: AdminCall,
    Path((organization_id, runtime_id)): Path<(Uuid, Uuid)>,
) -> Result<Json<RuntimeConnection>, ApiError> {
    let runtime = state
        .service
        .refresh_runtime(&call.principal, &call.admin, organization_id, runtime_id)
        .await?;
    Ok(Json(runtime))
}

async fn list_agents(
    State(state): State<AgentApiState>,
    call: AdminCall,
    Path(organization_id): Path<Uuid>,
) -> Result<Json<ListResponse<tpl_agent_core::agent_catalog::BusinessAgent>>, ApiError> {
    let agents = state
        .service
        .list_agents(&call.admin, organization_id)
        .await?;
    Ok(Json(ListResponse::new(agents)))
}

async fn get_agent(
    State(state): State<AgentApiState>,
    call: AdminCall,
    Path((organization_id, agent_id)): Path<(Uuid, Uuid)>,
) -> Result<Json<tpl_agent_core::agent_catalog::BusinessAgent>, ApiError> {
    let agent = state
        .service
        .get_agent(&call.admin, organization_id, agent_id)
        .await?;
    Ok(Json(agent))
}

async fn create_agent(
    State(state): State<AgentApiState>,
    call: AdminCall,
    Path(organization_id): Path<Uuid>,
    Json(request): Json<CreateAgentRequest>,
) -> Result<
    (
        StatusCode,
        Json<tpl_agent_core::agent_catalog::BusinessAgent>,
    ),
    ApiError,
> {
    let agent = state
        .service
        .create_agent(
            &call.principal,
            &call.admin,
            organization_id,
            NewAgent {
                organization_id,
                runtime_connection_id: request.runtime_connection_id,
                name: request.name,
                display_name: request.display_name,
                description: request.description,
                prompt: request.prompt,
                owner_admin_id: call.admin.admin_user_id,
            },
        )
        .await?;
    Ok((StatusCode::CREATED, Json(agent)))
}

async fn update_agent(
    State(state): State<AgentApiState>,
    call: AdminCall,
    Path((organization_id, agent_id)): Path<(Uuid, Uuid)>,
    Json(request): Json<UpdateAgentRequest>,
) -> Result<Json<tpl_agent_core::agent_catalog::BusinessAgent>, ApiError> {
    let agent = state
        .service
        .update_agent(
            &call.principal,
            &call.admin,
            organization_id,
            agent_id,
            AgentUpdate {
                display_name: request.display_name,
                description: request.description,
                prompt: request.prompt,
                enabled: request.enabled,
            },
        )
        .await?;
    Ok(Json(agent))
}

async fn get_agent_overview(
    State(state): State<AgentApiState>,
    call: AdminCall,
    Path((organization_id, agent_id)): Path<(Uuid, Uuid)>,
) -> Result<Json<AgentOverviewResponse>, ApiError> {
    let agent = state
        .service
        .get_agent(&call.admin, organization_id, agent_id)
        .await?;
    let access_rules = state
        .service
        .list_agent_access_rules(&call.admin, organization_id, agent_id)
        .await?;
    let entry_policy = state
        .service
        .get_agent_entry_policy(&call.admin, organization_id, agent_id)
        .await?;
    let delegation_policy = state
        .service
        .get_agent_delegation_policy(&call.admin, organization_id, agent_id)
        .await?;
    let skill_ids = state
        .service
        .list_agent_skill_ids(&call.admin, organization_id, agent_id)
        .await?;
    Ok(Json(AgentOverviewResponse {
        agent,
        access_rules,
        entry_policy,
        delegation_policy,
        skill_ids,
    }))
}

async fn list_access_rules(
    State(state): State<AgentApiState>,
    call: AdminCall,
    Path((organization_id, agent_id)): Path<(Uuid, Uuid)>,
) -> Result<Json<ListResponse<AgentAccessRule>>, ApiError> {
    let rules = state
        .service
        .list_agent_access_rules(&call.admin, organization_id, agent_id)
        .await?;
    Ok(Json(ListResponse::new(rules)))
}

async fn replace_access_rules(
    State(state): State<AgentApiState>,
    call: AdminCall,
    Path((organization_id, agent_id)): Path<(Uuid, Uuid)>,
    Json(request): Json<ReplaceAccessRulesRequest>,
) -> Result<Json<ListResponse<AgentAccessRule>>, ApiError> {
    let rules = state
        .service
        .replace_agent_access_rules(
            &call.principal,
            &call.admin,
            organization_id,
            agent_id,
            request.rules,
        )
        .await?;
    Ok(Json(ListResponse::new(rules)))
}

async fn get_entry_policy(
    State(state): State<AgentApiState>,
    call: AdminCall,
    Path((organization_id, agent_id)): Path<(Uuid, Uuid)>,
) -> Result<Json<Option<EntryPolicy>>, ApiError> {
    let policy = state
        .service
        .get_agent_entry_policy(&call.admin, organization_id, agent_id)
        .await?;
    Ok(Json(policy))
}

async fn upsert_entry_policy(
    State(state): State<AgentApiState>,
    call: AdminCall,
    Path((organization_id, agent_id)): Path<(Uuid, Uuid)>,
    Json(request): Json<EntryPolicyRequest>,
) -> Result<Json<EntryPolicy>, ApiError> {
    let policy = state
        .service
        .upsert_entry_policy(
            &call.principal,
            &call.admin,
            organization_id,
            agent_id,
            request.auto_visible,
            request.priority,
        )
        .await?;
    Ok(Json(policy))
}

async fn get_delegation_policy(
    State(state): State<AgentApiState>,
    call: AdminCall,
    Path((organization_id, agent_id)): Path<(Uuid, Uuid)>,
) -> Result<Json<Option<tpl_agent_core::delegation::DelegationPolicy>>, ApiError> {
    let policy = state
        .service
        .get_agent_delegation_policy(&call.admin, organization_id, agent_id)
        .await?;
    Ok(Json(policy))
}

async fn upsert_delegation_policy(
    State(state): State<AgentApiState>,
    call: AdminCall,
    Path((organization_id, agent_id)): Path<(Uuid, Uuid)>,
    Json(request): Json<DelegationPolicyRequest>,
) -> Result<Json<tpl_agent_core::delegation::DelegationPolicy>, ApiError> {
    let policy = state
        .service
        .upsert_delegation_policy(
            &call.principal,
            &call.admin,
            organization_id,
            agent_id,
            DelegationPolicyUpdate {
                child_agent_ids: request.child_agent_ids,
                max_depth: request.max_depth,
                max_child_runs: request.max_child_runs,
            },
        )
        .await?;
    Ok(Json(policy))
}

async fn list_agent_skills(
    State(state): State<AgentApiState>,
    call: AdminCall,
    Path((organization_id, agent_id)): Path<(Uuid, Uuid)>,
) -> Result<Json<ListResponse<Uuid>>, ApiError> {
    let skill_ids = state
        .service
        .list_agent_skill_ids(&call.admin, organization_id, agent_id)
        .await?;
    Ok(Json(ListResponse::new(skill_ids)))
}

async fn replace_agent_skills(
    State(state): State<AgentApiState>,
    call: AdminCall,
    Path((organization_id, agent_id)): Path<(Uuid, Uuid)>,
    Json(request): Json<ReplaceAgentSkillsRequest>,
) -> Result<Json<ListResponse<Skill>>, ApiError> {
    let skills = state
        .service
        .replace_agent_skills(
            &call.principal,
            &call.admin,
            organization_id,
            agent_id,
            request.skill_ids,
        )
        .await?;
    Ok(Json(ListResponse::new(skills)))
}

async fn list_skills(
    State(state): State<AgentApiState>,
    call: AdminCall,
    Path(organization_id): Path<Uuid>,
) -> Result<Json<ListResponse<Skill>>, ApiError> {
    let skills = state
        .service
        .list_skills(&call.admin, organization_id)
        .await?;
    Ok(Json(ListResponse::new(skills)))
}

async fn create_skill(
    State(state): State<AgentApiState>,
    call: AdminCall,
    Path(organization_id): Path<Uuid>,
    Json(request): Json<CreateSkillRequest>,
) -> Result<(StatusCode, Json<Skill>), ApiError> {
    let skill = state
        .service
        .create_skill(
            &call.principal,
            &call.admin,
            organization_id,
            NewSkill {
                name: request.name,
                description: request.description,
                instructions: request.instructions,
            },
        )
        .await?;
    Ok((StatusCode::CREATED, Json(skill)))
}

async fn update_skill(
    State(state): State<AgentApiState>,
    call: AdminCall,
    Path((organization_id, skill_id)): Path<(Uuid, Uuid)>,
    Json(request): Json<UpdateSkillRequest>,
) -> Result<Json<Skill>, ApiError> {
    let skill = state
        .service
        .update_skill(
            &call.principal,
            &call.admin,
            organization_id,
            skill_id,
            SkillUpdate {
                description: request.description,
                instructions: request.instructions,
                enabled: request.enabled,
            },
        )
        .await?;
    Ok(Json(skill))
}

async fn list_skill_assets(
    State(state): State<AgentApiState>,
    call: AdminCall,
    Path((organization_id, skill_id)): Path<(Uuid, Uuid)>,
) -> Result<Json<ListResponse<SkillAsset>>, ApiError> {
    let assets = state
        .service
        .list_skill_assets(&call.admin, organization_id, skill_id)
        .await?;
    Ok(Json(ListResponse::new(assets)))
}

async fn upload_skill_asset(
    State(state): State<AgentApiState>,
    call: AdminCall,
    Path((organization_id, skill_id)): Path<(Uuid, Uuid)>,
    multipart: axum::extract::Multipart,
) -> Result<(StatusCode, Json<SkillAsset>), ApiError> {
    let file = single_file(multipart, state.max_upload_bytes).await?;
    let asset = state
        .service
        .upload_skill_asset(
            &call.principal,
            &call.admin,
            organization_id,
            skill_id,
            file,
        )
        .await?;
    Ok((StatusCode::CREATED, Json(asset)))
}

async fn download_skill_asset(
    State(state): State<AgentApiState>,
    call: AdminCall,
    Path((organization_id, skill_id, asset_id)): Path<(Uuid, Uuid, Uuid)>,
) -> Result<axum::response::Response, ApiError> {
    let blob = state
        .service
        .download_skill_asset(&call.admin, organization_id, skill_id, asset_id)
        .await?;
    Ok(download_response(blob))
}

async fn delete_skill_asset(
    State(state): State<AgentApiState>,
    call: AdminCall,
    Path((organization_id, skill_id, asset_id)): Path<(Uuid, Uuid, Uuid)>,
) -> Result<StatusCode, ApiError> {
    state
        .service
        .delete_skill_asset(
            &call.principal,
            &call.admin,
            organization_id,
            skill_id,
            asset_id,
        )
        .await?;
    Ok(StatusCode::NO_CONTENT)
}

async fn list_audit(
    State(state): State<AgentApiState>,
    call: AdminCall,
    Path(organization_id): Path<Uuid>,
) -> Result<Json<ListResponse<tpl_agent_core::audit::AuditRecord>>, ApiError> {
    let records = state
        .service
        .list_audit(&call.admin, organization_id)
        .await?;
    Ok(Json(ListResponse::new(records)))
}

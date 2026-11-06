//! public 用户路由：组织目录、Agent 目录、Session/消息、Run/SSE、资源与产物。

use std::convert::Infallible;
use std::sync::Arc;
use std::time::Duration;

use axum::Json;
use axum::Router;
use axum::extract::{DefaultBodyLimit, Path, Query, State};
use axum::http::{HeaderMap, HeaderName, StatusCode};
use axum::response::IntoResponse;
use axum::response::sse::{Event, KeepAlive, Sse};
use axum::routing::{get, post};
use tokio_stream::wrappers::ReceiverStream;
use uuid::Uuid;

use tpl_agent_core::error::AgentError;
use tpl_agent_core::service::public::SendMessageRequest as CoreSendMessageRequest;

use crate::dto::{
    ArtifactResponse, AvailableAgentResponse, CancelRunResponse, CreateSessionRequest,
    ListResponse, RunCreatedResponse, RunEventsQuery, RunEventsResponse, SendMessageRequest,
    SessionDetailResponse, StreamQuery, UpdateSessionTitleRequest, UserResourceResponse,
};
use crate::error::ApiError;
use crate::extract::UserCall;
use crate::routes::files::{download_response, single_file};
use crate::server::AgentApiState;

/// SSE 轮询间隔。
const STREAM_POLL_INTERVAL: Duration = Duration::from_millis(250);
/// 单次事件读取上限；与 core 的 clamp 保持一致。
const STREAM_BATCH_SIZE: usize = 200;

const IDEMPOTENCY_KEY_HEADER: HeaderName = HeaderName::from_static("idempotency-key");

pub(crate) fn routes(max_upload_limit: usize) -> Router<AgentApiState> {
    Router::new()
        .route("/api/v1/organizations", get(list_user_organizations))
        .route(
            "/api/v1/organizations/{organization_id}/agents",
            get(list_available_agents),
        )
        .route("/api/v1/agents/{agent_id}", get(get_available_agent))
        .route("/api/v1/agents/{agent_id}/skills", get(list_agent_skills))
        .route(
            "/api/v1/organizations/{organization_id}/sessions",
            get(list_sessions).post(create_session),
        )
        .route(
            "/api/v1/sessions/{session_id}",
            get(get_session).patch(update_session_title),
        )
        .route("/api/v1/sessions/{session_id}/messages", post(send_message))
        .route("/api/v1/runs/{run_id}", get(get_run))
        .route("/api/v1/runs/{run_id}/events", get(list_run_events))
        .route("/api/v1/runs/{run_id}/stream", get(stream_run_events))
        .route("/api/v1/runs/{run_id}/cancel", post(cancel_run))
        .route("/api/v1/runs/{run_id}/artifacts", get(list_artifacts))
        .route("/api/v1/artifacts/{artifact_id}", get(download_artifact))
        .route(
            "/api/v1/organizations/{organization_id}/resources",
            get(list_resources)
                .post(upload_resource)
                .layer(DefaultBodyLimit::max(max_upload_limit)),
        )
        .route(
            "/api/v1/resources/{resource_id}",
            get(download_resource).delete(delete_resource),
        )
}

async fn list_user_organizations(
    State(state): State<AgentApiState>,
    call: UserCall,
) -> Result<Json<ListResponse<tpl_agent_core::organization::Organization>>, ApiError> {
    let organizations = state.service.list_user_organizations(&call.user).await?;
    Ok(Json(ListResponse::new(organizations)))
}

async fn list_available_agents(
    State(state): State<AgentApiState>,
    call: UserCall,
    Path(organization_id): Path<Uuid>,
) -> Result<Json<ListResponse<AvailableAgentResponse>>, ApiError> {
    let agents = state
        .service
        .list_available_agents(&call.user, organization_id)
        .await?;
    Ok(Json(ListResponse::new(
        agents
            .into_iter()
            .map(AvailableAgentResponse::from)
            .collect(),
    )))
}

async fn get_available_agent(
    State(state): State<AgentApiState>,
    call: UserCall,
    Path(agent_id): Path<Uuid>,
) -> Result<Json<AvailableAgentResponse>, ApiError> {
    let agent = state.service.preview_agent(&call.user, agent_id).await?;
    Ok(Json(AvailableAgentResponse::from(agent)))
}

async fn list_agent_skills(
    State(state): State<AgentApiState>,
    call: UserCall,
    Path(agent_id): Path<Uuid>,
) -> Result<Json<ListResponse<tpl_agent_core::skill_access::Skill>>, ApiError> {
    let skills = state
        .service
        .list_skills_for_agent(&call.user, agent_id)
        .await?;
    Ok(Json(ListResponse::new(skills)))
}

async fn create_session(
    State(state): State<AgentApiState>,
    call: UserCall,
    Path(organization_id): Path<Uuid>,
    Json(request): Json<CreateSessionRequest>,
) -> Result<(StatusCode, Json<tpl_agent_core::session::ChatSession>), ApiError> {
    let session = state
        .service
        .create_session(
            &call.principal,
            &call.user,
            organization_id,
            request.agent_id,
            request.title,
        )
        .await?;
    Ok((StatusCode::CREATED, Json(session)))
}

async fn list_sessions(
    State(state): State<AgentApiState>,
    call: UserCall,
    Path(organization_id): Path<Uuid>,
) -> Result<Json<ListResponse<tpl_agent_core::session::ChatSession>>, ApiError> {
    let sessions = state
        .service
        .list_sessions(&call.user, organization_id)
        .await?;
    Ok(Json(ListResponse::new(sessions)))
}

async fn get_session(
    State(state): State<AgentApiState>,
    call: UserCall,
    Path(session_id): Path<Uuid>,
) -> Result<Json<SessionDetailResponse>, ApiError> {
    let detail = state
        .service
        .get_session_detail(&call.user, session_id)
        .await?;
    Ok(Json(SessionDetailResponse::from(detail)))
}

async fn update_session_title(
    State(state): State<AgentApiState>,
    call: UserCall,
    Path(session_id): Path<Uuid>,
    Json(request): Json<UpdateSessionTitleRequest>,
) -> Result<Json<tpl_agent_core::session::ChatSession>, ApiError> {
    let session = state
        .service
        .update_session_title(&call.user, session_id, &request.title)
        .await?;
    Ok(Json(session))
}

async fn send_message(
    State(state): State<AgentApiState>,
    call: UserCall,
    Path(session_id): Path<Uuid>,
    headers: HeaderMap,
    Json(request): Json<SendMessageRequest>,
) -> Result<(StatusCode, Json<RunCreatedResponse>), ApiError> {
    let idempotency_key = headers
        .get(IDEMPOTENCY_KEY_HEADER)
        .and_then(|header_value| header_value.to_str().ok())
        .ok_or_else(|| AgentError::InvalidRequest("缺少 Idempotency-Key 请求头".to_owned()))?;
    let outcome = state
        .service
        .send_message(
            &call.principal,
            &call.user,
            session_id,
            idempotency_key,
            CoreSendMessageRequest {
                agent_id: request.agent_id,
                content: request.content,
                resource_ids: request.resource_ids,
                skill_ids: request.skill_ids,
            },
        )
        .await?;
    let status_code = if outcome.created {
        StatusCode::CREATED
    } else {
        StatusCode::OK
    };
    Ok((status_code, Json(RunCreatedResponse::from(outcome))))
}

async fn get_run(
    State(state): State<AgentApiState>,
    call: UserCall,
    Path(run_id): Path<Uuid>,
) -> Result<Json<tpl_agent_core::run::RunRecord>, ApiError> {
    let run = state.service.get_run(&call.user, run_id).await?;
    Ok(Json(run))
}

async fn list_run_events(
    State(state): State<AgentApiState>,
    call: UserCall,
    Path(run_id): Path<Uuid>,
    Query(query): Query<RunEventsQuery>,
) -> Result<Json<RunEventsResponse>, ApiError> {
    let run = state.service.get_run(&call.user, run_id).await?;
    let events = state
        .service
        .list_run_events(
            &call.user,
            run_id,
            query.after_seq.unwrap_or(0),
            query.limit.unwrap_or(100),
        )
        .await?;
    Ok(Json(RunEventsResponse {
        run_id: run.run_id,
        status: run.status,
        last_event_seq: run.last_event_seq,
        items: events,
    }))
}

async fn stream_run_events(
    State(state): State<AgentApiState>,
    call: UserCall,
    Path(run_id): Path<Uuid>,
    Query(query): Query<StreamQuery>,
) -> Result<impl IntoResponse, ApiError> {
    let after_seq = query.after_seq.unwrap_or(0);
    if after_seq < 0 {
        return Err(AgentError::InvalidRequest("after_seq 不能为负数".to_owned()).into());
    }
    let _run = state.service.get_run(&call.user, run_id).await?;

    let service = Arc::clone(&state.service);
    let user = call.user;
    let (event_sender, event_receiver) =
        tokio::sync::mpsc::channel::<Result<Event, Infallible>>(64);
    tokio::spawn(async move {
        let mut cursor = after_seq;
        loop {
            match service
                .list_run_events(&user, run_id, cursor, STREAM_BATCH_SIZE)
                .await
            {
                Ok(events) => {
                    for event in events {
                        cursor = event.seq;
                        // 与 list_run_events 使用同一事件 JSON，客户端可直接按 RunEvent 解析。
                        let Ok(event_json) = serde_json::to_string(&event) else {
                            tracing::warn!(run_id = %run_id, seq = event.seq, "序列化 Run 事件失败");
                            return;
                        };
                        let sse_event = Event::default()
                            .id(event.seq.to_string())
                            .event(event.event_type)
                            .data(event_json);
                        if event_sender.send(Ok(sse_event)).await.is_err() {
                            return;
                        }
                    }
                }
                Err(stream_error) => {
                    tracing::warn!(error = %stream_error, "读取 Run 事件流失败");
                    return;
                }
            }
            match service.get_run(&user, run_id).await {
                Ok(current_run) => {
                    if current_run.status.is_terminal() && current_run.last_event_seq <= cursor {
                        return;
                    }
                }
                Err(stream_error) => {
                    tracing::warn!(error = %stream_error, "读取 Run 状态失败");
                    return;
                }
            }
            tokio::time::sleep(STREAM_POLL_INTERVAL).await;
        }
    });
    Ok(Sse::new(ReceiverStream::new(event_receiver)).keep_alive(KeepAlive::default()))
}

async fn cancel_run(
    State(state): State<AgentApiState>,
    call: UserCall,
    Path(run_id): Path<Uuid>,
) -> Result<Json<CancelRunResponse>, ApiError> {
    let outcome = state
        .service
        .cancel_run(&call.principal, &call.user, run_id)
        .await?;
    Ok(Json(CancelRunResponse::from(outcome)))
}

async fn list_artifacts(
    State(state): State<AgentApiState>,
    call: UserCall,
    Path(run_id): Path<Uuid>,
) -> Result<Json<ListResponse<ArtifactResponse>>, ApiError> {
    let artifacts = state.service.list_artifacts(&call.user, run_id).await?;
    Ok(Json(ListResponse::new(
        artifacts.into_iter().map(ArtifactResponse::from).collect(),
    )))
}

async fn download_artifact(
    State(state): State<AgentApiState>,
    call: UserCall,
    Path(artifact_id): Path<Uuid>,
) -> Result<axum::response::Response, ApiError> {
    let blob = state
        .service
        .download_artifact(&call.user, artifact_id)
        .await?;
    Ok(download_response(blob))
}

async fn upload_resource(
    State(state): State<AgentApiState>,
    call: UserCall,
    Path(organization_id): Path<Uuid>,
    multipart: axum::extract::Multipart,
) -> Result<(StatusCode, Json<UserResourceResponse>), ApiError> {
    let file = single_file(multipart, state.max_upload_bytes).await?;
    let resource = state
        .service
        .upload_resource(&call.principal, &call.user, organization_id, file)
        .await?;
    Ok((
        StatusCode::CREATED,
        Json(UserResourceResponse::from(resource)),
    ))
}

async fn list_resources(
    State(state): State<AgentApiState>,
    call: UserCall,
    Path(organization_id): Path<Uuid>,
) -> Result<Json<ListResponse<UserResourceResponse>>, ApiError> {
    let resources = state
        .service
        .list_resources(&call.user, organization_id)
        .await?;
    Ok(Json(ListResponse::new(
        resources
            .into_iter()
            .map(UserResourceResponse::from)
            .collect(),
    )))
}

async fn download_resource(
    State(state): State<AgentApiState>,
    call: UserCall,
    Path(resource_id): Path<Uuid>,
) -> Result<axum::response::Response, ApiError> {
    let blob = state
        .service
        .download_resource(&call.user, resource_id)
        .await?;
    Ok(download_response(blob))
}

async fn delete_resource(
    State(state): State<AgentApiState>,
    call: UserCall,
    Path(resource_id): Path<Uuid>,
) -> Result<StatusCode, ApiError> {
    state
        .service
        .delete_resource(&call.principal, &call.user, resource_id)
        .await?;
    Ok(StatusCode::NO_CONTENT)
}

//! 连接管理路由：CRUD、停用、凭据、检查与授权。

use axum::Json;
use axum::Router;
use axum::extract::rejection::JsonRejection;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::routing::{get, post, put};
use serde_json::Value;
use tpl_mcp_core::{
    AssignmentSubject, ConnectionChanges, GroupId, NewConnection, SecretString, SubjectId,
    SubjectType,
};
use url::Url;
use uuid::Uuid;

use crate::dto::{
    AssignmentResponse, ConnectionResponse, CreateConnectionRequest, ListResponse,
    SetCredentialRequest, UpdateConnectionRequest,
};
use crate::error::ApiError;
use crate::extract::CallerIdentity;
use crate::server::McpApiState;

pub(crate) fn routes() -> Router<McpApiState> {
    Router::new()
        .route(
            "/api/v1/connections",
            get(list_connections).post(create_connection),
        )
        .route(
            "/api/v1/connections/{connection_id}",
            get(get_connection)
                .patch(update_connection)
                .delete(delete_connection),
        )
        .route(
            "/api/v1/connections/{connection_id}/disable",
            post(disable_connection),
        )
        .route(
            "/api/v1/connections/{connection_id}/credential",
            put(set_credential),
        )
        .route(
            "/api/v1/connections/{connection_id}/tools",
            get(check_connection),
        )
        .route(
            "/api/v1/connections/{connection_id}/assignments",
            get(list_assignments),
        )
        .route(
            "/api/v1/connections/{connection_id}/assignments/{subject_type}/{subject_id}",
            put(add_assignment).delete(remove_assignment),
        )
}

async fn create_connection(
    State(state): State<McpApiState>,
    CallerIdentity(identity): CallerIdentity,
    payload: Result<Json<CreateConnectionRequest>, JsonRejection>,
) -> Result<(StatusCode, Json<ConnectionResponse>), ApiError> {
    let Json(request) = payload?;
    let new_connection = NewConnection {
        name: request.name,
        endpoint_url: parse_endpoint(&request.endpoint_url)?,
        auth_type: request.auth_type.into(),
        credential: request.credential.map(SecretString::new),
    };
    let connection = state
        .service
        .create_connection(&identity, new_connection)
        .await?;
    Ok((
        StatusCode::CREATED,
        Json(ConnectionResponse::from(&connection)),
    ))
}

async fn list_connections(
    State(state): State<McpApiState>,
    CallerIdentity(identity): CallerIdentity,
) -> Result<Json<ListResponse<ConnectionResponse>>, ApiError> {
    let connections = state.service.list_connections(&identity).await?;
    Ok(Json(ListResponse {
        items: connections.iter().map(ConnectionResponse::from).collect(),
    }))
}

async fn get_connection(
    State(state): State<McpApiState>,
    CallerIdentity(identity): CallerIdentity,
    Path(connection_id): Path<Uuid>,
) -> Result<Json<ConnectionResponse>, ApiError> {
    let connection = state
        .service
        .get_connection(&identity, connection_id)
        .await?;
    Ok(Json(ConnectionResponse::from(&connection)))
}

async fn update_connection(
    State(state): State<McpApiState>,
    CallerIdentity(identity): CallerIdentity,
    Path(connection_id): Path<Uuid>,
    payload: Result<Json<UpdateConnectionRequest>, JsonRejection>,
) -> Result<Json<ConnectionResponse>, ApiError> {
    let Json(request) = payload?;
    let changes = ConnectionChanges {
        name: request.name,
        endpoint_url: match request.endpoint_url {
            Some(value) => Some(parse_endpoint(&value)?),
            None => None,
        },
        auth_type: request.auth_type.map(Into::into),
        status: request.status.map(Into::into),
    };
    let connection = state
        .service
        .update_connection(&identity, connection_id, changes)
        .await?;
    Ok(Json(ConnectionResponse::from(&connection)))
}

async fn disable_connection(
    State(state): State<McpApiState>,
    CallerIdentity(identity): CallerIdentity,
    Path(connection_id): Path<Uuid>,
) -> Result<Json<ConnectionResponse>, ApiError> {
    let connection = state
        .service
        .disable_connection(&identity, connection_id)
        .await?;
    Ok(Json(ConnectionResponse::from(&connection)))
}

async fn delete_connection(
    State(state): State<McpApiState>,
    CallerIdentity(identity): CallerIdentity,
    Path(connection_id): Path<Uuid>,
) -> Result<StatusCode, ApiError> {
    state
        .service
        .delete_connection(&identity, connection_id)
        .await?;
    Ok(StatusCode::NO_CONTENT)
}

async fn set_credential(
    State(state): State<McpApiState>,
    CallerIdentity(identity): CallerIdentity,
    Path(connection_id): Path<Uuid>,
    payload: Result<Json<SetCredentialRequest>, JsonRejection>,
) -> Result<Json<ConnectionResponse>, ApiError> {
    let Json(request) = payload?;
    let connection = state
        .service
        .set_credential(
            &identity,
            connection_id,
            request.auth_type.into(),
            request.credential.map(SecretString::new),
        )
        .await?;
    Ok(Json(ConnectionResponse::from(&connection)))
}

async fn check_connection(
    State(state): State<McpApiState>,
    CallerIdentity(identity): CallerIdentity,
    Path(connection_id): Path<Uuid>,
) -> Result<Json<ListResponse<Value>>, ApiError> {
    let tools = state
        .service
        .check_connection(&identity, connection_id)
        .await?;
    Ok(Json(ListResponse {
        items: tools.into_iter().map(|tool| tool.definition).collect(),
    }))
}

async fn list_assignments(
    State(state): State<McpApiState>,
    CallerIdentity(identity): CallerIdentity,
    Path(connection_id): Path<Uuid>,
) -> Result<Json<ListResponse<AssignmentResponse>>, ApiError> {
    let assignments = state
        .service
        .list_assignments(&identity, connection_id)
        .await?;
    Ok(Json(ListResponse {
        items: assignments.iter().map(AssignmentResponse::from).collect(),
    }))
}

async fn add_assignment(
    State(state): State<McpApiState>,
    CallerIdentity(identity): CallerIdentity,
    Path((connection_id, subject_type, subject_id)): Path<(Uuid, String, String)>,
) -> Result<StatusCode, ApiError> {
    let subject = parse_subject(&subject_type, &subject_id)?;
    state
        .service
        .add_assignment(&identity, connection_id, subject)
        .await?;
    Ok(StatusCode::NO_CONTENT)
}

async fn remove_assignment(
    State(state): State<McpApiState>,
    CallerIdentity(identity): CallerIdentity,
    Path((connection_id, subject_type, subject_id)): Path<(Uuid, String, String)>,
) -> Result<StatusCode, ApiError> {
    let subject = parse_subject(&subject_type, &subject_id)?;
    state
        .service
        .remove_assignment(&identity, connection_id, &subject)
        .await?;
    Ok(StatusCode::NO_CONTENT)
}

fn parse_endpoint(value: &str) -> Result<Url, ApiError> {
    Url::parse(value).map_err(|_| ApiError::invalid_parameter("endpoint_url 不是合法 URL"))
}

fn parse_subject(subject_type: &str, subject_id: &str) -> Result<AssignmentSubject, ApiError> {
    let subject_type = SubjectType::parse(subject_type)
        .ok_or_else(|| ApiError::invalid_parameter("subject_type 只能是 user 或 group"))?;
    match subject_type {
        SubjectType::User => SubjectId::new(subject_id)
            .map(AssignmentSubject::User)
            .map_err(|_| ApiError::invalid_parameter("subject_id 不合法")),
        SubjectType::Group => GroupId::new(subject_id)
            .map(AssignmentSubject::Group)
            .map_err(|_| ApiError::invalid_parameter("subject_id 不合法")),
    }
}

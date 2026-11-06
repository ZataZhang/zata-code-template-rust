//! 只面向 Agent HTTP 契约的上游客户端与错误映射。

use axum::Json;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use reqwest::Method;
use serde::Serialize;
use serde::de::DeserializeOwned;

use crate::contract::{ErrorEnvelope, SendMessageRequest};

/// 业务入口的稳定错误响应；成功时不会出现。
#[derive(Debug)]
pub struct ApiError {
    /// HTTP 状态。
    pub status: StatusCode,
    /// 稳定错误码。
    pub code: String,
    /// 展示说明。
    pub message: String,
}

impl ApiError {
    /// 业务服务无法连接 Agent 服务。
    pub fn agent_unavailable() -> Self {
        Self {
            status: StatusCode::BAD_GATEWAY,
            code: "agent_unavailable".to_owned(),
            message: "Agent 服务暂不可用".to_owned(),
        }
    }

    /// Agent 响应不符合声明的契约。
    pub fn contract_violation() -> Self {
        Self {
            status: StatusCode::BAD_GATEWAY,
            code: "agent_contract_violation".to_owned(),
            message: "Agent 响应不符合 HTTP 契约".to_owned(),
        }
    }

    /// 请求参数不合法（由本服务或上游返回）。
    pub fn invalid_request(message: impl Into<String>) -> Self {
        Self {
            status: StatusCode::BAD_REQUEST,
            code: "invalid_request".to_owned(),
            message: message.into(),
        }
    }

    /// 原样透传 Agent 的稳定错误码与状态；响应体不是错误封套时按契约破坏处理。
    pub fn from_upstream(status: StatusCode, body: &str) -> Self {
        match serde_json::from_str::<ErrorEnvelope>(body) {
            Ok(envelope) => Self {
                status,
                code: envelope.error.code,
                message: envelope.error.message,
            },
            Err(_) => Self::contract_violation(),
        }
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        #[derive(Serialize)]
        struct Body {
            code: String,
            message: String,
        }
        #[derive(Serialize)]
        struct Envelope {
            error: Body,
        }
        (
            self.status,
            Json(Envelope {
                error: Body {
                    code: self.code,
                    message: self.message,
                },
            }),
        )
            .into_response()
    }
}

/// Agent HTTP 上游响应。
pub struct UpstreamResponse {
    /// 上游状态码。
    pub status: StatusCode,
    /// 上游响应正文。
    pub body: String,
}

impl UpstreamResponse {
    /// 解析成功响应；状态不是 2xx 或正文不符合契约时返回错误。
    pub fn parse_success<T: DeserializeOwned>(self) -> Result<T, ApiError> {
        if !self.status.is_success() {
            return Err(ApiError::from_upstream(self.status, &self.body));
        }
        serde_json::from_str(&self.body).map_err(|_| ApiError::contract_violation())
    }
}

/// 只调用 Agent public HTTP API 的客户端。
pub struct AgentClient {
    http: reqwest::Client,
    base_url: String,
    service_token: String,
}

impl AgentClient {
    /// 构造客户端；服务令牌只保存在服务端进程内。
    pub fn new(base_url: String, service_token: String) -> anyhow::Result<Self> {
        let http = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(60))
            .build()?;
        Ok(Self {
            http,
            base_url: base_url.trim_end_matches('/').to_owned(),
            service_token,
        })
    }

    /// 代理一次 Agent 请求，只注入服务凭据，并转发用户 Cookie 与幂等键。
    pub async fn request(
        &self,
        method: Method,
        path: &str,
        cookie: Option<&str>,
        idempotency_key: Option<&str>,
        body: Option<&SendMessageRequest>,
    ) -> Result<UpstreamResponse, ApiError> {
        let mut request = self
            .http
            .request(method, format!("{}{path}", self.base_url))
            .bearer_auth(&self.service_token)
            .header("accept", "application/json");
        if let Some(cookie) = cookie {
            request = request.header("cookie", cookie);
        }
        if let Some(idempotency_key) = idempotency_key {
            request = request.header("idempotency-key", idempotency_key);
        }
        if let Some(body) = body {
            request = request.json(body);
        }
        let response = request.send().await.map_err(|error| {
            tracing::warn!(error = %error, "Agent 上游请求失败");
            ApiError::agent_unavailable()
        })?;
        let status = response.status();
        let body = response.text().await.map_err(|error| {
            tracing::warn!(error = %error, "Agent 上游响应读取失败");
            ApiError::agent_unavailable()
        })?;
        Ok(UpstreamResponse { status, body })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn passes_through_upstream_error_code_and_status() {
        let error = ApiError::from_upstream(
            StatusCode::NOT_FOUND,
            r#"{"error":{"code":"run_not_found","message":"目标不存在"}}"#,
        );
        assert_eq!(error.status, StatusCode::NOT_FOUND);
        assert_eq!(error.code, "run_not_found");
    }

    #[test]
    fn rejects_non_envelope_error_body() {
        let error = ApiError::from_upstream(StatusCode::BAD_GATEWAY, "upstream crashed");
        assert_eq!(error.status, StatusCode::BAD_GATEWAY);
        assert_eq!(error.code, "agent_contract_violation");
    }

    #[test]
    fn parses_success_only_for_2xx() {
        let error = UpstreamResponse {
            status: StatusCode::CONFLICT,
            body: r#"{"run":{},"created":true}"#.to_owned(),
        }
        .parse_success::<serde_json::Value>()
        .expect_err("非 2xx 不能按成功解析");
        assert_eq!(error.code, "agent_contract_violation");
    }
}

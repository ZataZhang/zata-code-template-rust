//! 真实入口端到端测试：真实 PostgreSQL、真实 `mcp-service` 进程、真实 rmcp 上游替身。
//!
//! 默认跳过；设置 `TEST_MCP_DATABASE_URL` 后运行：
//!
//! ```text
//! cargo test -p tpl-mcp --test http_end_to_end -- --ignored --nocapture
//! ```
//!
//! `just test real` 与 `just mcp-e2e --scenario ...` 会设置该变量并执行本文件。
//! 只有远端 MCP provider 是本地假实现；服务 HTTP 入口、调用方验签、授权策略、
//! PostgreSQL 迁移与仓储、凭据加密、出站地址校验和 rmcp 传输都走真实实现。

use std::collections::HashMap;
use std::fs::File;
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, Instant};

use axum::Router;
use axum::extract::{Request, State};
use axum::http::StatusCode;
use axum::http::header::AUTHORIZATION;
use axum::middleware::{self, Next};
use axum::response::Response;
use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use hmac::{Hmac, KeyInit, Mac};
use reqwest::Client;
use rmcp::model::{
    CallToolRequestParams, CallToolResponse, CallToolResult, ContentBlock, Implementation,
    ListToolsResult, PaginatedRequestParams, ServerCapabilities, ServerConfig, Tool,
};
use rmcp::service::RequestContext;
use rmcp::transport::streamable_http_client::{
    StreamableHttpClientTransport, StreamableHttpClientTransportConfig,
};
use rmcp::transport::streamable_http_server::session::local::LocalSessionManager;
use rmcp::transport::{StreamableHttpServerConfig, StreamableHttpService};
use rmcp::{ErrorData, RoleClient, RoleServer, ServerHandler, ServiceError, ServiceExt};
use serde_json::{Value, json};
use sha2::Sha256;
use sqlx::Row;
use tpl_mcp_core::exposed_tool_name;
use uuid::Uuid;

const CONSUMER_ID: &str = "consumer-e2e";
const SERVICE_TOKEN: &str = "e2e-service-token-0123456789abcdef";
const SIGNING_SECRET: &str = "e2e-signing-secret-0123456789abcdef";
const OTHER_CONSUMER_ID: &str = "consumer-e2e-other";
const OTHER_SERVICE_TOKEN: &str = "e2e-other-service-token-0123456789abcdef";
const OTHER_SIGNING_SECRET: &str = "e2e-other-signing-secret-0123456789abcdef";
/// Base64 编码的 32 字节测试主密钥（`0123456789abcdef` 重复两次）。
const CREDENTIAL_KEY_BASE64: &str = "MDEyMzQ1Njc4OWFiY2RlZjAxMjM0NTY3ODlhYmNkZWY=";
const UPSTREAM_TOOL: &str = "lookup_weather";

fn database_url() -> String {
    std::env::var("TEST_MCP_DATABASE_URL")
        .expect("必须设置 TEST_MCP_DATABASE_URL（MCP 专属测试库，见 docker-compose.testing.yml）")
}

fn consumers_json() -> String {
    json!([
        {
            "consumer_id": CONSUMER_ID,
            "token": SERVICE_TOKEN,
            "context_signing_secret": SIGNING_SECRET,
        },
        {
            "consumer_id": OTHER_CONSUMER_ID,
            "token": OTHER_SERVICE_TOKEN,
            "context_signing_secret": OTHER_SIGNING_SECRET,
        }
    ])
    .to_string()
}

#[derive(Clone)]
struct Caller {
    context: String,
    service_token: &'static str,
}

struct CallerIdentity<'a> {
    consumer_id: &'a str,
    tenant_id: &'a str,
    subject_id: &'a str,
    groups: &'a [&'a str],
    role: &'a str,
}

impl Caller {
    fn new(tenant: &str, subject: &str, groups: &[&str], role: &str) -> Self {
        Self::signed(
            CallerIdentity {
                consumer_id: CONSUMER_ID,
                tenant_id: tenant,
                subject_id: subject,
                groups,
                role,
            },
            SERVICE_TOKEN,
            SIGNING_SECRET,
        )
    }

    fn signed(
        identity: CallerIdentity<'_>,
        service_token: &'static str,
        signing_secret: &str,
    ) -> Self {
        let now = time::OffsetDateTime::now_utc().unix_timestamp();
        let claims = json!({
            "iss": "tpl-consumer-backend",
            "aud": "tpl-mcp-gateway",
            "consumer_id": identity.consumer_id,
            "tenant_id": identity.tenant_id,
            "subject_id": identity.subject_id,
            "groups": identity.groups,
            "role": identity.role,
            "iat": now,
            "exp": now + 120,
        });
        let payload = serde_json::to_vec(&claims).expect("测试声明可序列化");
        let payload_segment = URL_SAFE_NO_PAD.encode(payload);
        let mut mac =
            Hmac::<Sha256>::new_from_slice(signing_secret.as_bytes()).expect("测试签名密钥有效");
        mac.update(payload_segment.as_bytes());
        let signature = URL_SAFE_NO_PAD.encode(mac.finalize().into_bytes());
        Self {
            context: format!("{payload_segment}.{signature}"),
            service_token,
        }
    }

    fn apply(&self, request: reqwest::RequestBuilder) -> reqwest::RequestBuilder {
        request
            .bearer_auth(self.service_token)
            .header("x-user-context", &self.context)
    }
}

fn admin(tenant: &str) -> Caller {
    Caller::new(tenant, "admin-1", &[], "admin")
}

fn user(tenant: &str, subject: &str, groups: &[&str]) -> Caller {
    Caller::new(tenant, subject, groups, "user")
}

fn other_consumer_admin(tenant: &str) -> Caller {
    Caller::signed(
        CallerIdentity {
            consumer_id: OTHER_CONSUMER_ID,
            tenant_id: tenant,
            subject_id: "admin-1",
            groups: &[],
            role: "admin",
        },
        OTHER_SERVICE_TOKEN,
        OTHER_SIGNING_SECRET,
    )
}

fn other_consumer_user(tenant: &str, subject: &str) -> Caller {
    Caller::signed(
        CallerIdentity {
            consumer_id: OTHER_CONSUMER_ID,
            tenant_id: tenant,
            subject_id: subject,
            groups: &["team-a"],
            role: "user",
        },
        OTHER_SERVICE_TOKEN,
        OTHER_SIGNING_SECRET,
    )
}

struct FakeUpstream;

impl ServerHandler for FakeUpstream {
    fn get_info(&self) -> ServerConfig {
        ServerConfig::new(ServerCapabilities::builder().enable_tools().build())
            .with_server_info(Implementation::new("fake-upstream", "0.1.0"))
    }

    async fn list_tools(
        &self,
        _request: Option<PaginatedRequestParams>,
        _context: RequestContext<RoleServer>,
    ) -> Result<ListToolsResult, ErrorData> {
        let input_schema = json!({
            "type": "object",
            "properties": { "city": { "type": "string" } },
            "required": ["city"],
        });
        let tool = Tool::new(
            UPSTREAM_TOOL,
            "查询城市天气",
            input_schema
                .as_object()
                .cloned()
                .expect("测试 schema 是对象"),
        );
        Ok(ListToolsResult::with_all_items(vec![tool]))
    }

    async fn call_tool(
        &self,
        request: CallToolRequestParams,
        _context: RequestContext<RoleServer>,
    ) -> Result<CallToolResponse, ErrorData> {
        if request.name.as_ref() != UPSTREAM_TOOL {
            return Err(ErrorData::resource_not_found("远端工具不存在", None));
        }
        let city = request
            .arguments
            .as_ref()
            .and_then(|arguments| arguments.get("city"))
            .and_then(Value::as_str)
            .unwrap_or("unknown");
        Ok(CallToolResponse::Complete(CallToolResult::success(vec![
            ContentBlock::text(format!("upstream-result:{UPSTREAM_TOOL}:{city}")),
        ])))
    }
}

struct UpstreamState {
    expected_token: Option<String>,
    requests: Arc<AtomicUsize>,
}

async fn require_upstream_bearer(
    State(state): State<Arc<UpstreamState>>,
    request: Request,
    next: Next,
) -> Result<Response, StatusCode> {
    state.requests.fetch_add(1, Ordering::SeqCst);
    if let Some(expected_token) = state.expected_token.as_ref() {
        let expected = format!("Bearer {expected_token}");
        let supplied = request
            .headers()
            .get(AUTHORIZATION)
            .and_then(|value| value.to_str().ok());
        if supplied != Some(expected.as_str()) {
            return Err(StatusCode::UNAUTHORIZED);
        }
    }
    Ok(next.run(request).await)
}

/// 启动本地假上游；`expected_token` 存在时校验 Bearer 凭据。
/// 返回 endpoint 与请求计数，便于证明负控在建立连接前就被拒绝。
async fn start_fake_upstream(expected_token: Option<String>) -> (String, Arc<AtomicUsize>) {
    let service = StreamableHttpService::new(
        || Ok::<FakeUpstream, std::io::Error>(FakeUpstream),
        Arc::new(LocalSessionManager::default()),
        StreamableHttpServerConfig::default(),
    );
    let requests = Arc::new(AtomicUsize::new(0));
    let router = Router::new()
        .nest_service("/mcp", service)
        .layer(middleware::from_fn_with_state(
            Arc::new(UpstreamState {
                expected_token,
                requests: requests.clone(),
            }),
            require_upstream_bearer,
        ));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("绑定假上游端口");
    let address = listener.local_addr().expect("读取假上游地址");
    tokio::spawn(async move {
        let _ = axum::serve(listener, router).await;
    });
    (format!("http://{address}/mcp"), requests)
}

fn free_port() -> u16 {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("分配空闲端口");
    listener.local_addr().expect("读取空闲端口").port()
}

struct RunningGateway {
    child: Child,
    base_url: String,
    stdout_path: PathBuf,
    stderr_path: PathBuf,
}

impl RunningGateway {
    fn start(database_url: &str) -> Self {
        Self::start_with_policy(database_url, true)
    }

    /// `allow_loopback` 控制开发专用策略；false 等价于生产安全策略。
    fn start_with_policy(database_url: &str, allow_loopback: bool) -> Self {
        let port = free_port();
        let stamp = Uuid::new_v4().simple();
        let stdout_path = std::env::temp_dir().join(format!("mcp-e2e-{stamp}.stdout.log"));
        let stderr_path = std::env::temp_dir().join(format!("mcp-e2e-{stamp}.stderr.log"));
        let stdout = File::create(&stdout_path).expect("创建服务 stdout 日志");
        let stderr = File::create(&stderr_path).expect("创建服务 stderr 日志");
        let child = Command::new(env!("CARGO_BIN_EXE_mcp-service"))
            .env("APP_MCP_CONFIG_FILE", format!("missing-{stamp}.toml"))
            .env("MCP_DATABASE_URL", database_url)
            .env("MCP_CREDENTIAL_KEY_BASE64", CREDENTIAL_KEY_BASE64)
            .env("MCP_CONSUMERS_JSON", consumers_json())
            .env("APP_MCP__SERVER__HOST", "127.0.0.1")
            .env("APP_MCP__SERVER__PORT", port.to_string())
            .env(
                "APP_MCP__SECURITY__ALLOW_INSECURE_LOOPBACK",
                allow_loopback.to_string(),
            )
            // 用 debug 级运行，证明即使调高日志级别也不会把工具参数、结果或凭据写入日志。
            .env("RUST_LOG", "debug")
            .stdout(Stdio::from(stdout))
            .stderr(Stdio::from(stderr))
            .spawn()
            .expect("启动 mcp-service 进程");
        Self {
            child,
            base_url: format!("http://127.0.0.1:{port}"),
            stdout_path,
            stderr_path,
        }
    }

    async fn wait_ready(&mut self, client: &Client) {
        let deadline = Instant::now() + Duration::from_secs(30);
        loop {
            if let Some(status) = self.child.try_wait().expect("读取服务进程状态") {
                panic!("mcp-service 进程提前退出：{status}");
            }
            if let Ok(response) = client.get(format!("{}/ready", self.base_url)).send().await
                && response.status().is_success()
            {
                return;
            }
            if Instant::now() >= deadline {
                panic!("mcp-service 在 30 秒内未就绪");
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    }

    fn logs(&self) -> String {
        let stdout = std::fs::read_to_string(&self.stdout_path).unwrap_or_default();
        let stderr = std::fs::read_to_string(&self.stderr_path).unwrap_or_default();
        format!("{stdout}\n{stderr}")
    }
}

impl Drop for RunningGateway {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

async fn gateway_client(
    base_url: &str,
    caller: &Caller,
) -> rmcp::service::RunningService<RoleClient, ()> {
    let mut headers = HashMap::new();
    headers.insert(
        axum::http::header::HeaderName::from_static("x-user-context"),
        axum::http::header::HeaderValue::from_str(&caller.context).expect("上下文头合法"),
    );
    let config = StreamableHttpClientTransportConfig::with_uri(format!("{base_url}/mcp"))
        .auth_header(caller.service_token)
        .custom_headers(headers)
        .max_concurrent_requests(1)
        .reinit_on_expired_session(false);
    let http = Client::builder()
        .no_proxy()
        .build()
        .expect("创建 MCP 客户端");
    let transport = StreamableHttpClientTransport::with_client(http, config);
    ().serve(transport)
        .await
        .expect("连接 MCP 网关并完成初始化")
}

async fn call_gateway_tool(
    client: &rmcp::service::RunningService<RoleClient, ()>,
    name: &str,
    arguments: Value,
) -> Result<CallToolResult, ServiceError> {
    let mut request = CallToolRequestParams::new(name.to_owned());
    if let Some(arguments) = arguments.as_object() {
        request = request.with_arguments(arguments.clone());
    }
    client.call_tool(request).await
}

fn assert_forbidden(result: Result<CallToolResult, ServiceError>) {
    match result {
        Err(ServiceError::McpError(error)) => {
            assert_eq!(error.code.0, -32003, "实际错误：{error:?}");
            assert_eq!(
                error
                    .data
                    .as_ref()
                    .and_then(|data| data.get("status"))
                    .and_then(Value::as_i64),
                Some(403)
            );
        }
        other => panic!("期望 JSON-RPC 权限错误，实际：{other:?}"),
    }
}

fn assert_upstream_unavailable(result: Result<CallToolResult, ServiceError>) {
    match result {
        Err(ServiceError::McpError(error)) => {
            assert_eq!(error.code.0, -32005, "实际错误：{error:?}");
            assert_eq!(
                error
                    .data
                    .as_ref()
                    .and_then(|data| data.get("status"))
                    .and_then(Value::as_i64),
                Some(502)
            );
        }
        other => panic!("期望 JSON-RPC 上游不可用错误，实际：{other:?}"),
    }
}

fn checkpoint(message: impl std::fmt::Display) {
    println!("[检查点] {message}");
}

#[tokio::test]
#[ignore = "需要真实 PostgreSQL 与可执行 mcp-service；设置 TEST_MCP_DATABASE_URL 后运行"]
async fn real_authorization_scenario() {
    let database_url = database_url();
    let (upstream_url, _) = start_fake_upstream(None).await;
    let mut gateway = RunningGateway::start(&database_url);
    let http = Client::builder()
        .no_proxy()
        .build()
        .expect("创建 HTTP 客户端");
    gateway.wait_ready(&http).await;
    let base = gateway.base_url.clone();

    let stamp = Uuid::new_v4().simple().to_string();
    let tenant = format!("tenant-auth-{stamp}");
    let other_tenant = format!("tenant-other-{stamp}");
    let admin_caller = admin(&tenant);
    let allowed_user = user(&tenant, "user-allowed", &["team-a"]);
    let denied_user = user(&tenant, "user-denied", &[]);
    let other_tenant_user = user(&other_tenant, "user-allowed", &["team-a"]);

    // 未认证请求必须被拒绝。
    let unauthenticated = http
        .get(format!("{base}/api/v1/connections"))
        .send()
        .await
        .expect("发送未认证请求");
    assert_eq!(unauthenticated.status(), StatusCode::UNAUTHORIZED);
    checkpoint("未认证管理请求返回 401");

    // 管理员创建匿名连接并只授权给 user-allowed。
    let created = admin_caller
        .apply(http.post(format!("{base}/api/v1/connections")))
        .json(&json!({
            "name": format!("auth-{stamp}"),
            "endpoint_url": upstream_url,
            "auth_type": "none",
        }))
        .send()
        .await
        .expect("创建连接请求");
    assert_eq!(created.status(), StatusCode::CREATED);
    let created: Value = created.json().await.expect("创建响应是 JSON");
    let connection_id = created["id"].as_str().expect("响应包含连接 ID").to_owned();
    let connection_id = Uuid::parse_str(&connection_id).expect("连接 ID 是 UUID");

    let assignment = admin_caller
        .apply(http.put(format!(
            "{base}/api/v1/connections/{connection_id}/assignments/user/user-allowed"
        )))
        .send()
        .await
        .expect("创建用户授权");
    assert_eq!(assignment.status(), StatusCode::NO_CONTENT);
    checkpoint(format!(
        "管理员创建连接 {connection_id} 并通过 REST 授权 user-allowed"
    ));
    let expected_tool = exposed_tool_name(connection_id, UPSTREAM_TOOL);
    let city = format!("city-{stamp}");

    // 获准用户：看到带连接前缀的工具并成功调用。
    let allowed_client = gateway_client(&base, &allowed_user).await;
    let tools = allowed_client
        .list_all_tools()
        .await
        .expect("获准用户列出工具");
    assert_eq!(tools.len(), 1, "实际工具：{tools:?}");
    assert_eq!(tools[0].name.as_ref(), expected_tool);
    let result = call_gateway_tool(&allowed_client, &expected_tool, json!({ "city": city }))
        .await
        .expect("获准用户调用工具");
    let text = result
        .content
        .iter()
        .find_map(|content| match content {
            ContentBlock::Text(text) => Some(text.text.clone()),
            _ => None,
        })
        .expect("工具结果包含文本");
    assert_eq!(text, format!("upstream-result:{UPSTREAM_TOOL}:{city}"));
    checkpoint(format!(
        "获准用户 tools/list 返回命名空间工具 {expected_tool}，tools/call 返回 upstream 结果"
    ));
    let _ = allowed_client.cancel().await;

    // 未授权用户：看不到工具，调用返回文档化的 403。
    let denied_client = gateway_client(&base, &denied_user).await;
    assert!(
        denied_client
            .list_all_tools()
            .await
            .expect("未授权用户列出工具")
            .is_empty()
    );
    assert_forbidden(call_gateway_tool(&denied_client, &expected_tool, json!({})).await);
    checkpoint("负控：同租户未授权用户 tools/list 为空，tools/call 返回 -32003/403");
    let _ = denied_client.cancel().await;

    // 另一租户的同名用户：同样看不到、也不能调用。
    let other_client = gateway_client(&base, &other_tenant_user).await;
    assert!(
        other_client
            .list_all_tools()
            .await
            .expect("另一租户列出工具")
            .is_empty()
    );
    assert_forbidden(call_gateway_tool(&other_client, &expected_tool, json!({})).await);
    checkpoint("负控：另一租户同名用户 tools/list 为空，tools/call 返回 -32003/403");
    let _ = other_client.cancel().await;

    // 第二消费服务在同一租户字符串下登记自己的连接并授权同名用户。
    let other_admin = other_consumer_admin(&tenant);
    let other_created = other_admin
        .apply(http.post(format!("{base}/api/v1/connections")))
        .json(&json!({
            "name": format!("other-consumer-{stamp}"),
            "endpoint_url": upstream_url,
            "auth_type": "none",
        }))
        .send()
        .await
        .expect("第二消费服务创建连接");
    assert_eq!(other_created.status(), StatusCode::CREATED);
    let other_created: Value = other_created.json().await.expect("创建响应是 JSON");
    let other_connection_id =
        Uuid::parse_str(other_created["id"].as_str().expect("响应包含连接 ID"))
            .expect("连接 ID 是 UUID");
    let other_assignment = other_admin
        .apply(http.put(format!(
            "{base}/api/v1/connections/{other_connection_id}/assignments/user/user-allowed"
        )))
        .send()
        .await
        .expect("第二消费服务创建用户授权");
    assert_eq!(other_assignment.status(), StatusCode::NO_CONTENT);
    let other_expected_tool = exposed_tool_name(other_connection_id, UPSTREAM_TOOL);

    // 租户字符串相同，但消费服务不同：第一消费服务的获准用户仍只看到自己的连接。
    let allowed_client = gateway_client(&base, &allowed_user).await;
    let tools = allowed_client
        .list_all_tools()
        .await
        .expect("第一消费服务重新列出工具");
    assert_eq!(tools.len(), 1, "实际工具：{tools:?}");
    assert_eq!(tools[0].name.as_ref(), expected_tool);

    // 第二消费服务的同名用户只看到自己的连接，并能调用自己的工具。
    let other_allowed_client =
        gateway_client(&base, &other_consumer_user(&tenant, "user-allowed")).await;
    let other_tools = other_allowed_client
        .list_all_tools()
        .await
        .expect("第二消费服务列出工具");
    assert_eq!(other_tools.len(), 1, "实际工具：{other_tools:?}");
    assert_eq!(other_tools[0].name.as_ref(), other_expected_tool);
    let other_result = call_gateway_tool(
        &other_allowed_client,
        &other_expected_tool,
        json!({ "city": city }),
    )
    .await
    .expect("第二消费服务调用自己的工具");
    let other_text = other_result
        .content
        .iter()
        .find_map(|content| match content {
            ContentBlock::Text(text) => Some(text.text.clone()),
            _ => None,
        })
        .expect("工具结果包含文本");
    assert_eq!(
        other_text,
        format!("upstream-result:{UPSTREAM_TOOL}:{city}")
    );

    // 跨消费服务调用对方工具必须被拒绝，即使租户字符串、用户主体与分组相同。
    assert_forbidden(call_gateway_tool(&other_allowed_client, &expected_tool, json!({})).await);
    assert_forbidden(call_gateway_tool(&allowed_client, &other_expected_tool, json!({})).await);
    checkpoint("负控：消费服务 A 的用户看不到、也调不动消费服务 B 的同租户连接，反之亦然");
    let _ = other_allowed_client.cancel().await;
    let _ = allowed_client.cancel().await;

    // 日志不得包含工具参数值。
    let logs = gateway.logs();
    assert!(!logs.contains(&city), "服务日志不得包含工具参数");
    assert!(
        !logs.contains("upstream-result:"),
        "服务日志不得包含工具结果"
    );
    checkpoint("服务在 RUST_LOG=debug 下运行，日志仍不含工具参数与工具结果");
}

#[tokio::test]
#[ignore = "需要真实 PostgreSQL 与可执行 mcp-service；设置 TEST_MCP_DATABASE_URL 后运行"]
async fn real_remote_connection_scenario() {
    const CREDENTIAL: &str = "super-secret-token-0123456789";
    let database_url = database_url();
    let (upstream_url, upstream_requests) = start_fake_upstream(Some(CREDENTIAL.to_owned())).await;
    let mut gateway = RunningGateway::start(&database_url);
    let mut safe_gateway = RunningGateway::start_with_policy(&database_url, false);
    let http = Client::builder()
        .no_proxy()
        .build()
        .expect("创建 HTTP 客户端");
    gateway.wait_ready(&http).await;
    safe_gateway.wait_ready(&http).await;
    let base = gateway.base_url.clone();
    let safe_base = safe_gateway.base_url.clone();

    let stamp = Uuid::new_v4().simple().to_string();
    let tenant = format!("tenant-remote-{stamp}");
    let admin_caller = admin(&tenant);
    let allowed_user = user(&tenant, "user-allowed", &[]);
    let city = format!("city-{stamp}");

    // 负控 1：生产安全策略下 loopback 地址在写入阶段即被拒绝，且不会触发上游连接。
    let safe_create = admin_caller
        .apply(http.post(format!("{safe_base}/api/v1/connections")))
        .json(&json!({
            "name": format!("loopback-{stamp}"),
            "endpoint_url": upstream_url,
            "auth_type": "none",
        }))
        .send()
        .await
        .expect("发送生产策略 loopback 请求");
    assert_eq!(safe_create.status(), StatusCode::BAD_REQUEST);
    let safe_body: Value = safe_create.json().await.expect("错误响应是 JSON");
    assert_eq!(safe_body["error"]["code"], json!("unsafe_endpoint"));
    assert_eq!(
        upstream_requests.load(Ordering::SeqCst),
        0,
        "被拒绝的 endpoint 不得触发远端连接"
    );
    checkpoint("负控：生产安全策略拒绝 loopback HTTP，上游请求计数保持 0");

    // 负控 2：HTTPS 元数据、私网、IPv4 映射与 ULA 目标通过协议检查，必须由
    // 地址策略本身拒绝；这些目标不会触发任何上游请求。
    for target in [
        "https://169.254.169.254/latest/meta-data",
        "https://100.100.100.200/latest/meta-data",
        "https://10.0.0.1/mcp",
        "https://192.168.1.1/mcp",
        "https://[::ffff:a9fe:a9fe]/mcp",
        "https://[fd00:ec2::254]/mcp",
    ] {
        let rejected = admin_caller
            .apply(http.post(format!("{safe_base}/api/v1/connections")))
            .json(&json!({
                "name": format!("unsafe-{stamp}"),
                "endpoint_url": target,
                "auth_type": "bearer",
                "credential": CREDENTIAL,
            }))
            .send()
            .await
            .expect("发送不安全 endpoint 请求");
        assert_eq!(
            rejected.status(),
            StatusCode::BAD_REQUEST,
            "目标 {target} 必须在写入阶段被拒绝"
        );
        let rejected_body: Value = rejected.json().await.expect("错误响应是 JSON");
        assert_eq!(
            rejected_body["error"]["code"],
            json!("unsafe_endpoint"),
            "目标 {target} 应返回 unsafe_endpoint"
        );
    }
    assert_eq!(
        upstream_requests.load(Ordering::SeqCst),
        0,
        "被拒绝的 endpoint 不得触发远端连接"
    );
    checkpoint(
        "负控：HTTPS 元数据、私网、IPv4 映射与 ULA 地址被地址策略拒绝为 unsafe_endpoint，上游请求计数保持 0",
    );
    drop(safe_gateway);

    // 负控 3：开发 loopback 模式放宽的只是 loopback，私网地址仍被拒绝。
    let unsafe_create = admin_caller
        .apply(http.post(format!("{base}/api/v1/connections")))
        .json(&json!({
            "name": format!("unsafe-{stamp}"),
            "endpoint_url": "https://10.0.0.1/mcp",
            "auth_type": "bearer",
            "credential": CREDENTIAL,
        }))
        .send()
        .await
        .expect("发送不安全 endpoint 请求");
    assert_eq!(unsafe_create.status(), StatusCode::BAD_REQUEST);
    let unsafe_body: Value = unsafe_create.json().await.expect("错误响应是 JSON");
    assert_eq!(unsafe_body["error"]["code"], json!("unsafe_endpoint"));
    checkpoint("负控：开发 loopback 模式仍拒绝私网地址 https://10.0.0.1/mcp");

    // 管理员注册远端连接并提交凭据；响应只返回 configured 状态。
    let created = admin_caller
        .apply(http.post(format!("{base}/api/v1/connections")))
        .json(&json!({
            "name": format!("remote-{stamp}"),
            "endpoint_url": upstream_url,
            "auth_type": "bearer",
            "credential": CREDENTIAL,
        }))
        .send()
        .await
        .expect("创建远端连接请求");
    assert_eq!(created.status(), StatusCode::CREATED);
    let created_text = created.text().await.expect("读取创建响应");
    assert!(
        !created_text.contains(CREDENTIAL),
        "创建响应不得包含凭据明文"
    );
    let created: Value = serde_json::from_str(&created_text).expect("创建响应是 JSON");
    assert_eq!(created["credential_configured"], json!(true));
    assert!(created.get("credential").is_none());
    checkpoint("创建响应只返回 credential_configured=true，不包含凭据字段或明文");
    let connection_id =
        Uuid::parse_str(created["id"].as_str().expect("响应包含连接 ID")).expect("连接 ID 是 UUID");

    // 数据库只保存密文。
    let pool = sqlx::PgPool::connect(&database_url)
        .await
        .expect("连接测试数据库");
    let row = sqlx::query(
        "SELECT auth_type, status, credential_ciphertext \
         FROM mcp_connections WHERE id = $1",
    )
    .bind(connection_id)
    .fetch_one(&pool)
    .await
    .expect("读取连接行");
    assert_eq!(row.get::<String, _>("auth_type"), "bearer");
    assert_eq!(row.get::<String, _>("status"), "active");
    let ciphertext: Option<Vec<u8>> = row.get("credential_ciphertext");
    let ciphertext = ciphertext.expect("Bearer 连接必须保存密文");
    assert!(!ciphertext.is_empty());
    assert_ne!(ciphertext, CREDENTIAL.as_bytes());
    assert!(
        !ciphertext
            .windows(CREDENTIAL.len())
            .any(|window| window == CREDENTIAL.as_bytes()),
        "数据库中不得出现凭据明文"
    );
    checkpoint(format!(
        "PostgreSQL 只保存密文：{} 字节且不含凭据明文",
        ciphertext.len()
    ));

    // 检查连接：服务解密凭据并通过真实 rmcp 握手发现工具。
    let checked = admin_caller
        .apply(http.get(format!("{base}/api/v1/connections/{connection_id}/tools")))
        .send()
        .await
        .expect("检查连接");
    assert_eq!(checked.status(), StatusCode::OK);
    let checked: Value = checked.json().await.expect("检查响应是 JSON");
    assert_eq!(checked["items"][0]["name"], json!(UPSTREAM_TOOL));
    checkpoint(format!(
        "管理 API 检查连接并通过真实 rmcp 握手发现远端工具 {UPSTREAM_TOOL}"
    ));

    let assignment = admin_caller
        .apply(http.put(format!(
            "{base}/api/v1/connections/{connection_id}/assignments/user/user-allowed"
        )))
        .send()
        .await
        .expect("创建用户授权");
    assert_eq!(assignment.status(), StatusCode::NO_CONTENT);

    // 重启服务进程，验证新进程从同一数据库读取连接并解密凭据。
    let first_logs = gateway.logs();
    drop(gateway);
    let mut restarted = RunningGateway::start(&database_url);
    restarted.wait_ready(&http).await;
    let base_after_restart = restarted.base_url.clone();

    let fresh = admin_caller
        .apply(http.get(format!(
            "{base_after_restart}/api/v1/connections/{connection_id}"
        )))
        .send()
        .await
        .expect("重启后读取连接");
    assert_eq!(fresh.status(), StatusCode::OK);
    let fresh: Value = fresh.json().await.expect("重启后响应是 JSON");
    assert_eq!(fresh["credential_configured"], json!(true));
    checkpoint("新建进程后读取同一连接成功，仍只报告 credential_configured=true");

    let client = gateway_client(&base_after_restart, &allowed_user).await;
    let tools = client.list_all_tools().await.expect("重启后列出工具");
    assert_eq!(tools.len(), 1, "实际工具：{tools:?}");
    let expected_tool = exposed_tool_name(connection_id, UPSTREAM_TOOL);
    assert_eq!(tools[0].name.as_ref(), expected_tool);
    let result = call_gateway_tool(&client, &expected_tool, json!({ "city": city }))
        .await
        .expect("重启后调用工具");
    let text = result
        .content
        .iter()
        .find_map(|content| match content {
            ContentBlock::Text(text) => Some(text.text.clone()),
            _ => None,
        })
        .expect("工具结果包含文本");
    assert_eq!(text, format!("upstream-result:{UPSTREAM_TOOL}:{city}"));
    checkpoint("服务重启后新进程从同一 PostgreSQL 读取连接、解密凭据并完成真实 tools/call");
    let _ = client.cancel().await;

    // Oracle 5：一条真实不可用的上游只跳过该连接，健康连接继续可用。
    let dead_port = free_port();
    let dead_created = admin_caller
        .apply(http.post(format!("{base_after_restart}/api/v1/connections")))
        .json(&json!({
            "name": format!("dead-{stamp}"),
            "endpoint_url": format!("http://127.0.0.1:{dead_port}/mcp"),
            "auth_type": "none",
        }))
        .send()
        .await
        .expect("创建不可用连接");
    assert_eq!(dead_created.status(), StatusCode::CREATED);
    let dead_created: Value = dead_created.json().await.expect("创建响应是 JSON");
    let dead_connection_id = Uuid::parse_str(dead_created["id"].as_str().expect("响应包含连接 ID"))
        .expect("连接 ID 是 UUID");
    let dead_assignment = admin_caller
        .apply(http.put(format!(
            "{base_after_restart}/api/v1/connections/{dead_connection_id}/assignments/user/user-allowed"
        )))
        .send()
        .await
        .expect("授权不可用连接");
    assert_eq!(dead_assignment.status(), StatusCode::NO_CONTENT);
    let dead_tool = exposed_tool_name(dead_connection_id, UPSTREAM_TOOL);

    let client = gateway_client(&base_after_restart, &allowed_user).await;
    let tools = client.list_all_tools().await.expect("列出工具");
    assert_eq!(tools.len(), 1, "不可用连接不得提供工具；实际：{tools:?}");
    assert_eq!(tools[0].name.as_ref(), expected_tool);
    assert_upstream_unavailable(call_gateway_tool(&client, &dead_tool, json!({})).await);
    let result = call_gateway_tool(&client, &expected_tool, json!({ "city": city }))
        .await
        .expect("不可用连接存在时健康连接仍可调用");
    let text = result
        .content
        .iter()
        .find_map(|content| match content {
            ContentBlock::Text(text) => Some(text.text.clone()),
            _ => None,
        })
        .expect("工具结果包含文本");
    assert_eq!(text, format!("upstream-result:{UPSTREAM_TOOL}:{city}"));
    checkpoint("负控：真实不可用的上游连接不提供工具，调用返回 -32005/502；健康连接仍可发现和调用");
    let _ = client.cancel().await;

    // 两次进程的日志都不得包含凭据或工具参数。
    let combined_logs = format!("{first_logs}\n{}", restarted.logs());
    assert!(
        !combined_logs.contains(CREDENTIAL),
        "服务日志不得包含凭据明文"
    );
    assert!(!combined_logs.contains(&city), "服务日志不得包含工具参数");
    assert!(
        !combined_logs.contains("upstream-result:"),
        "服务日志不得包含工具结果"
    );
    checkpoint("两次服务进程（RUST_LOG=debug）的日志均不含凭据明文、工具参数与工具结果");

    pool.close().await;
}

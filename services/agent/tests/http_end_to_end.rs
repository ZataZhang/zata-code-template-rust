//! 真实入口端到端测试：真实 PostgreSQL、真实 HTTP 服务、真实装配的服务层。
//!
//! 默认跳过；设置 `TEST_AGENT_DATABASE_URL` 后用 `just test real` 运行。
//! 测试覆盖：管理员建立组织/成员/Runtime/Agent/ACL，public 用户创建 Session 并发送消息，
//! Sandbox 执行 Run、产物下载、越权拒绝，以及 tracing outbox 通过真实 HTTP 投递并推进确认检查点。

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_trait::async_trait;
use axum::Router;
use axum::extract::{Path, State};
use axum::http::HeaderMap;
use axum::response::Json;
use axum::routing::post;
use reqwest::{Client, Method, StatusCode};
use serde_json::{Value, json};
use tokio::net::TcpListener;
use tokio::sync::watch;
use uuid::Uuid;

use tpl_agent_api::server::{AgentApiState, build_router};
use tpl_agent_core::error::AgentError;
use tpl_agent_core::identity::{
    AdminIdentity, AdminIdentityValidator, ServiceTokenValidator, UserIdentity,
    UserIdentityValidator,
};
use tpl_agent_core::ports::{
    AuditRepository, BlobStore, CatalogRepository, ConversationRepository, CredentialResolver,
    DirectoryRepository, ReadinessProbe, ResourceRepository, RunExecutor, RunRepository,
    RuntimeProbe,
};
use tpl_agent_core::runtime_catalog::{BuiltinRuntimeDeclaration, RuntimeAdapterKind};
use tpl_agent_core::service::{AgentService, AgentServiceConfig, AgentServiceDependencies};
use tpl_agent_engines::CompositeRunExecutor;
use tpl_agent_engines::a2a::A2aRuntimeProbe;
use tpl_agent_infrastructure::identity::{EnvironmentCredentialResolver, ServiceTokenRegistry};
use tpl_agent_infrastructure::postgres::PostgresAgentRepository;
use tpl_agent_infrastructure::storage::FilesystemBlobStore;
use tpl_agent_infrastructure::tracing_outbox::{TracingIngestClient, TracingOutboxWorker};

/// 调用服务 Bearer token；长度满足服务端最小要求。
const SERVICE_TOKEN: &str = "agent-e2e-service-token-0123456789abcdef";
/// tracing producer token。
const PRODUCER_TOKEN: &str = "agent-e2e-producer-token-0123456789abcdef";
/// 平台管理员账号。
const PLATFORM_ADMIN_ID: i64 = 1;
/// 普通成员账号。
const MEMBER_USER_ID: i64 = 42;

struct TestUsers;

#[async_trait]
impl UserIdentityValidator for TestUsers {
    async fn authenticate_user(
        &self,
        cookie_header: Option<&str>,
    ) -> Result<UserIdentity, AgentError> {
        let user_id = cookie_value(cookie_header, "agent_test_user")
            .and_then(|value| value.parse::<i64>().ok())
            .ok_or(AgentError::Unauthenticated)?;
        Ok(UserIdentity {
            user_id,
            email: format!("user-{user_id}@example.test"),
        })
    }
}

struct TestAdmins;

#[async_trait]
impl AdminIdentityValidator for TestAdmins {
    async fn authenticate_admin(
        &self,
        cookie_header: Option<&str>,
    ) -> Result<AdminIdentity, AgentError> {
        let admin_user_id = cookie_value(cookie_header, "agent_test_admin")
            .and_then(|value| value.parse::<i64>().ok())
            .ok_or(AgentError::Unauthenticated)?;
        Ok(AdminIdentity {
            admin_user_id,
            display_name: format!("admin-{admin_user_id}"),
        })
    }
}

fn cookie_value(cookie_header: Option<&str>, name: &str) -> Option<String> {
    cookie_header?.split(';').find_map(|part| {
        let (key, value) = part.trim().split_once('=')?;
        (key == name).then(|| value.to_owned())
    })
}

fn admin_database_url() -> String {
    std::env::var("TEST_AGENT_DATABASE_URL")
        .expect("请设置 TEST_AGENT_DATABASE_URL（Agent 专属测试库，见 docker-compose.testing.yml）")
}

async fn truncate_agent_tables(database_url: &str) {
    let pool = sqlx::PgPool::connect(database_url).await.unwrap();
    sqlx::query(
        "TRUNCATE TABLE agent_audit_records, run_artifacts, user_resources, \
         agent_run_idempotency, agent_trace_outbox, agent_run_events, agent_runs, \
         chat_messages, chat_sessions, agent_skill_assignments, agent_skill_assets, \
         agent_skills, agent_delegation_policies, agent_entry_policies, \
         agent_access_rules, business_agents, runtime_connections, \
         organization_admin_scopes, organization_memberships, organization_units, \
         organizations CASCADE",
    )
    .execute(&pool)
    .await
    .unwrap();
}

struct RunningAgent {
    base_url: String,
    service: Arc<AgentService>,
    runs: Arc<dyn RunRepository>,
}

async fn start_agent(database_url: &str, storage_root: &std::path::Path) -> RunningAgent {
    let repository = Arc::new(
        PostgresAgentRepository::connect(database_url, 5, Duration::from_secs(5))
            .await
            .unwrap(),
    );
    repository.run_migrations().await.unwrap();
    truncate_agent_tables(database_url).await;

    let directory: Arc<dyn DirectoryRepository> = repository.clone();
    let catalog: Arc<dyn CatalogRepository> = repository.clone();
    let conversations: Arc<dyn ConversationRepository> = repository.clone();
    let runs: Arc<dyn RunRepository> = repository.clone();
    let resources: Arc<dyn ResourceRepository> = repository.clone();
    let audit: Arc<dyn AuditRepository> = repository.clone();
    let readiness: Arc<dyn ReadinessProbe> = repository.clone();

    let credential_resolver: Arc<dyn CredentialResolver> = Arc::new(EnvironmentCredentialResolver);
    let executor: Arc<dyn RunExecutor> =
        Arc::new(CompositeRunExecutor::new(Arc::clone(&credential_resolver)).unwrap());
    let runtime_probe: Arc<dyn RuntimeProbe> =
        Arc::new(A2aRuntimeProbe::new(Arc::clone(&credential_resolver)).unwrap());
    let blobs: Arc<dyn BlobStore> = Arc::new(FilesystemBlobStore::new(storage_root));

    let users: Arc<dyn UserIdentityValidator> = Arc::new(TestUsers);
    let admins: Arc<dyn AdminIdentityValidator> = Arc::new(TestAdmins);
    let service = Arc::new(AgentService::new(AgentServiceDependencies {
        directory,
        catalog,
        conversations,
        runs: Arc::clone(&runs),
        resources,
        audit,
        blobs,
        executor,
        runtime_probe,
        users: Arc::clone(&users),
        admins: Arc::clone(&admins),
        config: AgentServiceConfig {
            platform_admin_ids: vec![PLATFORM_ADMIN_ID],
            max_resource_bytes: 1_048_576,
            max_artifact_bytes: 1_048_576,
            builtin_runtimes: vec![BuiltinRuntimeDeclaration {
                name: "platform-sandbox".to_owned(),
                display_name: "Platform Sandbox".to_owned(),
                adapter_kind: RuntimeAdapterKind::Sandbox,
                enabled: true,
                allowed_tools: vec!["text.summarize".to_owned(), "artifact.write".to_owned()],
                max_steps: 8,
            }],
        },
    }));
    service.initialize().await.unwrap();

    let services: Arc<dyn ServiceTokenValidator> = Arc::new(ServiceTokenRegistry::new(
        BTreeMap::from([("agent-e2e".to_owned(), SERVICE_TOKEN.to_owned())]),
    ));
    let router = build_router(AgentApiState {
        service: Arc::clone(&service),
        services,
        users,
        admins,
        readiness,
        max_upload_bytes: 1_048_576,
    });
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, router).await.unwrap();
    });

    RunningAgent {
        base_url: format!("http://{address}"),
        service,
        runs,
    }
}

fn admin_request(client: &Client, method: Method, url: String) -> reqwest::RequestBuilder {
    client
        .request(method, url)
        .bearer_auth(SERVICE_TOKEN)
        .header(
            reqwest::header::COOKIE,
            format!("agent_test_admin={PLATFORM_ADMIN_ID}"),
        )
}

fn user_request(
    client: &Client,
    method: Method,
    url: String,
    user_id: i64,
) -> reqwest::RequestBuilder {
    client
        .request(method, url)
        .bearer_auth(SERVICE_TOKEN)
        .header(
            reqwest::header::COOKIE,
            format!("agent_test_user={user_id}"),
        )
}

async fn response_json(response: reqwest::Response) -> Value {
    response.json().await.unwrap_or(Value::Null)
}

fn multipart_file_body(filename: &str, media_type: &str, bytes: &[u8]) -> (String, Vec<u8>) {
    let boundary = format!("----agent-e2e-{}", Uuid::new_v4().simple());
    let mut body = Vec::with_capacity(bytes.len() + 256);
    body.extend_from_slice(
        format!(
            "--{boundary}\r\nContent-Disposition: form-data; name=\"file\"; filename=\"{filename}\"\r\nContent-Type: {media_type}\r\n\r\n"
        )
        .as_bytes(),
    );
    body.extend_from_slice(bytes);
    body.extend_from_slice(format!("\r\n--{boundary}--\r\n").as_bytes());
    (format!("multipart/form-data; boundary={boundary}"), body)
}

#[derive(Clone)]
struct TraceSink {
    requests: Arc<Mutex<Vec<TraceRequest>>>,
}

struct TraceRequest {
    run_id: Uuid,
    authorization: String,
    body: Value,
}

async fn trace_ingest(
    State(sink): State<TraceSink>,
    Path(run_id): Path<Uuid>,
    headers: HeaderMap,
    Json(body): Json<Value>,
) -> Json<Value> {
    let accepted_through_seq = body["events"]
        .as_array()
        .and_then(|events| events.last())
        .and_then(|event| event["seq"].as_i64())
        .unwrap_or(0);
    let authorization = headers
        .get(reqwest::header::AUTHORIZATION)
        .and_then(|value| value.to_str().ok())
        .unwrap_or_default()
        .to_owned();
    sink.requests.lock().unwrap().push(TraceRequest {
        run_id,
        authorization,
        body,
    });
    Json(json!({"accepted_through_seq": accepted_through_seq}))
}

async fn start_trace_sink() -> (String, TraceSink) {
    let sink = TraceSink {
        requests: Arc::new(Mutex::new(Vec::new())),
    };
    let router = Router::new()
        .route("/runs/{run_id}/events", post(trace_ingest))
        .with_state(sink.clone());
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, router).await.unwrap();
    });
    (format!("http://{address}"), sink)
}

#[tokio::test]
#[ignore = "需要真实 PostgreSQL，设置 TEST_AGENT_DATABASE_URL 后运行"]
async fn real_http_session_run_authorization_and_tracing() {
    let database_url = admin_database_url();

    let storage_root = std::env::temp_dir().join(format!("agent-e2e-{}", Uuid::new_v4()));
    std::fs::create_dir_all(&storage_root).unwrap();

    let agent = start_agent(&database_url, &storage_root).await;
    let client = Client::builder().build().unwrap();
    let base = &agent.base_url;

    // 1) 平台管理员创建组织并授予自己 scope。
    let created_organization = admin_request(
        &client,
        Method::POST,
        format!("{base}/admin/api/organizations"),
    )
    .json(&json!({
        "slug": format!("e2e-{}", Uuid::new_v4().simple()),
        "display_name": "E2E Organization",
    }))
    .send()
    .await
    .unwrap();
    assert_eq!(created_organization.status(), StatusCode::CREATED);
    let organization_id = response_json(created_organization).await["id"]
        .as_str()
        .unwrap()
        .to_owned();

    let granted_scope = admin_request(
        &client,
        Method::POST,
        format!("{base}/admin/api/organizations/{organization_id}/admin-scopes"),
    )
    .json(&json!({"admin_user_id": PLATFORM_ADMIN_ID}))
    .send()
    .await
    .unwrap();
    assert_eq!(granted_scope.status(), StatusCode::CREATED);

    // 2) 显式成员关系（upsert 幂等，返回 200）。
    let membership = admin_request(
        &client,
        Method::POST,
        format!("{base}/admin/api/organizations/{organization_id}/memberships"),
    )
    .json(&json!({
        "user_id": MEMBER_USER_ID,
        "display_name": "E2E Member",
    }))
    .send()
    .await
    .unwrap();
    assert_eq!(membership.status(), StatusCode::OK);

    // 3) 发现内置 Runtime 并创建绑定该 Runtime 的 Agent。
    let runtimes = admin_request(
        &client,
        Method::GET,
        format!("{base}/admin/api/organizations/{organization_id}/runtimes"),
    )
    .send()
    .await
    .unwrap();
    assert_eq!(runtimes.status(), StatusCode::OK);
    let runtimes = response_json(runtimes).await;
    let sandbox_runtime = runtimes["items"]
        .as_array()
        .unwrap()
        .iter()
        .find(|runtime| runtime["name"] == "platform-sandbox")
        .expect("内置 platform-sandbox 应已声明")
        .clone();
    assert_eq!(sandbox_runtime["status"], "ready");
    let runtime_id = sandbox_runtime["id"].as_str().unwrap().to_owned();

    let created_agent = admin_request(
        &client,
        Method::POST,
        format!("{base}/admin/api/organizations/{organization_id}/agents"),
    )
    .json(&json!({
        "runtime_connection_id": runtime_id,
        "name": "e2e-assistant",
        "display_name": "E2E Assistant",
        "description": "integration test agent",
        "prompt": "Summarize the user's message.",
    }))
    .send()
    .await
    .unwrap();
    assert_eq!(created_agent.status(), StatusCode::CREATED);
    let agent_id = response_json(created_agent).await["id"]
        .as_str()
        .unwrap()
        .to_owned();

    // 4) 访问规则：组织 allow；不允许的 Agent 不被其他组织成员看到。
    let replaced_rules = admin_request(
        &client,
        Method::PUT,
        format!("{base}/admin/api/organizations/{organization_id}/agents/{agent_id}/access-rules"),
    )
    .json(&json!({
        "rules": [{
            "effect": "allow",
            "subject": {"kind": "organization", "id": organization_id},
        }],
    }))
    .send()
    .await
    .unwrap();
    assert_eq!(replaced_rules.status(), StatusCode::OK);

    // 5) 成员可见；非成员被拒绝。
    let available = user_request(
        &client,
        Method::GET,
        format!("{base}/api/v1/organizations/{organization_id}/agents"),
        MEMBER_USER_ID,
    )
    .send()
    .await
    .unwrap();
    assert_eq!(available.status(), StatusCode::OK);
    assert!(
        response_json(available).await["items"]
            .as_array()
            .unwrap()
            .iter()
            .any(|item| item["agent"]["id"] == agent_id),
        "成员应能看到已授权 Agent"
    );

    let denied = user_request(
        &client,
        Method::GET,
        format!("{base}/api/v1/organizations/{organization_id}/agents"),
        MEMBER_USER_ID + 1,
    )
    .send()
    .await
    .unwrap();
    assert_eq!(denied.status(), StatusCode::FORBIDDEN);
    assert_eq!(
        response_json(denied).await["error"]["code"],
        "organization_membership_required"
    );

    // 6) 创建 Session 并发送消息；Run 真实执行并产出事件与产物。
    let created_session = user_request(
        &client,
        Method::POST,
        format!("{base}/api/v1/organizations/{organization_id}/sessions"),
        MEMBER_USER_ID,
    )
    .json(&json!({"agent_id": agent_id}))
    .send()
    .await
    .unwrap();
    assert_eq!(created_session.status(), StatusCode::CREATED);
    let session_id = response_json(created_session).await["id"]
        .as_str()
        .unwrap()
        .to_owned();

    // 6.1) 请求体与上传边界：JSON 256 KiB、上传 1 MiB（测试配置）。
    let oversized_json = user_request(
        &client,
        Method::POST,
        format!("{base}/api/v1/sessions/{session_id}/messages"),
        MEMBER_USER_ID,
    )
    .header("idempotency-key", Uuid::new_v4().to_string())
    .json(&json!({
        "content": "x".repeat(300 * 1024),
        "resource_ids": [],
        "skill_ids": [],
    }))
    .send()
    .await
    .unwrap();
    assert_eq!(oversized_json.status(), StatusCode::PAYLOAD_TOO_LARGE);
    assert_eq!(
        response_json(oversized_json).await["error"]["code"],
        "payload_too_large"
    );

    let (content_type, body) =
        multipart_file_body("within-limit.txt", "text/plain", &[b'a'; 300 * 1024]);
    let uploaded = user_request(
        &client,
        Method::POST,
        format!("{base}/api/v1/organizations/{organization_id}/resources"),
        MEMBER_USER_ID,
    )
    .header(reqwest::header::CONTENT_TYPE, content_type)
    .body(body)
    .send()
    .await
    .unwrap();
    assert_eq!(uploaded.status(), StatusCode::CREATED);

    let (content_type, body) =
        multipart_file_body("over-limit.txt", "text/plain", &[b'a'; 1_048_576 + 1]);
    let oversized_upload = user_request(
        &client,
        Method::POST,
        format!("{base}/api/v1/organizations/{organization_id}/resources"),
        MEMBER_USER_ID,
    )
    .header(reqwest::header::CONTENT_TYPE, content_type)
    .body(body)
    .send()
    .await
    .unwrap();
    assert_eq!(oversized_upload.status(), StatusCode::PAYLOAD_TOO_LARGE);
    assert_eq!(
        response_json(oversized_upload).await["error"]["code"],
        "payload_too_large"
    );

    let content = "Please summarize the launch plan.\n\n```artifact filename=\"report.md\" media_type=\"text/markdown\"\n# Report\n\nLaunch is on track.\n```";
    let created_run = user_request(
        &client,
        Method::POST,
        format!("{base}/api/v1/sessions/{session_id}/messages"),
        MEMBER_USER_ID,
    )
    .header("idempotency-key", Uuid::new_v4().to_string())
    .json(&json!({
        "content": content,
        "resource_ids": [],
        "skill_ids": [],
    }))
    .send()
    .await
    .unwrap();
    assert_eq!(created_run.status(), StatusCode::CREATED);
    let created_run = response_json(created_run).await;
    assert_eq!(created_run["created"], true);
    let run_id = created_run["run"]["run_id"].as_str().unwrap().to_owned();
    assert_eq!(
        created_run["run"]["snapshot"]["agent"]["runtime"]["adapter_kind"],
        "sandbox"
    );

    agent
        .service
        .execute_run(Uuid::parse_str(&run_id).unwrap())
        .await
        .unwrap();

    let finished_run = user_request(
        &client,
        Method::GET,
        format!("{base}/api/v1/runs/{run_id}"),
        MEMBER_USER_ID,
    )
    .send()
    .await
    .unwrap();
    assert_eq!(finished_run.status(), StatusCode::OK);
    let finished_run = response_json(finished_run).await;
    assert_eq!(finished_run["status"], "succeeded");
    assert!(finished_run["last_event_seq"].as_i64().unwrap() > 0);

    let events = user_request(
        &client,
        Method::GET,
        format!("{base}/api/v1/runs/{run_id}/events"),
        MEMBER_USER_ID,
    )
    .send()
    .await
    .unwrap();
    assert_eq!(events.status(), StatusCode::OK);
    let events = response_json(events).await;
    let event_types: Vec<&str> = events["items"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|event| event["event_type"].as_str())
        .collect();
    assert!(event_types.contains(&"run.completed"));
    assert!(event_types.contains(&"tool.call.completed"));

    let artifacts = user_request(
        &client,
        Method::GET,
        format!("{base}/api/v1/runs/{run_id}/artifacts"),
        MEMBER_USER_ID,
    )
    .send()
    .await
    .unwrap();
    assert_eq!(artifacts.status(), StatusCode::OK);
    let artifacts = response_json(artifacts).await;
    assert_eq!(artifacts["items"].as_array().unwrap().len(), 1);
    assert_eq!(artifacts["items"][0]["filename"], "report.md");

    // 7) tracing outbox 通过真实 HTTP 投递，并推进确认检查点。
    let (trace_url, sink) = start_trace_sink().await;
    let ingest_client = Arc::new(
        TracingIngestClient::new(trace_url, PRODUCER_TOKEN.to_owned(), Duration::from_secs(5))
            .unwrap(),
    );
    let (shutdown_sender, shutdown_receiver) = watch::channel(false);
    let worker = TracingOutboxWorker::new(
        Arc::clone(&agent.runs),
        ingest_client,
        Duration::from_millis(100),
    );
    let worker_handle = tokio::spawn(worker.run(shutdown_receiver));

    let mut delivered = false;
    for _ in 0..100 {
        let run = user_request(
            &client,
            Method::GET,
            format!("{base}/api/v1/runs/{run_id}"),
            MEMBER_USER_ID,
        )
        .send()
        .await
        .unwrap();
        let run = response_json(run).await;
        let delivered_through = run["trace_delivered_through_seq"].as_i64().unwrap();
        if delivered_through == run["last_event_seq"].as_i64().unwrap() {
            delivered = true;
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    let _ = shutdown_sender.send(true);
    let _ = worker_handle.await;

    assert!(delivered, "tracing outbox 应推进到 last_event_seq");
    let requests = sink.requests.lock().unwrap();
    assert_eq!(requests.len(), 1, "应投递一个 Run 的事件批次");
    assert_eq!(requests[0].run_id.to_string(), run_id);
    assert_eq!(
        requests[0].authorization,
        format!("Bearer {PRODUCER_TOKEN}")
    );
    assert!(
        !requests[0].body["events"].as_array().unwrap().is_empty(),
        "tracing 批次应包含事件"
    );

    std::fs::remove_dir_all(&storage_root).ok();
}

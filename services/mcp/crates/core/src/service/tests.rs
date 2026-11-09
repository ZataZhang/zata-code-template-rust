//! 用例级测试：用内存仓储与假外部依赖验证授权、凭据与工具聚合规则。
//!
//! 这些测试不连接数据库，也不访问网络；真实 PostgreSQL 与真实 MCP 协议由
//! `services/mcp/tests/` 与 `just mcp-e2e` 覆盖。

use std::collections::{HashMap, HashSet};
use std::net::SocketAddr;
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use serde_json::{Value, json};
use time::OffsetDateTime;
use url::Url;
use uuid::Uuid;

use super::{McpService, McpServiceConfig, McpServiceDependencies};
use crate::connection::{
    Assignment, AssignmentSubject, AuthType, Connection, ConnectionChanges, ConnectionStatus,
    NewConnection, TenantScope,
};
use crate::error::McpError;
use crate::identity::{CallerRole, IdentityContext};
use crate::ids::{ConsumerId, GroupId, SubjectId, TenantId};
use crate::ports::{
    ConnectionRepository, CredentialCipher, CredentialScope, CryptoError, EndpointPolicy,
    EndpointPolicyError, RemoteMcpClient, RemoteMcpError, RemoteTarget, RepositoryError,
    ValidatedEndpoint,
};
use crate::secret::{CredentialCiphertext, SecretString};
use crate::tool::{RemoteToolDefinition, ToolCallOutcome, exposed_tool_name};

/// 测试用内存仓储。
#[derive(Default)]
struct Store {
    connections: Vec<Connection>,
    assignments: Vec<Assignment>,
}

#[derive(Clone, Default)]
struct FakeRepository {
    store: Arc<Mutex<Store>>,
}

impl FakeRepository {
    fn with_store<T>(&self, action: impl FnOnce(&mut Store) -> T) -> T {
        let mut store = self.store.lock().expect("测试仓储锁可用");
        action(&mut store)
    }

    fn stored_connections(&self) -> Vec<Connection> {
        self.with_store(|store| store.connections.clone())
    }
}

#[async_trait]
impl ConnectionRepository for FakeRepository {
    async fn create(&self, connection: &Connection) -> Result<(), RepositoryError> {
        self.with_store(|store| {
            let duplicate = store.connections.iter().any(|existing| {
                existing.scope == connection.scope && existing.name == connection.name
            });
            if duplicate {
                return Err(RepositoryError::Conflict);
            }
            store.connections.push(connection.clone());
            Ok(())
        })
    }

    async fn update(&self, connection: &Connection) -> Result<(), RepositoryError> {
        self.with_store(|store| {
            let existing = store
                .connections
                .iter_mut()
                .find(|existing| existing.id == connection.id && existing.scope == connection.scope)
                .ok_or(RepositoryError::NotFound)?;
            *existing = connection.clone();
            Ok(())
        })
    }

    async fn delete(
        &self,
        scope: &TenantScope,
        connection_id: Uuid,
    ) -> Result<(), RepositoryError> {
        self.with_store(|store| {
            let before = store.connections.len();
            store
                .connections
                .retain(|c| !(c.id == connection_id && c.scope == *scope));
            if store.connections.len() == before {
                return Err(RepositoryError::NotFound);
            }
            store
                .assignments
                .retain(|assignment| assignment.connection_id != connection_id);
            Ok(())
        })
    }

    async fn find(
        &self,
        scope: &TenantScope,
        connection_id: Uuid,
    ) -> Result<Connection, RepositoryError> {
        self.with_store(|store| {
            store
                .connections
                .iter()
                .find(|c| c.id == connection_id && c.scope == *scope)
                .cloned()
                .ok_or(RepositoryError::NotFound)
        })
    }

    async fn list(&self, scope: &TenantScope) -> Result<Vec<Connection>, RepositoryError> {
        self.with_store(|store| {
            Ok(store
                .connections
                .iter()
                .filter(|c| c.scope == *scope)
                .cloned()
                .collect())
        })
    }

    async fn list_authorized(
        &self,
        scope: &TenantScope,
        subjects: &[AssignmentSubject],
    ) -> Result<Vec<Connection>, RepositoryError> {
        self.with_store(|store| {
            Ok(store
                .connections
                .iter()
                .filter(|connection| {
                    connection.scope == *scope
                        && connection.is_active()
                        && store.assignments.iter().any(|assignment| {
                            assignment.connection_id == connection.id
                                && subjects.contains(&assignment.subject)
                        })
                })
                .cloned()
                .collect())
        })
    }

    async fn is_authorized(
        &self,
        scope: &TenantScope,
        connection_id: Uuid,
        subjects: &[AssignmentSubject],
    ) -> Result<bool, RepositoryError> {
        self.with_store(|store| {
            let connection = store
                .connections
                .iter()
                .find(|c| c.id == connection_id && c.scope == *scope)
                .ok_or(RepositoryError::NotFound)?;
            if !connection.is_active() {
                return Ok(false);
            }
            Ok(store.assignments.iter().any(|assignment| {
                assignment.connection_id == connection_id && subjects.contains(&assignment.subject)
            }))
        })
    }

    async fn list_assignments(
        &self,
        scope: &TenantScope,
        connection_id: Uuid,
    ) -> Result<Vec<Assignment>, RepositoryError> {
        self.with_store(|store| {
            let exists = store
                .connections
                .iter()
                .any(|c| c.id == connection_id && c.scope == *scope);
            if !exists {
                return Err(RepositoryError::NotFound);
            }
            Ok(store
                .assignments
                .iter()
                .filter(|assignment| assignment.connection_id == connection_id)
                .cloned()
                .collect())
        })
    }

    async fn add_assignment(
        &self,
        scope: &TenantScope,
        assignment: &Assignment,
    ) -> Result<(), RepositoryError> {
        self.with_store(|store| {
            let exists = store
                .connections
                .iter()
                .any(|c| c.id == assignment.connection_id && c.scope == *scope);
            if !exists {
                return Err(RepositoryError::NotFound);
            }
            if !store.assignments.contains(assignment) {
                store.assignments.push(assignment.clone());
            }
            Ok(())
        })
    }

    async fn remove_assignment(
        &self,
        scope: &TenantScope,
        connection_id: Uuid,
        subject: &AssignmentSubject,
    ) -> Result<(), RepositoryError> {
        self.with_store(|store| {
            let exists = store
                .connections
                .iter()
                .any(|c| c.id == connection_id && c.scope == *scope);
            if !exists {
                return Err(RepositoryError::NotFound);
            }
            store.assignments.retain(|assignment| {
                !(assignment.connection_id == connection_id && assignment.subject == *subject)
            });
            Ok(())
        })
    }
}

/// 测试用可逆「加密」：十六进制编码并加固定前缀，确保仓储中不出现明文。
struct HexCipher;

const CIPHERTEXT_PREFIX: &str = "enc:";

impl HexCipher {
    fn encode(plaintext: &str) -> Vec<u8> {
        let mut encoded = CIPHERTEXT_PREFIX.as_bytes().to_vec();
        for byte in plaintext.as_bytes() {
            encoded.extend_from_slice(format!("{byte:02x}").as_bytes());
        }
        encoded
    }

    fn decode(ciphertext: &[u8]) -> Option<String> {
        let hex = std::str::from_utf8(ciphertext)
            .ok()?
            .strip_prefix(CIPHERTEXT_PREFIX)?;
        let decoded: Option<Vec<u8>> = (0..hex.len())
            .step_by(2)
            .map(|index| u8::from_str_radix(hex.get(index..index + 2)?, 16).ok())
            .collect();
        String::from_utf8(decoded?).ok()
    }
}

impl CredentialCipher for HexCipher {
    fn encrypt(
        &self,
        plaintext: &SecretString,
        _scope: &CredentialScope<'_>,
    ) -> Result<CredentialCiphertext, CryptoError> {
        Ok(CredentialCiphertext::new(Self::encode(plaintext.expose())))
    }

    fn decrypt(
        &self,
        ciphertext: &CredentialCiphertext,
        _scope: &CredentialScope<'_>,
    ) -> Result<SecretString, CryptoError> {
        let ciphertext = ciphertext.expose().ok_or(CryptoError::Decrypt)?;
        Self::decode(ciphertext)
            .map(SecretString::new)
            .ok_or(CryptoError::Decrypt)
    }
}

/// 测试用出站策略：只接受 HTTPS，并固定一个文档示例地址。
struct HttpsOnlyEndpointPolicy;

#[async_trait]
impl EndpointPolicy for HttpsOnlyEndpointPolicy {
    async fn validate(&self, endpoint: &Url) -> Result<ValidatedEndpoint, EndpointPolicyError> {
        if endpoint.scheme() != "https" {
            return Err(EndpointPolicyError::InsecureScheme);
        }
        Ok(ValidatedEndpoint {
            url: endpoint.clone(),
            // 203.0.113.0/24 是文档示例网段，测试不会真正连接这里。
            addresses: vec![SocketAddr::from(([203, 0, 113, 10], 443))],
        })
    }
}

/// 一次远端调用的记录：连接 ID、远端工具名与参数。
type RecordedCall = (Uuid, String, Option<Value>);

/// 测试用远端 MCP 客户端；按连接返回预设工具并记录调用。
#[derive(Clone, Default)]
struct FakeRemoteClient {
    tools: Arc<Mutex<HashMap<Uuid, Vec<String>>>>,
    listing_barrier: Arc<Mutex<Option<Arc<tokio::sync::Barrier>>>>,
    failing: Arc<Mutex<HashSet<Uuid>>>,
    calls: Arc<Mutex<Vec<RecordedCall>>>,
    seen_credentials: Arc<Mutex<Vec<Option<String>>>>,
}

impl FakeRemoteClient {
    fn set_tools(&self, connection_id: Uuid, tools: &[&str]) {
        self.tools.lock().expect("测试工具锁可用").insert(
            connection_id,
            tools.iter().map(|name| (*name).to_owned()).collect(),
        );
    }

    fn set_failing(&self, connection_id: Uuid) {
        self.failing
            .lock()
            .expect("测试失败集合锁可用")
            .insert(connection_id);
    }

    fn recorded_calls(&self) -> Vec<RecordedCall> {
        self.calls.lock().expect("测试调用记录锁可用").clone()
    }

    fn recorded_credentials(&self) -> Vec<Option<String>> {
        self.seen_credentials
            .lock()
            .expect("测试凭据记录锁可用")
            .clone()
    }

    fn is_failing(&self, connection_id: Uuid) -> bool {
        self.failing
            .lock()
            .expect("测试失败集合锁可用")
            .contains(&connection_id)
    }

    fn remember_credential(&self, target: &RemoteTarget) {
        self.seen_credentials
            .lock()
            .expect("测试凭据记录锁可用")
            .push(
                target
                    .credential
                    .as_ref()
                    .map(|secret| secret.expose().to_owned()),
            );
    }
}

#[async_trait]
impl RemoteMcpClient for FakeRemoteClient {
    async fn list_tools(
        &self,
        target: &RemoteTarget,
    ) -> Result<Vec<RemoteToolDefinition>, RemoteMcpError> {
        self.remember_credential(target);
        let listing_barrier = self.listing_barrier.lock().expect("测试同步锁可用").clone();
        if let Some(barrier) = listing_barrier {
            barrier.wait().await;
        }
        if self.is_failing(target.connection_id) {
            return Err(RemoteMcpError::Unavailable);
        }
        let tools = self
            .tools
            .lock()
            .expect("测试工具锁可用")
            .get(&target.connection_id)
            .cloned()
            .unwrap_or_default();
        Ok(tools
            .into_iter()
            .map(|name| RemoteToolDefinition {
                definition: json!({
                    "name": name,
                    "description": "测试用远端工具",
                    "inputSchema": { "type": "object" },
                }),
                name,
            })
            .collect())
    }

    async fn call_tool(
        &self,
        target: &RemoteTarget,
        tool_name: &str,
        arguments: Option<Value>,
    ) -> Result<ToolCallOutcome, RemoteMcpError> {
        self.remember_credential(target);
        if self.is_failing(target.connection_id) {
            return Err(RemoteMcpError::Unavailable);
        }
        let known = self
            .tools
            .lock()
            .expect("测试工具锁可用")
            .get(&target.connection_id)
            .is_some_and(|tools| tools.iter().any(|name| name == tool_name));
        if !known {
            return Err(RemoteMcpError::ToolNotFound);
        }
        self.calls.lock().expect("测试调用记录锁可用").push((
            target.connection_id,
            tool_name.to_owned(),
            arguments,
        ));
        Ok(ToolCallOutcome {
            is_error: false,
            result: json!({
                "content": [{ "type": "text", "text": format!("ok:{tool_name}") }],
                "isError": false,
            }),
        })
    }
}

struct TestRig {
    service: Arc<McpService>,
    repository: FakeRepository,
    remote: FakeRemoteClient,
}

impl TestRig {
    fn new() -> Self {
        let repository = FakeRepository::default();
        let remote = FakeRemoteClient::default();
        let service = Arc::new(McpService::new(McpServiceDependencies {
            repository: Arc::new(repository.clone()),
            cipher: Arc::new(HexCipher),
            endpoint_policy: Arc::new(HttpsOnlyEndpointPolicy),
            remote_client: Arc::new(remote.clone()),
            config: McpServiceConfig::default(),
        }));
        Self {
            service,
            repository,
            remote,
        }
    }

    fn seed_connection(&self, name: &str, status: ConnectionStatus) -> Connection {
        let connection = Connection {
            id: Uuid::new_v4(),
            scope: tenant_scope(),
            name: name.to_owned(),
            endpoint_url: Url::parse(&format!("https://{name}.example.com/mcp"))
                .expect("测试地址有效"),
            auth_type: AuthType::None,
            credential: CredentialCiphertext::none(),
            status,
            created_by: SubjectId::new("admin-1").expect("测试主体有效"),
            created_at: OffsetDateTime::now_utc(),
            updated_at: OffsetDateTime::now_utc(),
        };
        self.repository.with_store(|store| {
            store.connections.push(connection.clone());
        });
        connection
    }

    fn authorize(&self, connection_id: Uuid, subject: AssignmentSubject) {
        self.repository.with_store(|store| {
            store.assignments.push(Assignment {
                connection_id,
                subject,
            });
        });
    }
}

fn tenant_scope() -> TenantScope {
    TenantScope {
        consumer_id: ConsumerId::new("consumer-a").expect("测试消费服务有效"),
        tenant_id: TenantId::new("tenant-a").expect("测试租户有效"),
    }
}

fn admin_identity() -> IdentityContext {
    IdentityContext {
        consumer_id: ConsumerId::new("consumer-a").expect("测试消费服务有效"),
        tenant_id: TenantId::new("tenant-a").expect("测试租户有效"),
        subject_id: SubjectId::new("admin-1").expect("测试主体有效"),
        groups: Vec::new(),
        role: CallerRole::Admin,
    }
}

fn user_identity(groups: &[&str]) -> IdentityContext {
    IdentityContext {
        consumer_id: ConsumerId::new("consumer-a").expect("测试消费服务有效"),
        tenant_id: TenantId::new("tenant-a").expect("测试租户有效"),
        subject_id: SubjectId::new("user-1").expect("测试主体有效"),
        groups: groups
            .iter()
            .map(|group| GroupId::new(*group).expect("测试分组有效"))
            .collect(),
        role: CallerRole::User,
    }
}

fn new_connection(name: &str, auth_type: AuthType, credential: Option<&str>) -> NewConnection {
    NewConnection {
        name: name.to_owned(),
        endpoint_url: Url::parse("https://remote.example.com/mcp").expect("测试地址有效"),
        auth_type,
        credential: credential.map(SecretString::new),
    }
}

#[tokio::test]
async fn management_requires_admin_role() {
    let rig = TestRig::new();
    let error = rig
        .service
        .create_connection(
            &user_identity(&[]),
            new_connection("docs", AuthType::None, None),
        )
        .await
        .expect_err("普通用户不能管理连接");
    assert_eq!(error, McpError::Forbidden);
    assert!(rig.repository.stored_connections().is_empty());

    rig.service
        .create_connection(
            &admin_identity(),
            new_connection("docs", AuthType::None, None),
        )
        .await
        .expect("管理员可以创建连接");
    assert_eq!(rig.repository.stored_connections().len(), 1);
}

#[tokio::test]
async fn credentials_are_encrypted_at_rest_and_redacted() {
    let rig = TestRig::new();
    let created = rig
        .service
        .create_connection(
            &admin_identity(),
            new_connection("docs", AuthType::Bearer, Some("super-secret-token")),
        )
        .await
        .expect("创建带凭据的连接");
    assert!(created.credential_configured());

    let stored = rig.repository.stored_connections().remove(0);
    let ciphertext = stored.credential.expose().expect("应保存密文");
    assert_ne!(ciphertext, b"super-secret-token".as_slice());
    assert!(
        !ciphertext
            .windows(b"super-secret-token".len())
            .any(|window| window == b"super-secret-token".as_slice()),
        "仓储中不应出现凭据明文"
    );
    assert!(
        !format!("{stored:?}").contains("super-secret-token"),
        "连接 Debug 输出不应包含凭据"
    );

    rig.service
        .check_connection(&admin_identity(), created.id)
        .await
        .expect("检查连接时解密凭据");
    let seen = rig.remote.recorded_credentials();
    assert_eq!(seen, vec![Some("super-secret-token".to_owned())]);
}

#[tokio::test]
async fn auth_type_and_credential_must_match() {
    let rig = TestRig::new();
    let anonymous_with_secret = rig
        .service
        .create_connection(
            &admin_identity(),
            new_connection("docs", AuthType::None, Some("unexpected")),
        )
        .await
        .expect_err("匿名连接不能携带凭据");
    assert!(matches!(anonymous_with_secret, McpError::InvalidRequest(_)));

    let bearer_without_secret = rig
        .service
        .create_connection(
            &admin_identity(),
            new_connection("docs", AuthType::Bearer, None),
        )
        .await
        .expect_err("Bearer 连接必须提交凭据");
    assert!(matches!(bearer_without_secret, McpError::InvalidRequest(_)));

    let connection = rig
        .service
        .create_connection(
            &admin_identity(),
            new_connection("docs", AuthType::None, None),
        )
        .await
        .expect("创建匿名连接");
    let switch_without_credential = rig
        .service
        .update_connection(
            &admin_identity(),
            connection.id,
            ConnectionChanges {
                auth_type: Some(AuthType::Bearer),
                ..ConnectionChanges::default()
            },
        )
        .await
        .expect_err("没有凭据时不能切换到 Bearer");
    assert!(matches!(
        switch_without_credential,
        McpError::InvalidRequest(_)
    ));

    let configured = rig
        .service
        .set_credential(
            &admin_identity(),
            connection.id,
            AuthType::Bearer,
            Some(SecretString::new("rotated-token")),
        )
        .await
        .expect("设置凭据");
    assert_eq!(configured.auth_type, AuthType::Bearer);
    assert!(configured.credential_configured());

    let cleared = rig
        .service
        .update_connection(
            &admin_identity(),
            connection.id,
            ConnectionChanges {
                auth_type: Some(AuthType::None),
                ..ConnectionChanges::default()
            },
        )
        .await
        .expect("切回匿名会清除凭据");
    assert_eq!(cleared.auth_type, AuthType::None);
    assert!(!cleared.credential_configured());
}

#[tokio::test]
async fn oversized_endpoint_is_rejected_before_storage() {
    let rig = TestRig::new();
    let mut input = new_connection("docs", AuthType::None, None);
    input.endpoint_url = Url::parse(&format!("https://remote.example.com/{}", "a".repeat(2048)))
        .expect("测试地址有效");
    let error = rig
        .service
        .create_connection(&admin_identity(), input)
        .await
        .expect_err("超长 endpoint 必须被拒绝");
    assert!(matches!(error, McpError::InvalidRequest(_)));
    assert!(rig.repository.stored_connections().is_empty());

    let connection = rig
        .service
        .create_connection(
            &admin_identity(),
            new_connection("docs", AuthType::None, None),
        )
        .await
        .expect("创建合法连接");
    let error = rig
        .service
        .update_connection(
            &admin_identity(),
            connection.id,
            ConnectionChanges {
                endpoint_url: Some(
                    Url::parse(&format!("https://remote.example.com/{}", "b".repeat(2048)))
                        .expect("测试地址有效"),
                ),
                ..ConnectionChanges::default()
            },
        )
        .await
        .expect_err("超长 endpoint 更新必须被拒绝");
    assert!(matches!(error, McpError::InvalidRequest(_)));
}

#[tokio::test]
async fn tool_listing_is_namespaced_and_skips_failing_connections() {
    let rig = TestRig::new();
    let docs = rig.seed_connection("docs", ConnectionStatus::Active);
    let team = rig.seed_connection("team", ConnectionStatus::Active);
    let unassigned = rig.seed_connection("unassigned", ConnectionStatus::Active);
    rig.authorize(
        docs.id,
        AssignmentSubject::User(SubjectId::new("user-1").expect("测试主体有效")),
    );
    rig.authorize(
        team.id,
        AssignmentSubject::Group(GroupId::new("team-a").expect("测试分组有效")),
    );
    rig.remote.set_tools(docs.id, &["search", "fetch"]);
    rig.remote.set_tools(team.id, &["search", "only"]);
    rig.remote.set_tools(unassigned.id, &["hidden"]);

    let identity = user_identity(&["team-a"]);
    let tools = rig
        .service
        .list_authorized_tools(&identity)
        .await
        .expect("列出授权工具");
    let names: Vec<&str> = tools
        .iter()
        .map(|tool| tool.exposed_name.as_str())
        .collect();
    assert_eq!(names.len(), 4, "实际：{names:?}");
    assert!(names.contains(&exposed_tool_name(docs.id, "search").as_str()));
    assert!(names.contains(&exposed_tool_name(docs.id, "fetch").as_str()));
    assert!(names.contains(&exposed_tool_name(team.id, "search").as_str()));
    assert!(names.contains(&exposed_tool_name(team.id, "only").as_str()));
    assert_ne!(
        exposed_tool_name(docs.id, "search"),
        exposed_tool_name(team.id, "search"),
        "不同连接的同名远端工具必须通过连接前缀保持无歧义"
    );
    assert!(
        !names.contains(&exposed_tool_name(unassigned.id, "hidden").as_str()),
        "未授权连接不能出现"
    );

    rig.remote.set_failing(docs.id);
    let remaining = rig
        .service
        .list_authorized_tools(&identity)
        .await
        .expect("单条连接失败不影响其他连接");
    let remaining_names: Vec<String> = remaining
        .iter()
        .map(|tool| tool.exposed_name.clone())
        .collect();
    assert_eq!(
        remaining_names,
        vec![
            exposed_tool_name(team.id, "only"),
            exposed_tool_name(team.id, "search")
        ]
    );
}

#[tokio::test]
async fn consumption_revalidates_endpoint_and_fails_closed() {
    let rig = TestRig::new();
    let docs = rig.seed_connection("docs", ConnectionStatus::Active);
    rig.authorize(
        docs.id,
        AssignmentSubject::User(SubjectId::new("user-1").expect("测试主体有效")),
    );
    rig.remote.set_tools(docs.id, &["echo"]);
    let identity = user_identity(&[]);

    rig.repository.with_store(|store| {
        let connection = store
            .connections
            .iter_mut()
            .find(|connection| connection.id == docs.id)
            .expect("测试连接存在");
        connection.endpoint_url =
            Url::parse("http://internal.example.com/mcp").expect("测试地址有效");
    });

    assert!(
        rig.service
            .list_authorized_tools(&identity)
            .await
            .expect("消费阶段拒绝不安全 endpoint 时只跳过该连接")
            .is_empty()
    );
    let error = rig
        .service
        .call_authorized_tool(&identity, &exposed_tool_name(docs.id, "echo"), None)
        .await
        .expect_err("消费阶段必须重新校验 endpoint");
    assert!(
        matches!(error, McpError::UnsafeEndpoint(_)),
        "实际：{error:?}"
    );
    assert!(
        rig.remote.recorded_calls().is_empty(),
        "endpoint 被拒绝时不得发起远端调用"
    );
    assert!(
        rig.remote.recorded_credentials().is_empty(),
        "endpoint 被拒绝时不得把凭据交给远端客户端"
    );
}

#[tokio::test]
async fn tool_calls_recheck_authorization_and_map_remote_failures() {
    let rig = TestRig::new();
    let docs = rig.seed_connection("docs", ConnectionStatus::Active);
    let subject = AssignmentSubject::User(SubjectId::new("user-1").expect("测试主体有效"));
    rig.authorize(docs.id, subject.clone());
    rig.remote.set_tools(docs.id, &["echo"]);
    let identity = user_identity(&[]);
    let exposed = exposed_tool_name(docs.id, "echo");

    let outcome = rig
        .service
        .call_authorized_tool(&identity, &exposed, None)
        .await
        .expect("获准调用");
    assert!(!outcome.is_error);
    assert_eq!(
        rig.remote.recorded_calls(),
        vec![(docs.id, "echo".to_owned(), None)]
    );

    let scope = tenant_scope();
    rig.repository
        .remove_assignment(&scope, docs.id, &subject)
        .await
        .expect("移除授权");
    let forbidden = rig
        .service
        .call_authorized_tool(&identity, &exposed, None)
        .await
        .expect_err("调用阶段必须重新校验授权");
    assert_eq!(forbidden, McpError::Forbidden);

    rig.authorize(docs.id, subject);
    let missing_tool = rig
        .service
        .call_authorized_tool(&identity, &exposed_tool_name(docs.id, "missing"), None)
        .await
        .expect_err("远端不存在的工具");
    assert_eq!(missing_tool, McpError::NotFound);

    let unknown_name = rig
        .service
        .call_authorized_tool(&identity, "echo", None)
        .await
        .expect_err("未带连接前缀的名称不可调用");
    assert_eq!(unknown_name, McpError::NotFound);

    rig.remote.set_failing(docs.id);
    let unavailable = rig
        .service
        .call_authorized_tool(&identity, &exposed, None)
        .await
        .expect_err("远端不可用");
    assert_eq!(unavailable, McpError::UpstreamUnavailable);
}

#[tokio::test]
async fn disabled_connections_disappear_from_listing_and_calls() {
    let rig = TestRig::new();
    let docs = rig.seed_connection("docs", ConnectionStatus::Active);
    rig.authorize(
        docs.id,
        AssignmentSubject::User(SubjectId::new("user-1").expect("测试主体有效")),
    );
    rig.remote.set_tools(docs.id, &["echo"]);
    let identity = user_identity(&[]);

    assert_eq!(
        rig.service
            .list_authorized_tools(&identity)
            .await
            .expect("停用前可列出工具")
            .len(),
        1
    );

    rig.service
        .disable_connection(&admin_identity(), docs.id)
        .await
        .expect("管理员停用连接");
    assert!(
        rig.service
            .list_authorized_tools(&identity)
            .await
            .expect("停用后列表可用")
            .is_empty()
    );
    let forbidden = rig
        .service
        .call_authorized_tool(&identity, &exposed_tool_name(docs.id, "echo"), None)
        .await
        .expect_err("停用连接不可调用");
    assert_eq!(forbidden, McpError::Forbidden);
}

mod concurrency;

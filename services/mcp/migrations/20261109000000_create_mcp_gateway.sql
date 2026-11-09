-- MCP 服务独占的远端连接、加密凭据与用户/分组授权。
--
-- 本库不建跨服务外键，也不保存消费项目的账号、组织或会话数据；
-- consumer_id / tenant_id / subject_id 都是消费服务命名空间下的不透明字符串。
-- 凭据只保存认证加密后的密文，表中没有明文列。

CREATE TABLE mcp_connections (
    id UUID PRIMARY KEY,
    consumer_id TEXT NOT NULL,
    tenant_id TEXT NOT NULL,
    name TEXT NOT NULL,
    endpoint_url TEXT NOT NULL,
    auth_type TEXT NOT NULL,
    credential_ciphertext BYTEA,
    status TEXT NOT NULL,
    created_by TEXT NOT NULL,
    created_at TIMESTAMPTZ NOT NULL,
    updated_at TIMESTAMPTZ NOT NULL,
    CONSTRAINT mcp_connections_scope_name_unique UNIQUE (consumer_id, tenant_id, name),
    CONSTRAINT mcp_connections_auth_type_check CHECK (auth_type IN ('none', 'bearer')),
    CONSTRAINT mcp_connections_status_check CHECK (status IN ('active', 'disabled')),
    CONSTRAINT mcp_connections_credential_matches_auth CHECK (
        (auth_type = 'bearer' AND credential_ciphertext IS NOT NULL)
        OR (auth_type = 'none' AND credential_ciphertext IS NULL)
    ),
    CONSTRAINT mcp_connections_name_length CHECK (length(name) BETWEEN 1 AND 100),
    CONSTRAINT mcp_connections_endpoint_length CHECK (length(endpoint_url) BETWEEN 1 AND 2048)
);

COMMENT ON TABLE mcp_connections IS 'MCP 服务拥有的远端 Streamable HTTP 连接；按消费服务与租户隔离';
COMMENT ON COLUMN mcp_connections.id IS '连接 UUID，同时作为对外工具名中的稳定前缀';
COMMENT ON COLUMN mcp_connections.consumer_id IS '由服务 token 映射出的消费服务不透明 ID';
COMMENT ON COLUMN mcp_connections.tenant_id IS '签名用户上下文中声明的租户不透明 ID';
COMMENT ON COLUMN mcp_connections.name IS '租户内唯一的连接展示名称';
COMMENT ON COLUMN mcp_connections.endpoint_url IS '远端 Streamable HTTP endpoint；保存与调用前都执行出站地址策略';
COMMENT ON COLUMN mcp_connections.auth_type IS 'none 表示匿名；bearer 表示使用加密保存的凭据';
COMMENT ON COLUMN mcp_connections.credential_ciphertext IS 'AES-256-GCM 密文，附加数据绑定消费服务、租户与连接 ID；无明文列';
COMMENT ON COLUMN mcp_connections.status IS 'active 参与授权；disabled 立即从工具列表与调用中移除';
COMMENT ON COLUMN mcp_connections.created_by IS '创建该连接的调用方主体 ID';
COMMENT ON COLUMN mcp_connections.created_at IS '创建时间';
COMMENT ON COLUMN mcp_connections.updated_at IS '最后更新时间';

CREATE INDEX mcp_connections_scope_status_idx
    ON mcp_connections (consumer_id, tenant_id, status);

CREATE TABLE mcp_connection_assignments (
    id UUID PRIMARY KEY,
    connection_id UUID NOT NULL REFERENCES mcp_connections(id) ON DELETE CASCADE,
    subject_type TEXT NOT NULL,
    subject_id TEXT NOT NULL,
    created_at TIMESTAMPTZ NOT NULL,
    CONSTRAINT mcp_connection_assignments_unique
        UNIQUE (connection_id, subject_type, subject_id),
    CONSTRAINT mcp_connection_assignments_subject_type_check
        CHECK (subject_type IN ('user', 'group')),
    CONSTRAINT mcp_connection_assignments_subject_length
        CHECK (length(subject_id) BETWEEN 1 AND 128)
);

COMMENT ON TABLE mcp_connection_assignments IS '连接对用户或分组的显式授权；不沿组织层级继承';
COMMENT ON COLUMN mcp_connection_assignments.id IS '授权记录 UUID';
COMMENT ON COLUMN mcp_connection_assignments.connection_id IS '被授权的连接；删除连接时级联删除';
COMMENT ON COLUMN mcp_connection_assignments.subject_type IS 'user 或 group';
COMMENT ON COLUMN mcp_connection_assignments.subject_id IS '消费服务命名空间下的不透明用户或分组 ID';
COMMENT ON COLUMN mcp_connection_assignments.created_at IS '授权创建时间';

CREATE INDEX mcp_connection_assignments_subject_idx
    ON mcp_connection_assignments (subject_type, subject_id);

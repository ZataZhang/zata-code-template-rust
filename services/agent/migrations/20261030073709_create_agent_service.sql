-- Agent 服务独占的组织目录、Runtime/Agent 目录、Session/Run 事实、资源与 tracing outbox。
--
-- 本库不建跨服务外键，不保存 accounts 密码/会话，也不复制 tracing 事件表。
-- 所有表和列都带 COMMENT，约束显式命名，便于排障与迁移审阅。

CREATE TABLE organizations (
    id UUID PRIMARY KEY,
    slug TEXT NOT NULL,
    display_name TEXT NOT NULL,
    created_by_admin_id BIGINT NOT NULL,
    created_at TIMESTAMPTZ NOT NULL,
    CONSTRAINT organizations_slug_unique UNIQUE (slug),
    CONSTRAINT organizations_slug_format CHECK (slug ~ '^[a-z0-9][a-z0-9-]{1,62}$'),
    CONSTRAINT organizations_display_name_nonempty CHECK (length(display_name) BETWEEN 1 AND 200),
    CONSTRAINT organizations_created_by_positive CHECK (created_by_admin_id > 0)
);

COMMENT ON TABLE organizations IS 'Agent 服务拥有的组织（租户）目录；成员关系与授权不来自 accounts';
COMMENT ON COLUMN organizations.id IS '组织内部 UUID 标识，跨服务只通过 HTTP 传递';
COMMENT ON COLUMN organizations.slug IS '组织稳定短名，全局唯一，用于展示与管理员输入';
COMMENT ON COLUMN organizations.display_name IS '组织展示名称';
COMMENT ON COLUMN organizations.created_by_admin_id IS '创建组织的 accounts 管理员 ID';
COMMENT ON COLUMN organizations.created_at IS '组织创建时间';

CREATE TABLE organization_units (
    id UUID PRIMARY KEY,
    organization_id UUID NOT NULL REFERENCES organizations(id) ON DELETE CASCADE,
    name TEXT NOT NULL,
    created_at TIMESTAMPTZ NOT NULL,
    CONSTRAINT organization_units_name_unique UNIQUE (organization_id, name),
    CONSTRAINT organization_units_identity UNIQUE (id, organization_id),
    CONSTRAINT organization_units_name_nonempty CHECK (length(name) BETWEEN 1 AND 200)
);

COMMENT ON TABLE organization_units IS '组织内部门；授权不沿父子层级继承，因此本表不建 parent 关系';
COMMENT ON COLUMN organization_units.id IS '部门 UUID 标识';
COMMENT ON COLUMN organization_units.organization_id IS '所属组织';
COMMENT ON COLUMN organization_units.name IS '组织内唯一的部门名称';
COMMENT ON COLUMN organization_units.created_at IS '部门创建时间';

CREATE TABLE organization_memberships (
    organization_id UUID NOT NULL REFERENCES organizations(id) ON DELETE CASCADE,
    user_id BIGINT NOT NULL,
    unit_id UUID,
    display_name TEXT,
    created_at TIMESTAMPTZ NOT NULL,
    PRIMARY KEY (organization_id, user_id),
    CONSTRAINT organization_memberships_unit_fk FOREIGN KEY (unit_id, organization_id)
        REFERENCES organization_units(id, organization_id) ON DELETE SET NULL,
    CONSTRAINT organization_memberships_user_positive CHECK (user_id > 0),
    CONSTRAINT organization_memberships_display_name_length CHECK (
        display_name IS NULL OR length(display_name) BETWEEN 1 AND 200
    )
);

COMMENT ON TABLE organization_memberships IS '显式 public 用户成员关系；一个用户在一个组织内最多属于一个部门';
COMMENT ON COLUMN organization_memberships.organization_id IS '所属组织';
COMMENT ON COLUMN organization_memberships.user_id IS 'accounts 公开 HTTP 验证得到的用户 ID';
COMMENT ON COLUMN organization_memberships.unit_id IS '可选显式部门；部门授权只认这一条，不向上递归';
COMMENT ON COLUMN organization_memberships.display_name IS '组织内展示名称；不复制 accounts 邮箱或密码';
COMMENT ON COLUMN organization_memberships.created_at IS '成员关系建立时间';

CREATE INDEX organization_memberships_user_idx ON organization_memberships (user_id, organization_id);
CREATE INDEX organization_memberships_unit_idx ON organization_memberships (organization_id, unit_id);

CREATE TABLE organization_admin_scopes (
    organization_id UUID NOT NULL REFERENCES organizations(id) ON DELETE CASCADE,
    admin_user_id BIGINT NOT NULL,
    granted_by_admin_id BIGINT NOT NULL,
    created_at TIMESTAMPTZ NOT NULL,
    PRIMARY KEY (organization_id, admin_user_id),
    CONSTRAINT organization_admin_scopes_admin_positive CHECK (admin_user_id > 0),
    CONSTRAINT organization_admin_scopes_granted_by_positive CHECK (granted_by_admin_id > 0)
);

COMMENT ON TABLE organization_admin_scopes IS 'accounts 管理员对单一组织的显式管理 scope；平台管理员不自动获得';
COMMENT ON COLUMN organization_admin_scopes.organization_id IS '被授权管理的组织';
COMMENT ON COLUMN organization_admin_scopes.admin_user_id IS 'accounts 管理员 ID；与 public 用户 ID 不混用';
COMMENT ON COLUMN organization_admin_scopes.granted_by_admin_id IS '授予该 scope 的平台管理员 ID';
COMMENT ON COLUMN organization_admin_scopes.created_at IS '授权时间';

CREATE INDEX organization_admin_scopes_admin_idx ON organization_admin_scopes (admin_user_id);

CREATE TABLE runtime_connections (
    id UUID PRIMARY KEY,
    organization_id UUID REFERENCES organizations(id) ON DELETE CASCADE,
    name TEXT NOT NULL,
    display_name TEXT NOT NULL,
    origin TEXT NOT NULL,
    adapter_kind TEXT NOT NULL,
    endpoint_url TEXT,
    credential_alias TEXT,
    status TEXT NOT NULL,
    status_detail TEXT,
    config JSONB NOT NULL DEFAULT '{}'::jsonb,
    created_at TIMESTAMPTZ NOT NULL,
    updated_at TIMESTAMPTZ NOT NULL,
    CONSTRAINT runtime_connections_name_unique_external UNIQUE (organization_id, name),
    CONSTRAINT runtime_connections_origin_valid CHECK (origin IN ('builtin', 'external')),
    CONSTRAINT runtime_connections_adapter_valid CHECK (adapter_kind IN ('sandbox', 'a2a')),
    CONSTRAINT runtime_connections_status_valid CHECK (status IN ('ready', 'unavailable')),
    CONSTRAINT runtime_connections_origin_ownership CHECK (
        (origin = 'builtin' AND organization_id IS NULL)
        OR (origin = 'external' AND organization_id IS NOT NULL)
    ),
    CONSTRAINT runtime_connections_external_a2a CHECK (origin = 'builtin' OR adapter_kind = 'a2a'),
    CONSTRAINT runtime_connections_endpoint_by_adapter CHECK (
        (adapter_kind = 'sandbox' AND endpoint_url IS NULL)
        OR (adapter_kind = 'a2a' AND endpoint_url IS NOT NULL)
    ),
    CONSTRAINT runtime_connections_name_nonempty CHECK (length(name) BETWEEN 1 AND 128),
    CONSTRAINT runtime_connections_display_name_nonempty CHECK (length(display_name) BETWEEN 1 AND 200),
    CONSTRAINT runtime_connections_endpoint_length CHECK (
        endpoint_url IS NULL OR length(endpoint_url) BETWEEN 1 AND 2048
    ),
    CONSTRAINT runtime_connections_credential_alias_length CHECK (
        credential_alias IS NULL OR length(credential_alias) BETWEEN 1 AND 128
    ),
    CONSTRAINT runtime_connections_status_detail_length CHECK (
        status_detail IS NULL OR length(status_detail) BETWEEN 1 AND 500
    )
);

COMMENT ON TABLE runtime_connections IS 'Runtime 执行载体；origin 表示部署内置还是组织外部接入，adapter_kind 表示执行协议';
COMMENT ON COLUMN runtime_connections.id IS 'Runtime 连接 UUID 标识，Agent 创建时冻结绑定';
COMMENT ON COLUMN runtime_connections.organization_id IS '外部 Runtime 的所属组织；内置 Runtime 为 NULL，全局只读';
COMMENT ON COLUMN runtime_connections.name IS '组织内 Runtime 名称；内置 Runtime 由部署声明给出';
COMMENT ON COLUMN runtime_connections.display_name IS 'Runtime 展示名称';
COMMENT ON COLUMN runtime_connections.origin IS 'builtin=部署声明且只读；external=组织管理员接入';
COMMENT ON COLUMN runtime_connections.adapter_kind IS 'sandbox=内置受限执行器；a2a=外部 A2A 1.0 JSON-RPC';
COMMENT ON COLUMN runtime_connections.endpoint_url IS '外部 A2A Agent Card 基础 URL；Sandbox 为空';
COMMENT ON COLUMN runtime_connections.credential_alias IS '外部 Runtime 凭据别名；真实密钥只在执行环境注入';
COMMENT ON COLUMN runtime_connections.status IS '最近一次声明/发现结果：ready 可执行，unavailable 明确不可用';
COMMENT ON COLUMN runtime_connections.status_detail IS '低敏不可用原因，不保存凭据或远端响应正文';
COMMENT ON COLUMN runtime_connections.config IS '内置 Runtime 的工具白名单与步数上限等非秘密声明';
COMMENT ON COLUMN runtime_connections.created_at IS '连接登记时间';
COMMENT ON COLUMN runtime_connections.updated_at IS '连接状态或声明最近更新时间';

CREATE UNIQUE INDEX runtime_connections_builtin_name_unique
    ON runtime_connections (name) WHERE organization_id IS NULL;
CREATE INDEX runtime_connections_organization_idx ON runtime_connections (organization_id, created_at);

CREATE TABLE business_agents (
    id UUID PRIMARY KEY,
    organization_id UUID NOT NULL REFERENCES organizations(id) ON DELETE CASCADE,
    runtime_connection_id UUID NOT NULL REFERENCES runtime_connections(id),
    name TEXT NOT NULL,
    display_name TEXT NOT NULL,
    description TEXT NOT NULL,
    prompt TEXT NOT NULL,
    owner_admin_id BIGINT NOT NULL,
    enabled BOOLEAN NOT NULL,
    descriptor_checksum TEXT NOT NULL,
    created_at TIMESTAMPTZ NOT NULL,
    updated_at TIMESTAMPTZ NOT NULL,
    CONSTRAINT business_agents_name_unique UNIQUE (organization_id, name),
    CONSTRAINT business_agents_name_nonempty CHECK (length(name) BETWEEN 1 AND 128),
    CONSTRAINT business_agents_display_name_nonempty CHECK (length(display_name) BETWEEN 1 AND 200),
    CONSTRAINT business_agents_description_length CHECK (length(description) <= 4000),
    CONSTRAINT business_agents_prompt_length CHECK (length(prompt) BETWEEN 1 AND 20000),
    CONSTRAINT business_agents_owner_positive CHECK (owner_admin_id > 0),
    CONSTRAINT business_agents_checksum_format CHECK (descriptor_checksum ~ '^[0-9a-f]{64}$')
);

COMMENT ON TABLE business_agents IS 'Business Agent 业务定义；没有 AgentVersion 表，Run 冻结自己的执行快照';
COMMENT ON COLUMN business_agents.id IS 'Agent UUID 标识';
COMMENT ON COLUMN business_agents.organization_id IS 'Agent 所属组织';
COMMENT ON COLUMN business_agents.runtime_connection_id IS '创建时绑定的 Runtime；后续更新不得改绑';
COMMENT ON COLUMN business_agents.name IS '组织内稳定名称，用于 API 与委派引用';
COMMENT ON COLUMN business_agents.display_name IS 'Agent 展示名称';
COMMENT ON COLUMN business_agents.description IS 'Agent 能力描述，供目录展示与 Auto 选择参考';
COMMENT ON COLUMN business_agents.prompt IS 'Agent 基础提示词；Run 创建时冻结生效文本';
COMMENT ON COLUMN business_agents.owner_admin_id IS '负责该 Agent 的管理员 ID';
COMMENT ON COLUMN business_agents.enabled IS '是否允许创建新 Run；不删除历史 Run';
COMMENT ON COLUMN business_agents.descriptor_checksum IS '业务 descriptor 与 Runtime binding 的 SHA-256，更新时重算';
COMMENT ON COLUMN business_agents.created_at IS 'Agent 创建时间';
COMMENT ON COLUMN business_agents.updated_at IS 'Agent 最近更新时间';

CREATE INDEX business_agents_organization_idx ON business_agents (organization_id, created_at);
CREATE INDEX business_agents_runtime_idx ON business_agents (runtime_connection_id);

CREATE TABLE agent_access_rules (
    id UUID PRIMARY KEY,
    agent_id UUID NOT NULL REFERENCES business_agents(id) ON DELETE CASCADE,
    effect TEXT NOT NULL,
    subject_kind TEXT NOT NULL,
    subject_id TEXT NOT NULL,
    created_at TIMESTAMPTZ NOT NULL,
    CONSTRAINT agent_access_rules_effect_valid CHECK (effect IN ('allow', 'deny')),
    CONSTRAINT agent_access_rules_subject_kind_valid CHECK (subject_kind IN ('organization', 'unit', 'user')),
    CONSTRAINT agent_access_rules_deny_only_user CHECK (effect = 'allow' OR subject_kind = 'user'),
    CONSTRAINT agent_access_rules_subject_nonempty CHECK (length(subject_id) BETWEEN 1 AND 128),
    CONSTRAINT agent_access_rules_unique UNIQUE (agent_id, effect, subject_kind, subject_id)
);

COMMENT ON TABLE agent_access_rules IS 'Agent allow/deny 规则；organization/unit 提供 allow，user deny 再收窄，deny 优先';
COMMENT ON COLUMN agent_access_rules.id IS '规则 UUID 标识';
COMMENT ON COLUMN agent_access_rules.agent_id IS '被授权 Agent';
COMMENT ON COLUMN agent_access_rules.effect IS 'allow 或 deny；deny 只允许针对个人';
COMMENT ON COLUMN agent_access_rules.subject_kind IS 'organization=组织全体成员；unit=显式部门；user=单个用户';
COMMENT ON COLUMN agent_access_rules.subject_id IS '组织 UUID、部门 UUID 或用户 ID 的文本形式';
COMMENT ON COLUMN agent_access_rules.created_at IS '规则创建时间';

CREATE INDEX agent_access_rules_agent_idx ON agent_access_rules (agent_id);

CREATE TABLE agent_entry_policies (
    agent_id UUID PRIMARY KEY REFERENCES business_agents(id) ON DELETE CASCADE,
    organization_id UUID NOT NULL REFERENCES organizations(id) ON DELETE CASCADE,
    auto_visible BOOLEAN NOT NULL DEFAULT FALSE,
    priority INTEGER NOT NULL DEFAULT 100,
    updated_at TIMESTAMPTZ NOT NULL,
    CONSTRAINT agent_entry_policies_priority_range CHECK (priority BETWEEN 0 AND 10000)
);

COMMENT ON TABLE agent_entry_policies IS '组织 Auto 入口候选；只有显式授权且 runnable 的 Agent 可被选择';
COMMENT ON COLUMN agent_entry_policies.agent_id IS '参与 Auto 候选的 Agent';
COMMENT ON COLUMN agent_entry_policies.organization_id IS '候选所属组织';
COMMENT ON COLUMN agent_entry_policies.auto_visible IS '是否允许未指定 Agent 的 Run 自动选择';
COMMENT ON COLUMN agent_entry_policies.priority IS '候选优先级，数字越小越优先；仅影响新 Run';
COMMENT ON COLUMN agent_entry_policies.updated_at IS '策略最近更新时间';

CREATE INDEX agent_entry_policies_org_idx
    ON agent_entry_policies (organization_id, auto_visible, priority);

CREATE TABLE agent_delegation_policies (
    parent_agent_id UUID PRIMARY KEY REFERENCES business_agents(id) ON DELETE CASCADE,
    child_agent_ids JSONB NOT NULL DEFAULT '[]'::jsonb,
    max_depth INTEGER NOT NULL DEFAULT 1,
    max_child_runs INTEGER NOT NULL DEFAULT 2,
    updated_at TIMESTAMPTZ NOT NULL,
    CONSTRAINT agent_delegation_policies_child_ids_array CHECK (jsonb_typeof(child_agent_ids) = 'array'),
    CONSTRAINT agent_delegation_policies_depth_range CHECK (max_depth BETWEEN 0 AND 3),
    CONSTRAINT agent_delegation_policies_child_runs_range CHECK (max_child_runs BETWEEN 0 AND 8)
);

COMMENT ON TABLE agent_delegation_policies IS '受治理的 Sandbox 子 Agent 委派策略；子 Run 沿用同一 Canonical Run 内核';
COMMENT ON COLUMN agent_delegation_policies.parent_agent_id IS '发起委派的父 Agent';
COMMENT ON COLUMN agent_delegation_policies.child_agent_ids IS '允许委派到的同组织 Agent UUID 字符串数组';
COMMENT ON COLUMN agent_delegation_policies.max_depth IS '最大委派深度；0 表示禁止委派';
COMMENT ON COLUMN agent_delegation_policies.max_child_runs IS '单次父 Run 最多创建的子 Run 数';
COMMENT ON COLUMN agent_delegation_policies.updated_at IS '策略最近更新时间';

CREATE TABLE agent_skills (
    id UUID PRIMARY KEY,
    organization_id UUID NOT NULL REFERENCES organizations(id) ON DELETE CASCADE,
    name TEXT NOT NULL,
    description TEXT NOT NULL,
    instructions TEXT NOT NULL,
    enabled BOOLEAN NOT NULL,
    created_at TIMESTAMPTZ NOT NULL,
    updated_at TIMESTAMPTZ NOT NULL,
    CONSTRAINT agent_skills_name_unique UNIQUE (organization_id, name),
    CONSTRAINT agent_skills_name_nonempty CHECK (length(name) BETWEEN 1 AND 128),
    CONSTRAINT agent_skills_description_length CHECK (length(description) <= 2000),
    CONSTRAINT agent_skills_instructions_length CHECK (length(instructions) BETWEEN 1 AND 20000)
);

COMMENT ON TABLE agent_skills IS '组织级通用 Skill；只保存运行机制与说明，不迁移来源业务 Skill 内容';
COMMENT ON COLUMN agent_skills.id IS 'Skill UUID 标识';
COMMENT ON COLUMN agent_skills.organization_id IS 'Skill 所属组织';
COMMENT ON COLUMN agent_skills.name IS '组织内唯一 Skill 名称';
COMMENT ON COLUMN agent_skills.description IS 'Skill 展示说明';
COMMENT ON COLUMN agent_skills.instructions IS '装配进 Run 快照的 Skill 指令';
COMMENT ON COLUMN agent_skills.enabled IS '是否允许后续 Run 选择';
COMMENT ON COLUMN agent_skills.created_at IS 'Skill 创建时间';
COMMENT ON COLUMN agent_skills.updated_at IS 'Skill 最近更新时间';

CREATE TABLE agent_skill_assets (
    id UUID PRIMARY KEY,
    skill_id UUID NOT NULL REFERENCES agent_skills(id) ON DELETE CASCADE,
    name TEXT NOT NULL,
    media_type TEXT NOT NULL,
    size_bytes BIGINT NOT NULL,
    sha256 TEXT NOT NULL,
    storage_key TEXT NOT NULL,
    created_at TIMESTAMPTZ NOT NULL,
    CONSTRAINT agent_skill_assets_name_unique UNIQUE (skill_id, name),
    CONSTRAINT agent_skill_assets_name_nonempty CHECK (length(name) BETWEEN 1 AND 255),
    CONSTRAINT agent_skill_assets_media_type_length CHECK (length(media_type) BETWEEN 1 AND 128),
    CONSTRAINT agent_skill_assets_size_nonnegative CHECK (size_bytes >= 0),
    CONSTRAINT agent_skill_assets_checksum_format CHECK (sha256 ~ '^[0-9a-f]{64}$'),
    CONSTRAINT agent_skill_assets_storage_key_unique UNIQUE (storage_key)
);

COMMENT ON TABLE agent_skill_assets IS 'Skill 资产元数据；正文存放在受控 Blob 存储，路径不使用用户文件名';
COMMENT ON COLUMN agent_skill_assets.id IS 'Skill 资产 UUID 标识';
COMMENT ON COLUMN agent_skill_assets.skill_id IS '所属 Skill';
COMMENT ON COLUMN agent_skill_assets.name IS 'Skill 内资产名称';
COMMENT ON COLUMN agent_skill_assets.media_type IS '资产 MIME 类型';
COMMENT ON COLUMN agent_skill_assets.size_bytes IS '资产字节数';
COMMENT ON COLUMN agent_skill_assets.sha256 IS '资产内容 SHA-256，读取时校验';
COMMENT ON COLUMN agent_skill_assets.storage_key IS 'Blob 存储键，不与用户文件名拼接';
COMMENT ON COLUMN agent_skill_assets.created_at IS '资产上传时间';

CREATE TABLE agent_skill_assignments (
    agent_id UUID NOT NULL REFERENCES business_agents(id) ON DELETE CASCADE,
    skill_id UUID NOT NULL REFERENCES agent_skills(id) ON DELETE CASCADE,
    created_at TIMESTAMPTZ NOT NULL,
    PRIMARY KEY (agent_id, skill_id)
);

COMMENT ON TABLE agent_skill_assignments IS 'Agent 可选择的 Skill 授权；Run 只快照实际选择的 Skill';
COMMENT ON COLUMN agent_skill_assignments.agent_id IS '被授权的 Agent';
COMMENT ON COLUMN agent_skill_assignments.skill_id IS '被授权的 Skill';
COMMENT ON COLUMN agent_skill_assignments.created_at IS '授权时间';

CREATE INDEX agent_skill_assignments_skill_idx ON agent_skill_assignments (skill_id);

CREATE TABLE chat_sessions (
    id UUID PRIMARY KEY,
    organization_id UUID NOT NULL REFERENCES organizations(id) ON DELETE CASCADE,
    owner_user_id BIGINT NOT NULL,
    agent_id UUID REFERENCES business_agents(id) ON DELETE SET NULL,
    title TEXT NOT NULL,
    created_at TIMESTAMPTZ NOT NULL,
    last_activity_at TIMESTAMPTZ NOT NULL,
    CONSTRAINT chat_sessions_owner_positive CHECK (owner_user_id > 0),
    CONSTRAINT chat_sessions_title_length CHECK (length(title) BETWEEN 1 AND 200)
);

COMMENT ON TABLE chat_sessions IS '用户 Session（产品会话）一对一映射 tracing thread；所有者隔离';
COMMENT ON COLUMN chat_sessions.id IS 'Session UUID 标识，同时作为 tracing thread_id';
COMMENT ON COLUMN chat_sessions.organization_id IS 'Session 所属组织';
COMMENT ON COLUMN chat_sessions.owner_user_id IS 'accounts 验证的 Session 所有者；只有所有者可读写';
COMMENT ON COLUMN chat_sessions.agent_id IS '可选显式 Agent；为空表示每条 Run 按入口策略 Auto 选择';
COMMENT ON COLUMN chat_sessions.title IS 'Session 展示标题';
COMMENT ON COLUMN chat_sessions.created_at IS 'Session 创建时间';
COMMENT ON COLUMN chat_sessions.last_activity_at IS 'Session 最近消息或 Run 活动时间';

CREATE INDEX chat_sessions_owner_idx ON chat_sessions (owner_user_id, organization_id, last_activity_at DESC);

CREATE TABLE chat_messages (
    id UUID PRIMARY KEY,
    session_id UUID NOT NULL REFERENCES chat_sessions(id) ON DELETE CASCADE,
    run_id UUID,
    role TEXT NOT NULL,
    content TEXT NOT NULL,
    resource_ids JSONB NOT NULL DEFAULT '[]'::jsonb,
    created_at TIMESTAMPTZ NOT NULL,
    CONSTRAINT chat_messages_role_valid CHECK (role IN ('user', 'assistant')),
    CONSTRAINT chat_messages_content_length CHECK (length(content) BETWEEN 1 AND 100000),
    CONSTRAINT chat_messages_resource_ids_array CHECK (jsonb_typeof(resource_ids) = 'array')
);

COMMENT ON TABLE chat_messages IS 'Session 对话消息；每条 user 消息对应一个 Canonical Run';
COMMENT ON COLUMN chat_messages.id IS '消息 UUID 标识';
COMMENT ON COLUMN chat_messages.session_id IS '消息所属 Session';
COMMENT ON COLUMN chat_messages.run_id IS '消息对应的 Run；Run 写入后回填';
COMMENT ON COLUMN chat_messages.role IS 'user 或 assistant';
COMMENT ON COLUMN chat_messages.content IS '消息正文，只在本服务数据库保存';
COMMENT ON COLUMN chat_messages.resource_ids IS '消息引用的用户资源 UUID 数组';
COMMENT ON COLUMN chat_messages.created_at IS '消息创建时间';

CREATE INDEX chat_messages_session_idx ON chat_messages (session_id, created_at);

CREATE TABLE agent_runs (
    id UUID PRIMARY KEY,
    organization_id UUID NOT NULL REFERENCES organizations(id) ON DELETE CASCADE,
    session_id UUID NOT NULL REFERENCES chat_sessions(id) ON DELETE CASCADE,
    owner_user_id BIGINT NOT NULL,
    agent_id UUID NOT NULL REFERENCES business_agents(id),
    runtime_connection_id UUID NOT NULL REFERENCES runtime_connections(id),
    parent_run_id UUID REFERENCES agent_runs(id),
    delegation_id UUID,
    delegation_depth SMALLINT NOT NULL DEFAULT 0,
    origin TEXT NOT NULL,
    status TEXT NOT NULL,
    idempotency_key TEXT NOT NULL,
    request_digest TEXT NOT NULL,
    input_message_id UUID NOT NULL REFERENCES chat_messages(id),
    snapshot JSONB NOT NULL,
    agent_snapshot_checksum TEXT NOT NULL,
    context_pack_checksum TEXT NOT NULL,
    access_policy_checksum TEXT NOT NULL,
    routing_checksum TEXT NOT NULL,
    last_event_seq BIGINT NOT NULL DEFAULT 0,
    trace_delivered_through_seq BIGINT NOT NULL DEFAULT 0,
    error_code TEXT,
    cancel_requested_at TIMESTAMPTZ,
    started_at TIMESTAMPTZ,
    finished_at TIMESTAMPTZ,
    created_at TIMESTAMPTZ NOT NULL,
    CONSTRAINT agent_runs_owner_positive CHECK (owner_user_id > 0),
    CONSTRAINT agent_runs_origin_valid CHECK (origin IN ('direct', 'auto', 'delegation')),
    CONSTRAINT agent_runs_status_valid CHECK (
        status IN ('queued', 'running', 'cancelling', 'succeeded', 'failed', 'cancelled')
    ),
    CONSTRAINT agent_runs_idempotency_key_length CHECK (length(idempotency_key) BETWEEN 1 AND 200),
    CONSTRAINT agent_runs_request_digest_format CHECK (request_digest ~ '^[0-9a-f]{64}$'),
    CONSTRAINT agent_runs_agent_checksum_format CHECK (agent_snapshot_checksum ~ '^[0-9a-f]{64}$'),
    CONSTRAINT agent_runs_context_checksum_format CHECK (context_pack_checksum ~ '^[0-9a-f]{64}$'),
    CONSTRAINT agent_runs_access_checksum_format CHECK (access_policy_checksum ~ '^[0-9a-f]{64}$'),
    CONSTRAINT agent_runs_routing_checksum_format CHECK (routing_checksum ~ '^[0-9a-f]{64}$'),
    CONSTRAINT agent_runs_delegation_depth_range CHECK (delegation_depth BETWEEN 0 AND 3),
    CONSTRAINT agent_runs_event_seq_nonnegative CHECK (
        last_event_seq >= 0 AND trace_delivered_through_seq >= 0
    ),
    CONSTRAINT agent_runs_error_code_length CHECK (error_code IS NULL OR length(error_code) BETWEEN 1 AND 128),
    CONSTRAINT agent_runs_snapshot_object CHECK (jsonb_typeof(snapshot) = 'object')
);

COMMENT ON TABLE agent_runs IS 'Canonical Run：Run 状态与执行事件的唯一事实源；每个 user 消息创建一个 Run';
COMMENT ON COLUMN agent_runs.id IS 'Run UUID 标识，同时也是 tracing producer 的 run_id';
COMMENT ON COLUMN agent_runs.organization_id IS 'Run 所属组织';
COMMENT ON COLUMN agent_runs.session_id IS 'Run 所属 Session，也作为 tracing thread_id';
COMMENT ON COLUMN agent_runs.owner_user_id IS 'Run 所有者；会话与 Run 的 owner ACL 依据';
COMMENT ON COLUMN agent_runs.agent_id IS '本次 Run 使用的 Agent；父子 Run 可不同';
COMMENT ON COLUMN agent_runs.runtime_connection_id IS '创建 Run 时冻结的 Runtime binding';
COMMENT ON COLUMN agent_runs.parent_run_id IS '受治理委派的父 Run；顶层 Run 为空';
COMMENT ON COLUMN agent_runs.delegation_id IS '委派标识，用于关联父 Run 的 delegation 事件';
COMMENT ON COLUMN agent_runs.delegation_depth IS '从顶层 Run 起的委派深度';
COMMENT ON COLUMN agent_runs.origin IS 'direct=显式 Agent；auto=入口策略选择；delegation=父 Agent 委派';
COMMENT ON COLUMN agent_runs.status IS 'Run 状态机：queued/running/cancelling 与三个终态';
COMMENT ON COLUMN agent_runs.idempotency_key IS '调用方幂等键；与 owner 组成唯一查重范围';
COMMENT ON COLUMN agent_runs.request_digest IS '规范化请求的 SHA-256；与幂等键共同判定重放或冲突';
COMMENT ON COLUMN agent_runs.input_message_id IS '触发本 Run 的 user 消息';
COMMENT ON COLUMN agent_runs.snapshot IS 'Run 自有的 Agent/Runtime/提示词/资源/Skill/路由/委派快照';
COMMENT ON COLUMN agent_runs.agent_snapshot_checksum IS 'Agent descriptor 与 Runtime binding 快照的 SHA-256';
COMMENT ON COLUMN agent_runs.context_pack_checksum IS '输入、资源与 Skill 快照的 SHA-256';
COMMENT ON COLUMN agent_runs.access_policy_checksum IS '创建 Run 时授权依据与 ACL 快照的 SHA-256';
COMMENT ON COLUMN agent_runs.routing_checksum IS '路由候选与选择结果的 SHA-256';
COMMENT ON COLUMN agent_runs.last_event_seq IS '本 Run 已写入的最大事件序号，用于事务内追加';
COMMENT ON COLUMN agent_runs.trace_delivered_through_seq IS '已由 tracing producer API 确认的连续事件序号检查点';
COMMENT ON COLUMN agent_runs.error_code IS '失败或取消后的低敏错误类别';
COMMENT ON COLUMN agent_runs.cancel_requested_at IS '首次协作式取消请求时间；重复取消不改变';
COMMENT ON COLUMN agent_runs.started_at IS 'run.started 时间';
COMMENT ON COLUMN agent_runs.finished_at IS '进入终态的时间';
COMMENT ON COLUMN agent_runs.created_at IS 'Run 创建时间';

ALTER TABLE chat_messages
    ADD CONSTRAINT chat_messages_run_fk FOREIGN KEY (run_id) REFERENCES agent_runs(id) ON DELETE SET NULL;

CREATE INDEX agent_runs_session_idx ON agent_runs (session_id, created_at);
CREATE INDEX agent_runs_owner_idx ON agent_runs (owner_user_id, created_at DESC);
CREATE INDEX agent_runs_status_idx ON agent_runs (status, created_at);
CREATE INDEX agent_runs_parent_idx ON agent_runs (parent_run_id);

CREATE TABLE agent_run_events (
    run_id UUID NOT NULL REFERENCES agent_runs(id) ON DELETE CASCADE,
    seq BIGINT NOT NULL,
    event_type TEXT NOT NULL,
    occurred_at TIMESTAMPTZ NOT NULL,
    payload JSONB NOT NULL,
    PRIMARY KEY (run_id, seq),
    CONSTRAINT agent_run_events_seq_positive CHECK (seq > 0),
    CONSTRAINT agent_run_events_type_length CHECK (length(event_type) BETWEEN 1 AND 128),
    CONSTRAINT agent_run_events_payload_object CHECK (jsonb_typeof(payload) = 'object')
);

COMMENT ON TABLE agent_run_events IS 'Run 内从 1 开始的持久单调事件；SSE 重放与 tracing 投递的共同事实源';
COMMENT ON COLUMN agent_run_events.run_id IS '事件所属 Run';
COMMENT ON COLUMN agent_run_events.seq IS 'Run 内连续序号，从 1 开始';
COMMENT ON COLUMN agent_run_events.event_type IS '与 tracing 冻结目录一致的脱敏事件类型';
COMMENT ON COLUMN agent_run_events.occurred_at IS '事件发生时间';
COMMENT ON COLUMN agent_run_events.payload IS '已脱敏事件载荷；不含完整提示词、工具参数或凭据';

CREATE INDEX agent_run_events_type_idx ON agent_run_events (event_type, occurred_at DESC);

CREATE TABLE agent_trace_outbox (
    id BIGINT GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    run_id UUID NOT NULL REFERENCES agent_runs(id) ON DELETE CASCADE,
    event_seq BIGINT NOT NULL,
    event_type TEXT NOT NULL,
    occurred_at TIMESTAMPTZ NOT NULL,
    payload JSONB NOT NULL,
    status TEXT NOT NULL DEFAULT 'pending',
    attempts INTEGER NOT NULL DEFAULT 0,
    next_attempt_at TIMESTAMPTZ NOT NULL,
    delivered_at TIMESTAMPTZ,
    last_error_code TEXT,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    CONSTRAINT agent_trace_outbox_event_unique UNIQUE (run_id, event_seq),
    CONSTRAINT agent_trace_outbox_status_valid CHECK (status IN ('pending', 'delivered')),
    CONSTRAINT agent_trace_outbox_attempts_nonnegative CHECK (attempts >= 0),
    CONSTRAINT agent_trace_outbox_event_seq_positive CHECK (event_seq > 0),
    CONSTRAINT agent_trace_outbox_payload_object CHECK (jsonb_typeof(payload) = 'object'),
    CONSTRAINT agent_trace_outbox_error_code_length CHECK (
        last_error_code IS NULL OR length(last_error_code) BETWEEN 1 AND 128
    ),
    CONSTRAINT agent_trace_outbox_delivered_consistency CHECK (
        (status = 'delivered' AND delivered_at IS NOT NULL)
        OR (status = 'pending' AND delivered_at IS NULL)
    )
);

COMMENT ON TABLE agent_trace_outbox IS '本地事务生成的 tracing 待投递事件；幂等、有界重试，不阻塞 Run';
COMMENT ON COLUMN agent_trace_outbox.id IS 'outbox 自增标识，提供稳定的插入顺序';
COMMENT ON COLUMN agent_trace_outbox.run_id IS '事件所属 Run';
COMMENT ON COLUMN agent_trace_outbox.event_seq IS '对应 Run 事件序号，唯一防止重复投递行';
COMMENT ON COLUMN agent_trace_outbox.event_type IS 'tracing 事件类型';
COMMENT ON COLUMN agent_trace_outbox.occurred_at IS '事件发生时间';
COMMENT ON COLUMN agent_trace_outbox.payload IS '脱敏 tracing 载荷';
COMMENT ON COLUMN agent_trace_outbox.status IS 'pending 待投递；delivered 已被 producer API 确认';
COMMENT ON COLUMN agent_trace_outbox.attempts IS '已尝试投递次数';
COMMENT ON COLUMN agent_trace_outbox.next_attempt_at IS '下一次允许尝试时间，用于有界退避';
COMMENT ON COLUMN agent_trace_outbox.delivered_at IS 'tracing 确认时间';
COMMENT ON COLUMN agent_trace_outbox.last_error_code IS '最近一次投递失败的低敏错误类别';
COMMENT ON COLUMN agent_trace_outbox.created_at IS 'outbox 行创建时间';

CREATE INDEX agent_trace_outbox_pending_idx
    ON agent_trace_outbox (status, next_attempt_at, id);
CREATE INDEX agent_trace_outbox_run_idx ON agent_trace_outbox (run_id, event_seq);

CREATE TABLE agent_run_idempotency (
    owner_user_id BIGINT NOT NULL,
    idempotency_key TEXT NOT NULL,
    request_digest TEXT NOT NULL,
    run_id UUID NOT NULL REFERENCES agent_runs(id) ON DELETE CASCADE,
    created_at TIMESTAMPTZ NOT NULL,
    PRIMARY KEY (owner_user_id, idempotency_key),
    CONSTRAINT agent_run_idempotency_owner_positive CHECK (owner_user_id > 0),
    CONSTRAINT agent_run_idempotency_key_length CHECK (length(idempotency_key) BETWEEN 1 AND 128),
    CONSTRAINT agent_run_idempotency_digest_format CHECK (request_digest ~ '^[0-9a-f]{64}$')
);

COMMENT ON TABLE agent_run_idempotency IS 'user + Idempotency-Key 唯一映射；相同规范化请求重放原 Run';
COMMENT ON COLUMN agent_run_idempotency.owner_user_id IS '幂等键所属最终用户';
COMMENT ON COLUMN agent_run_idempotency.idempotency_key IS '调用方提供的 Idempotency-Key';
COMMENT ON COLUMN agent_run_idempotency.request_digest IS '规范化请求摘要；不一致时返回冲突';
COMMENT ON COLUMN agent_run_idempotency.run_id IS '首次创建的 Run';
COMMENT ON COLUMN agent_run_idempotency.created_at IS '幂等记录创建时间';

CREATE TABLE user_resources (
    id UUID PRIMARY KEY,
    organization_id UUID NOT NULL REFERENCES organizations(id) ON DELETE CASCADE,
    owner_user_id BIGINT NOT NULL,
    filename TEXT NOT NULL,
    media_type TEXT NOT NULL,
    size_bytes BIGINT NOT NULL,
    sha256 TEXT NOT NULL,
    storage_key TEXT NOT NULL,
    created_at TIMESTAMPTZ NOT NULL,
    deleted_at TIMESTAMPTZ,
    CONSTRAINT user_resources_owner_positive CHECK (owner_user_id > 0),
    CONSTRAINT user_resources_filename_length CHECK (length(filename) BETWEEN 1 AND 255),
    CONSTRAINT user_resources_media_type_length CHECK (length(media_type) BETWEEN 1 AND 128),
    CONSTRAINT user_resources_size_nonnegative CHECK (size_bytes >= 0),
    CONSTRAINT user_resources_checksum_format CHECK (sha256 ~ '^[0-9a-f]{64}$'),
    CONSTRAINT user_resources_storage_key_unique UNIQUE (storage_key)
);

COMMENT ON TABLE user_resources IS '用户上传资源元数据；正文在受控 Blob，所有读取校验 owner';
COMMENT ON COLUMN user_resources.id IS '资源 UUID 标识，API 不使用文件路径';
COMMENT ON COLUMN user_resources.organization_id IS '资源所属组织';
COMMENT ON COLUMN user_resources.owner_user_id IS '资源所有者；跨 owner 读取被拒绝';
COMMENT ON COLUMN user_resources.filename IS '用户可见文件名，仅用于展示与下载头';
COMMENT ON COLUMN user_resources.media_type IS '资源 MIME 类型';
COMMENT ON COLUMN user_resources.size_bytes IS '资源字节数';
COMMENT ON COLUMN user_resources.sha256 IS '资源内容 SHA-256';
COMMENT ON COLUMN user_resources.storage_key IS 'Blob 存储键';
COMMENT ON COLUMN user_resources.created_at IS '上传时间';
COMMENT ON COLUMN user_resources.deleted_at IS '软删除时间；删除后新 Run 不可再引用';

CREATE INDEX user_resources_owner_idx
    ON user_resources (owner_user_id, organization_id, created_at DESC)
    WHERE deleted_at IS NULL;

CREATE TABLE run_artifacts (
    id UUID PRIMARY KEY,
    run_id UUID NOT NULL REFERENCES agent_runs(id) ON DELETE CASCADE,
    owner_user_id BIGINT NOT NULL,
    filename TEXT NOT NULL,
    media_type TEXT NOT NULL,
    size_bytes BIGINT NOT NULL,
    sha256 TEXT NOT NULL,
    storage_key TEXT NOT NULL,
    created_at TIMESTAMPTZ NOT NULL,
    CONSTRAINT run_artifacts_owner_positive CHECK (owner_user_id > 0),
    CONSTRAINT run_artifacts_filename_length CHECK (length(filename) BETWEEN 1 AND 255),
    CONSTRAINT run_artifacts_media_type_length CHECK (length(media_type) BETWEEN 1 AND 128),
    CONSTRAINT run_artifacts_size_nonnegative CHECK (size_bytes >= 0),
    CONSTRAINT run_artifacts_checksum_format CHECK (sha256 ~ '^[0-9a-f]{64}$'),
    CONSTRAINT run_artifacts_storage_key_unique UNIQUE (storage_key)
);

COMMENT ON TABLE run_artifacts IS 'Run 产生的通用产物元数据；正文在受控 Blob';
COMMENT ON COLUMN run_artifacts.id IS '产物 UUID 标识';
COMMENT ON COLUMN run_artifacts.run_id IS '产生该产物的 Run';
COMMENT ON COLUMN run_artifacts.owner_user_id IS 'Run 所有者，也是产物下载 owner ACL 依据';
COMMENT ON COLUMN run_artifacts.filename IS '产物文件名';
COMMENT ON COLUMN run_artifacts.media_type IS '产物 MIME 类型';
COMMENT ON COLUMN run_artifacts.size_bytes IS '产物字节数';
COMMENT ON COLUMN run_artifacts.sha256 IS '产物内容 SHA-256';
COMMENT ON COLUMN run_artifacts.storage_key IS 'Blob 存储键';
COMMENT ON COLUMN run_artifacts.created_at IS '产物创建时间';

CREATE INDEX run_artifacts_run_idx ON run_artifacts (run_id, created_at);

CREATE TABLE agent_audit_records (
    id BIGINT GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    occurred_at TIMESTAMPTZ NOT NULL,
    service_id TEXT NOT NULL,
    admin_user_id BIGINT,
    user_id BIGINT,
    organization_id UUID,
    action TEXT NOT NULL,
    target_id TEXT,
    outcome TEXT NOT NULL,
    detail JSONB NOT NULL DEFAULT '{}'::jsonb,
    CONSTRAINT agent_audit_records_service_nonempty CHECK (length(service_id) BETWEEN 1 AND 128),
    CONSTRAINT agent_audit_records_action_nonempty CHECK (length(action) BETWEEN 1 AND 128),
    CONSTRAINT agent_audit_records_outcome_valid CHECK (outcome IN ('allowed', 'denied', 'succeeded', 'failed')),
    CONSTRAINT agent_audit_records_target_length CHECK (target_id IS NULL OR length(target_id) <= 256),
    CONSTRAINT agent_audit_records_detail_object CHECK (jsonb_typeof(detail) = 'object')
);

COMMENT ON TABLE agent_audit_records IS '服务身份与最终用户/管理员的关键操作审计；不保存凭据或正文';
COMMENT ON COLUMN agent_audit_records.id IS '审计自增标识';
COMMENT ON COLUMN agent_audit_records.occurred_at IS '操作发生时间';
COMMENT ON COLUMN agent_audit_records.service_id IS '经 Bearer 凭据验证的调用服务标识';
COMMENT ON COLUMN agent_audit_records.admin_user_id IS 'accounts 管理员身份；public 操作为空';
COMMENT ON COLUMN agent_audit_records.user_id IS 'accounts 最终用户身份；管理员操作可为空';
COMMENT ON COLUMN agent_audit_records.organization_id IS '操作涉及的组织；组织删除后保留审计值';
COMMENT ON COLUMN agent_audit_records.action IS '稳定动作名，如 agent.create、run.cancel';
COMMENT ON COLUMN agent_audit_records.target_id IS '操作目标稳定标识';
COMMENT ON COLUMN agent_audit_records.outcome IS 'allowed/denied 表示授权结果，succeeded/failed 表示执行结果';
COMMENT ON COLUMN agent_audit_records.detail IS '低敏附加信息，不含提示词、资源正文或秘密';

-- Agent tracing 服务拥有自己的来源凭据、Run 投影、追加事件与导出审计。

CREATE TABLE producer_sources (
    source_id TEXT PRIMARY KEY,
    project_id TEXT NOT NULL,
    token_hash BYTEA NOT NULL UNIQUE,
    enabled BOOLEAN NOT NULL DEFAULT TRUE,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    CONSTRAINT producer_sources_source_id_nonempty CHECK (length(source_id) BETWEEN 1 AND 128),
    CONSTRAINT producer_sources_project_id_nonempty CHECK (length(project_id) BETWEEN 1 AND 128)
);

COMMENT ON TABLE producer_sources IS 'Agent producer 来源与项目绑定；数据库只保存凭据 SHA-256 摘要';
COMMENT ON COLUMN producer_sources.source_id IS 'producer 来源唯一标识';
COMMENT ON COLUMN producer_sources.project_id IS 'source 所属项目标识';
COMMENT ON COLUMN producer_sources.token_hash IS 'producer bearer token 的 SHA-256 摘要，不保存明文';
COMMENT ON COLUMN producer_sources.enabled IS '来源是否允许继续写入';
COMMENT ON COLUMN producer_sources.created_at IS '来源首次登记时间';

CREATE TABLE agent_threads (
    thread_id TEXT NOT NULL,
    source_id TEXT NOT NULL REFERENCES producer_sources(source_id),
    project_id TEXT NOT NULL,
    created_at TIMESTAMPTZ NOT NULL,
    last_activity_at TIMESTAMPTZ NOT NULL,
    PRIMARY KEY (thread_id, source_id),
    CONSTRAINT agent_threads_thread_id_unique UNIQUE (thread_id),
    CONSTRAINT agent_threads_thread_id_nonempty CHECK (length(thread_id) BETWEEN 1 AND 256)
);

COMMENT ON TABLE agent_threads IS '从 Run 投影聚合的 thread 查询索引；事件仍是事实源';
COMMENT ON COLUMN agent_threads.thread_id IS 'canonical 会话标识，全服务唯一';
COMMENT ON COLUMN agent_threads.source_id IS '拥有该会话的 producer 来源';
COMMENT ON COLUMN agent_threads.project_id IS '来源配置绑定的项目标识';
COMMENT ON COLUMN agent_threads.created_at IS '会话中首次 Run 创建时间';
COMMENT ON COLUMN agent_threads.last_activity_at IS '会话中最近事件时间';

CREATE TABLE agent_runs (
    source_id TEXT NOT NULL REFERENCES producer_sources(source_id),
    run_id TEXT NOT NULL,
    project_id TEXT NOT NULL,
    thread_id TEXT NOT NULL,
    agent_id TEXT NOT NULL,
    status TEXT NOT NULL DEFAULT 'queued',
    trace_id TEXT,
    created_at TIMESTAMPTZ NOT NULL,
    started_at TIMESTAMPTZ,
    finished_at TIMESTAMPTZ,
    accepted_through_seq BIGINT NOT NULL DEFAULT 0,
    last_activity_at TIMESTAMPTZ NOT NULL,
    PRIMARY KEY (source_id, run_id),
    CONSTRAINT agent_runs_run_id_unique UNIQUE (run_id),
    CONSTRAINT agent_runs_thread_fk FOREIGN KEY (thread_id, source_id) REFERENCES agent_threads(thread_id, source_id),
    CONSTRAINT agent_runs_status_valid CHECK (status IN ('queued', 'running', 'cancelling', 'succeeded', 'failed', 'cancelled', 'interrupted')),
    CONSTRAINT agent_runs_accepted_seq_nonnegative CHECK (accepted_through_seq >= 0)
);

COMMENT ON TABLE agent_runs IS '从追加事件重建的 Run 查询投影，不是事件事实源';
COMMENT ON COLUMN agent_runs.source_id IS '写入该 Run 的 producer 来源';
COMMENT ON COLUMN agent_runs.run_id IS 'producer 提供的 Run 标识，在 source 内唯一';
COMMENT ON COLUMN agent_runs.project_id IS '来源配置绑定的项目标识';
COMMENT ON COLUMN agent_runs.thread_id IS '由 run.created 事件确定的会话标识';
COMMENT ON COLUMN agent_runs.agent_id IS '由 run.created 事件确定的 Agent 标识';
COMMENT ON COLUMN agent_runs.status IS '从 Run 生命周期事件投影的状态';
COMMENT ON COLUMN agent_runs.trace_id IS '首个有效事件 trace ID，缺省时由查询投影稳定派生';
COMMENT ON COLUMN agent_runs.created_at IS 'Run 首条事件发生时间';
COMMENT ON COLUMN agent_runs.started_at IS 'run.started 事件发生时间';
COMMENT ON COLUMN agent_runs.finished_at IS 'Run 终态事件发生时间';
COMMENT ON COLUMN agent_runs.accepted_through_seq IS '事务已提交的连续事件序号检查点';
COMMENT ON COLUMN agent_runs.last_activity_at IS '最后一条已提交事件的发生时间';

CREATE INDEX agent_runs_created_at_idx ON agent_runs (created_at DESC, run_id);
CREATE INDEX agent_runs_thread_activity_idx ON agent_runs (thread_id, last_activity_at DESC);
CREATE INDEX agent_runs_status_created_idx ON agent_runs (status, created_at DESC);

CREATE TABLE agent_run_events (
    source_id TEXT NOT NULL,
    run_id TEXT NOT NULL,
    seq BIGINT NOT NULL,
    event_type TEXT NOT NULL,
    occurred_at TIMESTAMPTZ NOT NULL,
    recorded_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    trace_id TEXT,
    span_id TEXT,
    parent_span_id TEXT,
    payload JSONB NOT NULL,
    envelope JSONB NOT NULL,
    event_hash BYTEA NOT NULL,
    PRIMARY KEY (source_id, run_id, seq),
    CONSTRAINT agent_run_events_run_fk FOREIGN KEY (source_id, run_id) REFERENCES agent_runs(source_id, run_id) ON DELETE CASCADE,
    CONSTRAINT agent_run_events_seq_positive CHECK (seq > 0)
);

COMMENT ON TABLE agent_run_events IS '按 Run 序号追加的 canonical Agent 事件；轨迹树与账本由此重建';
COMMENT ON COLUMN agent_run_events.source_id IS 'producer 来源标识';
COMMENT ON COLUMN agent_run_events.run_id IS '事件所属 Run 标识';
COMMENT ON COLUMN agent_run_events.seq IS 'Run 内从 1 开始的连续序号';
COMMENT ON COLUMN agent_run_events.event_type IS '冻结目录中的事件类型或扩展事件类型';
COMMENT ON COLUMN agent_run_events.occurred_at IS 'producer 声明的事件发生时间';
COMMENT ON COLUMN agent_run_events.recorded_at IS '数据库提交事件的服务端时间';
COMMENT ON COLUMN agent_run_events.trace_id IS 'OpenTelemetry 格式的 trace 标识；可为空';
COMMENT ON COLUMN agent_run_events.span_id IS 'OpenTelemetry 格式的 span 标识；可为空';
COMMENT ON COLUMN agent_run_events.parent_span_id IS 'producer 声明的父 span 标识；只接受显式关系';
COMMENT ON COLUMN agent_run_events.payload IS '按事件目录验证并保留的 canonical 载荷';
COMMENT ON COLUMN agent_run_events.envelope IS '完整可导出的事件 envelope，不含 producer 凭据';
COMMENT ON COLUMN agent_run_events.event_hash IS '规范化 envelope 的 SHA-256，用于识别精确重试与序号冲突';

CREATE INDEX agent_run_events_type_time_idx ON agent_run_events (event_type, occurred_at DESC);

CREATE TABLE agent_run_export_audit (
    id BIGINT GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    source_id TEXT NOT NULL,
    run_id TEXT NOT NULL,
    admin_user_id TEXT NOT NULL,
    event_count INTEGER NOT NULL,
    byte_count BIGINT NOT NULL,
    exported_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    CONSTRAINT agent_run_export_audit_run_fk FOREIGN KEY (source_id, run_id) REFERENCES agent_runs(source_id, run_id) ON DELETE CASCADE,
    CONSTRAINT agent_run_export_audit_counts_nonnegative CHECK (event_count >= 0 AND byte_count >= 0)
);

COMMENT ON TABLE agent_run_export_audit IS '原始事件导出审计记录；只写不读，不保存导出内容';
COMMENT ON COLUMN agent_run_export_audit.id IS '导出审计记录自增标识';
COMMENT ON COLUMN agent_run_export_audit.source_id IS '被导出 Run 的来源标识';
COMMENT ON COLUMN agent_run_export_audit.run_id IS '被导出 Run 标识';
COMMENT ON COLUMN agent_run_export_audit.admin_user_id IS 'accounts-service 验证的管理员 user_id';
COMMENT ON COLUMN agent_run_export_audit.event_count IS '导出的事件条数';
COMMENT ON COLUMN agent_run_export_audit.byte_count IS '导出 JSON 文件字节数';
COMMENT ON COLUMN agent_run_export_audit.exported_at IS '导出响应准备完成时间';

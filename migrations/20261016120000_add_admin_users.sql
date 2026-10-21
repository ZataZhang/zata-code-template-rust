-- 管理员账号与用户启停。
--
-- 管理员是独立的一张表，与 users 完全隔离：没有「用户表里加管理员标志」这回事，
-- 普通账号被攻破拿不到任何管理权限。管理员只能由启动时的引导账号创建，没有注册入口。
-- 不修改已应用的 create_users 迁移，users 的新增列在这里追加。

CREATE TABLE admin_users (
    id            BIGINT GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    username      TEXT        NOT NULL,
    display_name  TEXT        NOT NULL,
    password_hash TEXT        NOT NULL,
    created_at    TIMESTAMPTZ NOT NULL DEFAULT now(),
    -- 唯一约束保证并发引导时只有一个成功。
    CONSTRAINT admin_users_username_unique UNIQUE (username),
    -- 应用层写入前会转成小写；这里兜底。
    CONSTRAINT admin_users_username_lowercase CHECK (username = lower(username))
);

COMMENT ON TABLE admin_users IS '管理员账号：用户名加密码登录管理后台，与普通用户完全隔离';
COMMENT ON COLUMN admin_users.id IS '管理员 ID，数据库自增';
COMMENT ON COLUMN admin_users.username IS '登录用户名，已规范化为小写，全局唯一';
COMMENT ON COLUMN admin_users.display_name IS '后台界面展示的名称';
COMMENT ON COLUMN admin_users.password_hash IS '密码哈希（argon2id 的 PHC 字符串），绝不存明文';
COMMENT ON COLUMN admin_users.created_at IS '创建时间';

ALTER TABLE users
    ADD COLUMN status TEXT NOT NULL DEFAULT 'active',
    ADD CONSTRAINT users_status_valid CHECK (status IN ('active', 'disabled'));

COMMENT ON COLUMN users.status IS '账号状态：active 可登录；disabled 被管理员停用，不能登录且已有登录态立即失效。数据不删除';

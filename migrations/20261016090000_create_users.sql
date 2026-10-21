-- 用户表：邮箱 + 密码哈希。
--
-- 约定（见 docs/ai-standards/migrations.md）：
-- - 表和每一列都要有 COMMENT，作为运维排障时的字段说明；
-- - 已应用到任何环境的迁移文件不要再修改，需要变更就新增一个迁移。

CREATE TABLE users (
    id            BIGINT GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    email         TEXT        NOT NULL,
    password_hash TEXT        NOT NULL,
    created_at    TIMESTAMPTZ NOT NULL DEFAULT now(),
    -- 唯一约束保证并发注册同一邮箱时只有一个成功。
    CONSTRAINT users_email_unique UNIQUE (email),
    -- 应用层写入前会转成小写；这里兜底，防止绕过应用层写入大小写不同的重复邮箱。
    CONSTRAINT users_email_lowercase CHECK (email = lower(email))
);

COMMENT ON TABLE users IS '用户账号：邮箱加密码登录';
COMMENT ON COLUMN users.id IS '用户 ID，数据库自增';
COMMENT ON COLUMN users.email IS '登录邮箱，已规范化为小写，全局唯一';
COMMENT ON COLUMN users.password_hash IS '密码哈希（argon2id 的 PHC 字符串），绝不存明文';
COMMENT ON COLUMN users.created_at IS '注册时间';

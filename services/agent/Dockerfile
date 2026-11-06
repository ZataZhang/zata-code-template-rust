# syntax=docker/dockerfile:1
# Agent 服务独立镜像。构建上下文是仓库根目录：
#   docker build -f services/agent/Dockerfile -t tpl-agent:local .
#
# 运行期配置来自根 config.toml 的 [agent] 子树与环境变量：
#   - DATABASE_URL / APP_AGENT__DATABASE__* 指向 Agent 专属 PostgreSQL；
#   - AGENT_SERVICE_TOKENS_JSON 是调用服务凭据清单；
#   - AGENT_TRACING_PRODUCER_TOKEN 是向 agent-tracing 投递事件的 producer token；
#   - A2A 外部 Runtime 的密钥按 credential_alias 以 AGENT_CREDENTIAL_<ALIAS> 注入。
# 这些值都是秘密，只能由部署平台提供，不写进镜像与配置文件。

FROM rust:1-slim-bookworm AS builder
WORKDIR /app

COPY Cargo.toml Cargo.lock ./
COPY services ./services
COPY platform ./platform
COPY tools ./tools
COPY examples ./examples
COPY .sqlx ./.sqlx

RUN --mount=type=cache,target=/usr/local/cargo/registry \
    --mount=type=cache,target=/app/target \
    SQLX_OFFLINE=true cargo build --release --locked -p tpl-agent --bin agent-service \
    && cp target/release/agent-service /usr/local/bin/agent-service

FROM debian:bookworm-slim
# HTTP 客户端需要系统 CA 信任库验证 Nacos 与外部 Runtime 的 HTTPS 证书。
RUN apt-get update \
    && apt-get install --no-install-recommends --yes ca-certificates \
    && rm -rf /var/lib/apt/lists/* \
    && useradd --system --uid 10003 --no-create-home app
WORKDIR /app

COPY --from=builder /usr/local/bin/agent-service /usr/local/bin/agent-service
COPY config.toml ./config.toml
RUN mkdir -p /app/data/blobs && chown -R app /app/data

ENV APP_CONFIG_FILE=/app/config.toml
ENV APP_AGENT__SERVER__HOST=0.0.0.0
ENV APP_AGENT__STORAGE__ROOT=/app/data/blobs
USER app
EXPOSE 8091
ENTRYPOINT ["agent-service"]

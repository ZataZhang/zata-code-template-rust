# syntax=docker/dockerfile:1
# MCP 服务独立镜像。构建上下文是仓库根目录：
#   docker build -f services/mcp/Dockerfile -t tpl-mcp:local .
#
# 运行期秘密只由环境注入，不写进镜像与配置文件：
#   - MCP_DATABASE_URL 指向 MCP 专属 PostgreSQL；
#   - MCP_CREDENTIAL_KEY_BASE64 是凭据加密主密钥（Base64 的 32 字节）；
#   - MCP_CONSUMERS_JSON 是消费服务凭据与上下文签名密钥清单。
# 本镜像不依赖 accounts / agent 的私有 crate，也不读取它们的配置。

FROM rust:1-slim-bookworm AS builder
WORKDIR /app

COPY Cargo.toml Cargo.lock ./
COPY services ./services
COPY platform ./platform
COPY tools ./tools
COPY examples ./examples

RUN --mount=type=cache,target=/usr/local/cargo/registry \
    --mount=type=cache,target=/app/target \
    cargo build --release --locked -p tpl-mcp --bin mcp-service \
    && cp target/release/mcp-service /usr/local/bin/mcp-service

FROM debian:bookworm-slim
# 出站 HTTPS 需要系统 CA 信任库校验远端 MCP server 的证书。
RUN apt-get update \
    && apt-get install --no-install-recommends --yes ca-certificates \
    && rm -rf /var/lib/apt/lists/* \
    && useradd --system --uid 10004 --no-create-home app
WORKDIR /app

COPY --from=builder /usr/local/bin/mcp-service /usr/local/bin/mcp-service
COPY services/mcp/config.toml ./config.toml

ENV APP_MCP_CONFIG_FILE=/app/config.toml
ENV APP_MCP__SERVER__HOST=0.0.0.0
USER app
EXPOSE 8092
ENTRYPOINT ["mcp-service"]

# syntax=docker/dockerfile:1

# ---- 构建阶段 ----
FROM rust:1-slim-bookworm AS builder
WORKDIR /app

COPY Cargo.toml Cargo.lock ./
COPY crates ./crates
# 工作区成员里还有 tools/*（开发期工具 xtask）：不拷进来 cargo 解析不了工作区；它不会被编译进镜像。
COPY tools ./tools
# 迁移 SQL 在编译期被嵌进二进制（sqlx::migrate!），所以构建上下文里必须有它；运行阶段不需要再带这个目录。
COPY migrations ./migrations
# query! 宏使用提交的离线 schema；镜像构建不需要连接数据库。
COPY .sqlx ./.sqlx

# 缓存依赖下载与编译产物，第二次构建只重编改动的部分。
# 缓存挂载在构建结束后不会留在镜像里，所以要在同一个 RUN 里把二进制拷出来。
RUN --mount=type=cache,target=/usr/local/cargo/registry \
    --mount=type=cache,target=/app/target \
    SQLX_OFFLINE=true cargo build --release --locked -p tpl-app --bin server \
    && cp target/release/server /usr/local/bin/server

# ---- 运行阶段：只放一个二进制 + 配置文件，不带 Rust 工具链 ----
FROM debian:bookworm-slim
RUN useradd --system --uid 10001 --no-create-home app
WORKDIR /app

COPY --from=builder /usr/local/bin/server /usr/local/bin/server
COPY config.toml ./config.toml

# 容器里要监听所有网卡；其他配置用环境变量注入（见 config.toml 顶部说明）。
ENV APP_SERVER__HOST=0.0.0.0
USER app
EXPOSE 8080

# 存活/就绪探针由编排平台直接请求 /live 与 /ready，镜像里不带 curl。
ENTRYPOINT ["server"]

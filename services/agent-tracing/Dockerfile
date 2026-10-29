# syntax=docker/dockerfile:1

FROM rust:1-slim-bookworm AS builder
WORKDIR /app

COPY Cargo.toml Cargo.lock ./
COPY services ./services
COPY tools ./tools
COPY .sqlx ./.sqlx

RUN --mount=type=cache,target=/usr/local/cargo/registry \
    --mount=type=cache,target=/app/target \
    SQLX_OFFLINE=true cargo build --release --locked -p tpl-agent-tracing --bin agent-tracing-service \
    && cp target/release/agent-tracing-service /usr/local/bin/agent-tracing-service

FROM debian:bookworm-slim
RUN useradd --system --uid 10002 --no-create-home app
WORKDIR /app

COPY --from=builder /usr/local/bin/agent-tracing-service /usr/local/bin/agent-tracing-service
COPY services/agent-tracing/config.toml ./config.toml

ENV APP_CONFIG_FILE=/app/config.toml
ENV APP_SERVER__HOST=0.0.0.0
USER app
EXPOSE 8090
ENTRYPOINT ["agent-tracing-service"]

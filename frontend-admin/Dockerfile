# syntax=docker/dockerfile:1
# ------------------------------------------------------------------------------
# 管理后台（Vite 静态站点 + nginx）
# 构建上下文就是本目录：docker build -t tpl-admin .
# 运行时用环境变量 BACKEND_UPSTREAM（host:port，默认 backend:8080）指定后端，
# TRUSTED_PROXY_CIDR 指定可信的上一层代理地址段（默认不信任）；
# 浏览器只访问本站，/api/* 由 nginx 反向代理到后端，所以后端不需要开放 CORS。
# ------------------------------------------------------------------------------

# ── 构建阶段 ──────────────────────────────────────────────────────────────────
FROM node:22-alpine AS builder
WORKDIR /app
ENV CI=true
RUN npm install -g pnpm@11.3.0

# 先只放依赖清单，让依赖层能被 Docker 缓存复用。
COPY package.json pnpm-lock.yaml pnpm-workspace.yaml ./
RUN pnpm install --frozen-lockfile

COPY . .
RUN pnpm build

# ── 运行阶段 ──────────────────────────────────────────────────────────────────
FROM nginx:1.27-alpine
# 官方镜像会在启动时把 /etc/nginx/templates/*.template 里的 ${变量} 用环境变量替换后写入 conf.d。
ENV BACKEND_UPSTREAM=backend:8080
# 上一层 HTTPS 终止代理的地址段（默认不信任任何上一层，见 nginx.conf）。
ENV TRUSTED_PROXY_CIDR=127.0.0.1
COPY nginx.conf /etc/nginx/templates/default.conf.template
COPY --from=builder /app/dist /usr/share/nginx/html
EXPOSE 80

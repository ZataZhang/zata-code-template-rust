# syntax=docker/dockerfile:1
# ------------------------------------------------------------------------------
# 前台站点（Next.js standalone）
# 构建上下文就是本目录：docker build --build-arg BACKEND_URL=http://backend:8080 -t tpl-web .
# ------------------------------------------------------------------------------

# ── 构建阶段 ──────────────────────────────────────────────────────────────────
FROM node:22-alpine AS builder
WORKDIR /app
ENV CI=true
RUN npm install -g pnpm@11.3.0

# Next 的 rewrites 在**构建时**就固化进产物（.next/routes-manifest.json），运行时再改环境变量不会生效。
# 所以后端地址必须作为构建参数传入；默认值适用于「同一个 Compose 网络里有个叫 backend 的服务」。
ARG BACKEND_URL=http://backend:8080
ENV BACKEND_URL=${BACKEND_URL}

# 先只放依赖清单，让依赖层能被 Docker 缓存复用。
COPY package.json pnpm-lock.yaml pnpm-workspace.yaml ./
RUN pnpm install --frozen-lockfile

COPY . .
RUN pnpm build

# ── 运行阶段：只带 standalone 产物，不带依赖安装与源码 ───────────────────────
FROM node:22-alpine AS runner
WORKDIR /app
ENV NODE_ENV=production
ENV PORT=3000
ENV HOSTNAME=0.0.0.0

RUN addgroup -S app && adduser -S app -G app
COPY --from=builder --chown=app:app /app/.next/standalone ./
COPY --from=builder --chown=app:app /app/.next/static ./.next/static
COPY --from=builder --chown=app:app /app/public ./public

USER app
EXPOSE 3000
CMD ["node", "server.js"]

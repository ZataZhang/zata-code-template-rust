# syntax=docker/dockerfile:1
# ------------------------------------------------------------------------------
# 前台站点（Next.js standalone）；构建上下文是仓库根目录：
# docker build -f frontends/public/Dockerfile --build-arg BACKEND_URL=http://accounts:8080 .
# ------------------------------------------------------------------------------

FROM node:22-alpine AS builder
WORKDIR /repo
ENV CI=true
RUN npm install -g pnpm@11.3.0

# 账号 UI 是一个独立打包的 React 包，先构建它，再安装消费该包的站点。
COPY packages/accounts-ui ./packages/accounts-ui
RUN --mount=type=cache,id=tpl-web-pnpm-store,target=/pnpm/store,sharing=locked \
    pnpm --dir packages/accounts-ui install --frozen-lockfile \
    --store-dir=/pnpm/store --network-concurrency=4 --fetch-timeout=600000 --fetch-retries=5 \
    && pnpm --dir packages/accounts-ui build

# Next rewrites 在构建时写入 standalone 产物，必须在构建阶段提供服务地址。
ARG BACKEND_URL=http://accounts:8080
ENV BACKEND_URL=${BACKEND_URL}

COPY frontends/public/package.json frontends/public/pnpm-lock.yaml frontends/public/pnpm-workspace.yaml ./frontends/public/
WORKDIR /repo/frontends/public
RUN --mount=type=cache,id=tpl-web-pnpm-store,target=/pnpm/store,sharing=locked \
    pnpm install --frozen-lockfile --store-dir=/pnpm/store \
    --network-concurrency=4 --fetch-timeout=600000 --fetch-retries=5

COPY frontends/public ./
RUN pnpm build

FROM node:22-alpine AS runner
WORKDIR /app
ENV NODE_ENV=production
ENV PORT=3000
ENV HOSTNAME=0.0.0.0

RUN addgroup -S app && adduser -S app -G app
COPY --from=builder --chown=app:app /repo/frontends/public/.next/standalone ./
COPY --from=builder --chown=app:app /repo/frontends/public/.next/static ./frontends/public/.next/static
COPY --from=builder --chown=app:app /repo/frontends/public/public ./frontends/public/public

USER app
EXPOSE 3000
CMD ["node", "frontends/public/server.js"]

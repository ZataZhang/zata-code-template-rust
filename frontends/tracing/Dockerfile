# syntax=docker/dockerfile:1

FROM node:22-alpine AS builder
WORKDIR /app
ENV CI=true
RUN npm install -g pnpm@11.3.0

COPY package.json pnpm-lock.yaml pnpm-workspace.yaml ./
RUN pnpm install --frozen-lockfile
COPY . .
RUN pnpm build

FROM nginx:1.27-alpine
ENV ACCOUNTS_UPSTREAM=accounts:8080
ENV TRACING_UPSTREAM=agent-tracing:8090
ENV TRUSTED_PROXY_CIDR=127.0.0.1
COPY nginx.conf /etc/nginx/templates/default.conf.template
COPY --from=builder /app/dist /usr/share/nginx/html
EXPOSE 80

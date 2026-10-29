import type { NextConfig } from "next"
import createNextIntlPlugin from "next-intl/plugin"
import path from "node:path"

// 把 i18n/request.ts 注册为 next-intl 的请求配置入口：插件在服务端渲染时
// 调用它解析 locale 与文案，客户端则由根布局注入的 Provider 提供。
const withNextIntl = createNextIntlPlugin("./i18n/request.ts")

// Rust 后端的默认监听地址（见仓库根的 config.toml）。
const backendUrl =
  process.env.BACKEND_URL || process.env.API_BASE_URL || "http://localhost:8080"

const nextConfig: NextConfig = {
  // 生产镜像的 runtime stage 只拷 .next/standalone，不装依赖；不开这个开关
  // 该目录压根不会生成，镜像构建会在 COPY 处失败。
  output: "standalone",
  outputFileTracingRoot: path.join(import.meta.dirname, ".."),
  transpilePackages: ["@tpl/accounts-ui"],
  devIndicators: false,
  allowedDevOrigins: ["127.0.0.1"],
  async rewrites() {
    return [
      {
        source: "/api/:path*",
        destination: `${backendUrl}/:path*`,
      },
    ]
  },
}

export default withNextIntl(nextConfig)

import type { NextRequest } from "next/server"

/**
 * 浏览器只访问同源 `/api/agent/*`；这个 Route Handler 在服务端注入 Agent 服务凭据并转发 Cookie。
 * 服务令牌永远不会出现在浏览器里。
 */
export const dynamic = "force-dynamic"

const AGENT_SERVICE_URL = (process.env.AGENT_SERVICE_URL ?? "http://127.0.0.1:8091").replace(
  /\/+$/,
  ""
)
const AGENT_SERVICE_TOKEN = process.env.AGENT_SERVICE_TOKEN ?? ""

const FORWARDED_REQUEST_HEADERS = ["accept", "content-type", "cookie", "idempotency-key"]

type RouteContext = { params: Promise<{ path: string[] }> }

/** 把 Next 的路径片段安全地拼到固定的 Agent 服务地址后。 */
function upstreamUrl(request: NextRequest, segments: string[]): string {
  const encodedPath = segments.map(encodeURIComponent).join("/")
  const query = request.nextUrl.search
  return `${AGENT_SERVICE_URL}/api/${encodedPath}${query}`
}

/** 代理一次 Agent HTTP 请求，保留状态码、内容类型与流式响应体。 */
async function proxy(request: NextRequest, context: RouteContext): Promise<Response> {
  if (!AGENT_SERVICE_TOKEN) {
    return Response.json(
      { error: { code: "agent_unavailable", message: "Agent 服务凭据未配置" } },
      { status: 503 }
    )
  }
  const { path } = await context.params
  const headers = new Headers()
  for (const name of FORWARDED_REQUEST_HEADERS) {
    const value = request.headers.get(name)
    if (value) headers.set(name, value)
  }
  headers.set("authorization", `Bearer ${AGENT_SERVICE_TOKEN}`)

  const hasBody = request.method !== "GET" && request.method !== "HEAD"
  const body = hasBody ? await request.arrayBuffer() : undefined
  let upstream: Response
  try {
    upstream = await fetch(upstreamUrl(request, path), {
      method: request.method,
      headers,
      body,
      cache: "no-store",
    })
  } catch {
    return Response.json(
      { error: { code: "agent_unavailable", message: "Agent 服务暂不可用" } },
      { status: 503 }
    )
  }

  const responseHeaders = new Headers()
  const contentType = upstream.headers.get("content-type")
  if (contentType) responseHeaders.set("content-type", contentType)
  const contentDisposition = upstream.headers.get("content-disposition")
  if (contentDisposition) responseHeaders.set("content-disposition", contentDisposition)
  responseHeaders.set("cache-control", "no-store")
  return new Response(upstream.body, {
    status: upstream.status,
    headers: responseHeaders,
  })
}

export const GET = proxy
export const POST = proxy
export const PATCH = proxy
export const PUT = proxy
export const DELETE = proxy

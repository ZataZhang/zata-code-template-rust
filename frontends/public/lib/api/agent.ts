/** Agent 服务 public HTTP 客户端：浏览器只访问同源 `/api/agent/*`，服务凭据由 Next 服务端注入。 */
import { ApiRequestError } from "./client"

const AGENT_API_PREFIX = "/api/agent/v1"

/** Agent 列表的统一包装。 */
export type AgentList<T> = { items: T[] }

/** 组织目录项。 */
export type AgentOrganization = {
  id: string
  slug: string
  display_name: string
  created_at: string
}

/** Business Agent descriptor。 */
export type BusinessAgent = {
  id: string
  organization_id: string
  runtime_connection_id: string
  name: string
  display_name: string
  description: string
  prompt: string
  owner_admin_id: number
  enabled: boolean
  descriptor_checksum: string
  created_at: string
  updated_at: string
}

/** public 可用 Agent 条目。 */
export type AvailableAgent = {
  agent: BusinessAgent
  runtime_name: string
  auto_visible: boolean
  priority: number
}

/** 对话 Session。 */
export type ChatSession = {
  id: string
  organization_id: string
  owner_user_id: number
  agent_id: string | null
  title: string
  created_at: string
  last_activity_at: string
}

/** 消息角色。 */
export type MessageRole = "user" | "assistant"

/** Session 内消息。 */
export type ChatMessage = {
  id: string
  session_id: string
  run_id: string | null
  role: MessageRole
  content: string
  resource_ids: string[]
  created_at: string
}

/** Run 状态。 */
export type RunStatus =
  | "queued"
  | "running"
  | "cancelling"
  | "succeeded"
  | "failed"
  | "cancelled"

/** Run 来源。 */
export type RunOrigin = "direct" | "auto" | "delegation"

/** Run 的持久化记录（只声明前端用到的字段，其余由服务端保留）。 */
export type AgentRun = {
  run_id: string
  organization_id: string
  session_id: string
  owner_user_id: number
  agent_id: string
  runtime_connection_id: string
  origin: RunOrigin
  status: RunStatus
  idempotency_key: string
  request_digest: string
  last_event_seq: number
  trace_delivered_through_seq: number
  error_code: string | null
  created_at: string
  started_at: string | null
  finished_at: string | null
  snapshot: {
    agent: {
      agent_id: string
      name: string
      display_name: string
      descriptor_checksum: string
      runtime: {
        runtime_id: string
        name: string
        origin: string
        adapter_kind: string
      }
    }
    input: { content: string }
    context_pack_checksum: string
  }
}

/** 创建/重放 Run 的响应。 */
export type RunCreated = { run: AgentRun; created: boolean }

/** 取消 Run 的响应。 */
export type RunCancelled = { run: AgentRun; cancelled_immediately: boolean }

/** 持久化 Run 事件。 */
export type RunEvent = {
  run_id: string
  seq: number
  event_type: string
  occurred_at: string
  payload: unknown
}

/** Session 详情。 */
export type SessionDetail = {
  session: ChatSession
  messages: ChatMessage[]
  runs: AgentRun[]
}

/** 产物元数据。 */
export type AgentArtifact = {
  id: string
  run_id: string
  owner_user_id: number
  filename: string
  media_type: string
  size_bytes: number
  sha256: string
  created_at: string
}

/** 用户资源元数据。 */
export type AgentResource = {
  id: string
  organization_id: string
  owner_user_id: number
  filename: string
  media_type: string
  size_bytes: number
  sha256: string
  created_at: string
}

/** Skill 元数据。 */
export type AgentSkill = {
  id: string
  organization_id: string
  name: string
  description: string
  instructions: string
  enabled: boolean
  created_at: string
  updated_at: string
}

/** 发送消息的可选上下文。 */
export type SendMessagePayload = {
  agent_id?: string | null
  content: string
  resource_ids?: string[]
  skill_ids?: string[]
}

async function agentRequest<T>(path: string, init?: RequestInit): Promise<T> {
  let response: Response
  try {
    response = await fetch(`${AGENT_API_PREFIX}${path}`, {
      ...init,
      credentials: "include",
      cache: "no-store",
      signal: init?.signal ?? AbortSignal.timeout(30_000),
    })
  } catch {
    throw new ApiRequestError("Agent request failed", undefined, "agent_unavailable")
  }
  if (!response.ok) {
    let body: { error?: { code?: unknown; message?: unknown } } = {}
    try {
      body = (await response.json()) as typeof body
    } catch {
      body = {}
    }
    const code = typeof body.error?.code === "string" ? body.error.code : undefined
    const message =
      typeof body.error?.message === "string" ? body.error.message : "Agent request failed"
    throw new ApiRequestError(message, response.status, code)
  }
  if (response.status === 204) return undefined as T
  return (await response.json()) as T
}

/** 当前用户加入的组织。 */
export async function listOrganizations(): Promise<AgentOrganization[]> {
  const response = await agentRequest<AgentList<AgentOrganization>>("/organizations")
  return response.items
}

/** 当前用户在指定组织内可用的 Agent。 */
export async function listAvailableAgents(
  organizationId: string
): Promise<AvailableAgent[]> {
  const response = await agentRequest<AgentList<AvailableAgent>>(
    `/organizations/${organizationId}/agents`
  )
  return response.items
}

/** 指定 Agent 可选择的 Skill。 */
export async function listAgentSkills(agentId: string): Promise<AgentSkill[]> {
  const response = await agentRequest<AgentList<AgentSkill>>(`/agents/${agentId}/skills`)
  return response.items
}

/** 当前用户在组织内创建的 Session。 */
export async function listSessions(organizationId: string): Promise<ChatSession[]> {
  const response = await agentRequest<AgentList<ChatSession>>(
    `/organizations/${organizationId}/sessions`
  )
  return response.items
}

/** 新建 Session；agent_id 为空表示 Auto 入口。 */
export async function createSession(
  organizationId: string,
  payload: { agent_id?: string | null; title?: string } = {}
): Promise<ChatSession> {
  return agentRequest<ChatSession>(`/organizations/${organizationId}/sessions`, {
    method: "POST",
    headers: { "Content-Type": "application/json" },
    body: JSON.stringify(payload),
  })
}

/** 读取 Session、消息与 Run 历史。 */
export async function getSessionDetail(sessionId: string): Promise<SessionDetail> {
  return agentRequest<SessionDetail>(`/sessions/${sessionId}`)
}

/** 发送消息并创建/重放 Run。 */
export async function sendMessage(
  sessionId: string,
  payload: SendMessagePayload,
  idempotencyKey: string
): Promise<RunCreated> {
  return agentRequest<RunCreated>(`/sessions/${sessionId}/messages`, {
    method: "POST",
    headers: {
      "Content-Type": "application/json",
      "Idempotency-Key": idempotencyKey,
    },
    body: JSON.stringify(payload),
    signal: AbortSignal.timeout(30_000),
  })
}

/** 读取 Run 当前状态。 */
export async function getRun(runId: string): Promise<AgentRun> {
  return agentRequest<AgentRun>(`/runs/${runId}`)
}

/** 读取检查点之后的持久事件。 */
export async function listRunEvents(runId: string, afterSeq = 0): Promise<RunEvent[]> {
  const response = await agentRequest<AgentList<RunEvent>>(
    `/runs/${runId}/events?after_seq=${afterSeq}`
  )
  return response.items
}

/** 取消 Run；重复调用保持幂等。 */
export async function cancelRun(runId: string): Promise<RunCancelled> {
  return agentRequest<RunCancelled>(`/runs/${runId}/cancel`, { method: "POST" })
}

/** 列出 Run 产物。 */
export async function listRunArtifacts(runId: string): Promise<AgentArtifact[]> {
  const response = await agentRequest<AgentList<AgentArtifact>>(`/runs/${runId}/artifacts`)
  return response.items
}

/** 列出用户已上传资源。 */
export async function listResources(organizationId: string): Promise<AgentResource[]> {
  const response = await agentRequest<AgentList<AgentResource>>(
    `/organizations/${organizationId}/resources`
  )
  return response.items
}

/** 上传用户资源；正文只经过 Next 服务端代理。 */
export async function uploadResource(
  organizationId: string,
  file: File
): Promise<AgentResource> {
  const form = new FormData()
  form.append("file", file)
  return agentRequest<AgentResource>(`/organizations/${organizationId}/resources`, {
    method: "POST",
    body: form,
    signal: AbortSignal.timeout(60_000),
  })
}

/** 产物下载地址；同源代理会注入服务凭据与 Cookie。 */
export function artifactDownloadUrl(artifactId: string): string {
  return `${AGENT_API_PREFIX}/artifacts/${artifactId}`
}

/** 用 fetch 读取 SSE，断线后由调用方用最新序号续读。 */
export async function streamRunEvents(
  runId: string,
  afterSeq: number,
  onEvent: (event: RunEvent) => void,
  signal: AbortSignal
): Promise<void> {
  const response = await fetch(
    `${AGENT_API_PREFIX}/runs/${runId}/stream?after_seq=${afterSeq}`,
    { credentials: "include", cache: "no-store", signal }
  )
  if (!response.ok || !response.body) {
    throw new ApiRequestError("Run stream unavailable", response.status, "agent_unavailable")
  }
  const reader = response.body.getReader()
  const decoder = new TextDecoder()
  let buffer = ""
  for (;;) {
    const { done, value } = await reader.read()
    if (done) break
    buffer += decoder.decode(value, { stream: true })
    let boundary = buffer.indexOf("\n\n")
    while (boundary >= 0) {
      const chunk = buffer.slice(0, boundary)
      buffer = buffer.slice(boundary + 2)
      let data = ""
      for (const line of chunk.split("\n")) {
        if (line.startsWith("data:")) data += line.slice(5).trimStart()
      }
      if (data) {
        try {
          onEvent(JSON.parse(data) as RunEvent)
        } catch {
          // 非 JSON 的 keep-alive 注释由服务端处理，这里忽略。
        }
      }
      boundary = buffer.indexOf("\n\n")
    }
  }
}

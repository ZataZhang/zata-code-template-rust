export type RunStatus = 'queued' | 'running' | 'cancelling' | 'succeeded' | 'failed' | 'cancelled' | 'interrupted'
export type SpanStatus = 'running' | 'completed' | 'failed' | 'incomplete'
export type SpanKind = 'root' | 'model' | 'tool' | 'artifact' | 'decision'

export interface AdminIdentity {
  user_id: string
  display_name: string
  username: string
}

export interface TraceSummary {
  run_id: string
  thread_id: string
  source_id: string
  project_id: string
  agent_id: string
  agent_name: string
  status: RunStatus
  created_at: string
  started_at: string | null
  finished_at: string | null
  duration_ms: number | null
  trace_id: string
  visibility: 'full' | 'limited'
  model_call_count: number
  tool_call_count: number
  usage: Record<string, number> | null
  error_count: number
}

export interface DecisionPayload {
  decision_id: string
  gate: string
  tool_name: string
  tool_call_ref: string
  verdict: string
  answer: string
  probability: number | null
  probabilities: Record<string, number>
  irreversible: boolean | null
  fail_open: boolean
  policy_fingerprint: string
  elapsed_ms: number
  judgement_state: string
}

export interface TraceSpan {
  span_id: string
  parent_span_id: string | null
  kind: SpanKind
  name: string
  status: SpanStatus
  started_at: string | null
  finished_at: string | null
  duration_ms: number | null
  event_seq: number
  model_call_id: string | null
  model_name: string | null
  turn_index: number | null
  usage: Record<string, number> | null
  finish_reason: string | null
  tool_call_id: string | null
  tool_name: string | null
  tool_arguments: unknown | null
  tool_result: unknown | null
  result_checksum: string | null
  result_length: number | null
  error_code: string | null
  error_message: string | null
  diagnostics: string[]
  decision: DecisionPayload | null
}

export interface TraceTreeNode {
  span: TraceSpan
  children: TraceTreeNode[]
}

export interface TraceDiagnostic {
  code: string
  message: string
  span_id: string | null
}

export interface TraceDetail {
  summary: TraceSummary
  root_span_id: string | null
  spans: TraceSpan[]
  tree: TraceTreeNode[]
  diagnostics: TraceDiagnostic[]
  content_policy: string
}

export interface ThreadSummary {
  thread_id: string
  source_id: string
  project_id: string
  run_count: number
  limited_run_count: number
  agent_names: string[]
  status_counts: Record<string, number>
  created_at: string
  last_activity_at: string
}

export interface ThreadTraceRecord {
  record_id: string
  kind: string
  thread_id: string
  run_id: string
  seq: number
  occurred_at: string | null
  role: string | null
  content: unknown | null
  span: TraceSpan | null
  child_run_id: string | null
}

export interface ThreadTraceDetail {
  summary: ThreadSummary
  runs: TraceDetail[]
  records: ThreadTraceRecord[]
}

export interface Page<T> {
  items: T[]
  total: number
  page: number
  page_size: number
}

export interface RunFilters {
  search: string
  status: string
  created_after: string
  created_before: string
  page: number
}

export interface ThreadFilters {
  search: string
  status: string
  created_after: string
  created_before: string
  page: number
}

export class ApiError extends Error {
  constructor(
    message: string,
    readonly status: number,
    readonly code: string,
  ) {
    super(message)
    this.name = 'ApiError'
  }
}

async function requestJson<T>(path: string, init: RequestInit = {}): Promise<T> {
  const response = await fetch(path, {
    ...init,
    credentials: 'same-origin',
    cache: 'no-store',
    headers: {
      Accept: 'application/json',
      ...init.headers,
    },
  })
  if (!response.ok) {
    const body = (await response.json().catch(() => null)) as { error?: { code?: string; message?: string } } | null
    throw new ApiError(body?.error?.message ?? `请求失败（${response.status}）`, response.status, body?.error?.code ?? 'request_failed')
  }
  if (response.status === 204) return undefined as T
  return (await response.json()) as T
}

function queryString(values: Record<string, string | number | undefined>): string {
  const query = new URLSearchParams()
  for (const [key, value] of Object.entries(values)) {
    if (value !== undefined && value !== '') query.set(key, String(value))
  }
  return query.size ? `?${query.toString()}` : ''
}

export const api = {
  currentAdmin: () => requestJson<AdminIdentity>('/api/admin/auth/me'),
  login: (identifier: string, password: string) =>
    requestJson<AdminIdentity>('/api/admin/auth/login', {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({ identifier, password }),
    }),
  logout: () =>
    requestJson<void>('/api/admin/auth/logout', {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: '{}',
    }),
  listRuns: (filters: RunFilters) =>
    requestJson<Page<TraceSummary>>(
      `/api/admin/agent-traces${queryString({ search: filters.search, status: filters.status, created_after: filters.created_after, created_before: filters.created_before, page: filters.page, page_size: 20 })}`,
    ),
  getRun: (runId: string) => requestJson<TraceDetail>(`/api/admin/agent-traces/${encodeURIComponent(runId)}`),
  listThreads: (filters: ThreadFilters) =>
    requestJson<Page<ThreadSummary>>(
      `/api/admin/agent-traces/threads${queryString({ search: filters.search, status: filters.status, created_after: filters.created_after, created_before: filters.created_before, page: filters.page, page_size: 20 })}`,
    ),
  getThread: (threadId: string) =>
    requestJson<ThreadTraceDetail>(`/api/admin/agent-traces/threads/${encodeURIComponent(threadId)}`),
  exportRun: async (runId: string) => {
    const response = await fetch(`/api/admin/agent-traces/${encodeURIComponent(runId)}/export`, {
      credentials: 'same-origin',
      cache: 'no-store',
      headers: { Accept: 'application/json' },
    })
    if (!response.ok) {
      const body = (await response.json().catch(() => null)) as { error?: { code?: string; message?: string } } | null
      throw new ApiError(body?.error?.message ?? `导出失败（${response.status}）`, response.status, body?.error?.code ?? 'export_failed')
    }
    const blob = await response.blob()
    const objectUrl = URL.createObjectURL(blob)
    const anchor = document.createElement('a')
    anchor.href = objectUrl
    anchor.download = `agent-run-${runId}.json`
    anchor.click()
    URL.revokeObjectURL(objectUrl)
  },
}

import axios, { AxiosError, type AxiosRequestConfig } from 'axios'

/** 后端的统一错误体：`{"error": {"code": "...", "message": "..."}}`。 */
type ApiErrorBody = { error: { code: string; message: string } }

/** 判断响应体是不是后端的统一错误体。 */
function isApiErrorBody(data: unknown): data is ApiErrorBody {
  if (!data || typeof data !== 'object' || !('error' in data)) return false
  const error = data.error
  return (
    !!error &&
    typeof error === 'object' &&
    'code' in error &&
    typeof error.code === 'string' &&
    'message' in error &&
    typeof error.message === 'string'
  )
}

const apiClient = axios.create({
  baseURL: '/api',
  headers: {
    'Content-Type': 'application/json',
  },
  withCredentials: true,
})

/** Error thrown for failed API requests with status and detail. */
export class ApiRequestError extends Error {
  status: number
  /** 后端的错误码（如 `invalid_credentials`）；响应不是统一错误体时为 `null`。 */
  code: string | null
  detail: unknown

  /**
   * Create an API request error.
   *
   * @param status - HTTP response status code.
   * @param message - Human-readable error message.
   * @param detail - Raw error detail from the server.
   * @param code - Machine-readable error code from the server, if any.
   */
  constructor(
    status: number,
    message: string,
    detail: unknown,
    code: string | null = null
  ) {
    super(message)
    this.status = status
    this.code = code
    this.detail = detail
    this.name = 'ApiRequestError'
  }
}

/** Resolve a user-facing error message from an Axios error. */
function resolveErrorMessage(error: AxiosError): string {
  const data = error.response?.data
  if (isApiErrorBody(data)) return data.error.message
  if (typeof data === 'string' && data.length > 0) return data
  return error.message || `HTTP ${error.response?.status}`
}

/** Throw an ``ApiRequestError`` for an Axios failure, otherwise re-throw. */
function handleApiError(error: unknown): never {
  if (error instanceof AxiosError && error.response) {
    const data = error.response.data
    throw new ApiRequestError(
      error.response.status,
      resolveErrorMessage(error),
      data,
      isApiErrorBody(data) ? data.error.code : null
    )
  }
  throw error
}

/** Send a GET request and return the typed response data. */
export async function apiGet<T>(
  path: string,
  config?: AxiosRequestConfig
): Promise<T> {
  try {
    const response = await apiClient.get<T>(path, config)
    return response.data
  } catch (error) {
    handleApiError(error)
  }
}

/** Send a POST request with an optional body and return the typed response data. */
export async function apiPost<T>(
  path: string,
  body?: unknown,
  config?: AxiosRequestConfig
): Promise<T> {
  try {
    const response = await apiClient.post<T>(path, body, config)
    return response.data
  } catch (error) {
    handleApiError(error)
  }
}

/** Agent 管理 API 的同源前缀；Vite/nginx 代理会在服务端注入服务凭据。 */
const AGENT_API_BASE_URL = '/agent-api'

/** 请求 Agent 管理 API，复用统一的错误体解析。 */
async function agentRequest<T>(
  method: 'GET' | 'POST' | 'PUT' | 'DELETE',
  path: string,
  body?: unknown
): Promise<T> {
  try {
    const response = await apiClient.request<T>({
      baseURL: AGENT_API_BASE_URL,
      url: path,
      method,
      data: body,
    })
    return response.data
  } catch (error) {
    handleApiError(error)
  }
}

/** 发送 Agent 管理 GET 请求。 */
export async function apiAgentGet<T>(path: string): Promise<T> {
  return agentRequest<T>('GET', path)
}

/** 发送 Agent 管理 POST 请求。 */
export async function apiAgentPost<T>(
  path: string,
  body?: unknown
): Promise<T> {
  return agentRequest<T>('POST', path, body)
}

/** 发送 Agent 管理 PUT 请求。 */
export async function apiAgentPut<T>(path: string, body?: unknown): Promise<T> {
  return agentRequest<T>('PUT', path, body)
}

/** 发送 Agent 管理 DELETE 请求。 */
export async function apiAgentDelete<T>(path: string): Promise<T> {
  return agentRequest<T>('DELETE', path)
}

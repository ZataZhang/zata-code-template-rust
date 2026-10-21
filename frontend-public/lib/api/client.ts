import axios, { type AxiosResponse } from "axios"

/** Axios instance for the public frontend API with credentials and 401 handling. */
export const apiClient = axios.create({
  baseURL: "/api",
  headers: {
    "Content-Type": "application/json",
  },
  withCredentials: true,
})

apiClient.interceptors.response.use(
  (response) => response,
  (error) => {
    if (axios.isAxiosError(error) && error.response?.status === 401) {
      // /auth/* 的 401 是预期内的业务结果（密码错误、尚未登录），由调用方自己处理。
      // 若在这里整页跳转，登录失败时页面会被刷新，错误提示随之丢失。
      const requestUrl = error.config?.url ?? ""
      if (typeof window !== "undefined" && !requestUrl.startsWith("/auth/")) {
        window.location.href = "/login"
      }
    }
    return Promise.reject(error)
  }
)

/** 后端统一的错误响应体：`{"error": {"code": "...", "message": "..."}}`。 */
type ApiErrorBody = {
  error?: { code?: unknown; message?: unknown }
}

/** Error thrown for API requests, carrying the backend's machine-readable error code. */
export class ApiRequestError extends Error {
  /**
   * Error thrown for API requests.
   *
   * @param message - Human-readable message (from the backend, or the network error).
   * @param status - HTTP response status code, absent for network failures.
   * @param code - Machine-readable error code such as `invalid_credentials`; use it to pick a translated message.
   * @param retryAfterSeconds - Seconds to wait before retrying, set when the request was rate limited (429).
   */
  constructor(
    message: string,
    public status?: number,
    public code?: string,
    public retryAfterSeconds?: number
  ) {
    super(message)
    this.name = "ApiRequestError"
  }
}

/** 把 Axios 错误转成 `ApiRequestError`，其他错误原样返回。 */
function toApiRequestError(error: unknown): unknown {
  if (!axios.isAxiosError(error)) return error
  const body = error.response?.data as ApiErrorBody | undefined
  const code = typeof body?.error?.code === "string" ? body.error.code : undefined
  const message =
    typeof body?.error?.message === "string" ? body.error.message : error.message
  const retryAfterHeader = error.response?.headers?.["retry-after"]
  const retryAfterSeconds = Number.parseInt(String(retryAfterHeader ?? ""), 10)
  return new ApiRequestError(
    message,
    error.response?.status,
    code,
    Number.isFinite(retryAfterSeconds) ? retryAfterSeconds : undefined
  )
}

/** 等待请求完成并返回数据；失败时抛出 `ApiRequestError`。 */
async function unwrap<T>(request: Promise<AxiosResponse<T>>): Promise<T> {
  try {
    return (await request).data
  } catch (error) {
    throw toApiRequestError(error)
  }
}

/** Send a GET request and return the typed response data. */
export function apiGet<T>(url: string): Promise<T> {
  return unwrap(apiClient.get<T>(url))
}

/** Send a POST request with a JSON body and return the typed response data. */
export function apiPost<T>(url: string, data: unknown): Promise<T> {
  return unwrap(apiClient.post<T>(url, data))
}

/** Send a PUT request with a JSON body and return the typed response data. */
export function apiPut<T>(url: string, data: unknown): Promise<T> {
  return unwrap(apiClient.put<T>(url, data))
}

/** Send a DELETE request and throw on failure. */
export async function apiDelete(url: string): Promise<void> {
  await unwrap(apiClient.delete(url))
}

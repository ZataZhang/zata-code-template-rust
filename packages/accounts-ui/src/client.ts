/**
 * 认证 API 返回的普通用户会话。
 */
export type AccountSession = {
  id: number
  email: string
}

/**
 * 登录和注册需要的邮箱与密码。
 */
export type AccountCredentials = {
  email: string
  password: string
}

/**
 * 可供宿主根据错误码选择文案的认证错误。
 */
export class AccountsApiError extends Error {
  /**
   * 新建包含 HTTP 状态、错误码和等待时间的 API 错误。
   *
   * @param message - 服务端提供的兜底消息。
   * @param status - HTTP 状态码；网络错误时为空。
   * @param code - 服务端稳定的错误码。
   * @param retryAfterSeconds - 429 响应要求的等待秒数。
   */
  constructor(
    message: string,
    public readonly status?: number,
    public readonly code?: string,
    public readonly retryAfterSeconds?: number
  ) {
    super(message)
    this.name = "AccountsApiError"
  }
}

type ApiErrorEnvelope = {
  error?: { code?: unknown; message?: unknown }
}

type UserEnvelope = {
  user: AccountSession
}

/**
 * 账号 API 客户端设置。
 */
export type AccountsClientOptions = {
  /** 宿主同源代理的前缀，例如 `/api`。 */
  baseUrl: string
  /** 单次请求的最大等待毫秒数，默认 10 秒，上限 60 秒。 */
  timeoutMs?: number
}

/**
 * 创建与账号服务通信的普通用户客户端。
 *
 * 请求会携带同源会话 Cookie。管理员 API 与 Cookie 不属于此客户端。
 *
 * @param options - 宿主提供的账号 API 地址。
 * @returns 注册、登录、会话查询及退出方法。
 */
export function createAccountsClient(options: AccountsClientOptions) {
  const normalized_base_url = options.baseUrl.replace(/\/+$/, "")
  const timeout_ms = Math.min(60_000, Math.max(1, options.timeoutMs ?? 10_000))

  async function request<T>(path: string, body?: unknown): Promise<T> {
    const response = await fetch(`${normalized_base_url}${path}`, {
      method: body === undefined ? "GET" : "POST",
      credentials: "include",
      headers: body === undefined ? undefined : { "Content-Type": "application/json" },
      body: body === undefined ? undefined : JSON.stringify(body),
      signal: AbortSignal.timeout(timeout_ms),
    })

    if (!response.ok) {
      let parsed_body: ApiErrorEnvelope = {}
      try {
        parsed_body = (await response.json()) as ApiErrorEnvelope
      } catch {
        parsed_body = {}
      }
      const retry_after_seconds = Number.parseInt(
        response.headers.get("Retry-After") ?? "",
        10
      )
      const code = parsed_body.error?.code
      const message = parsed_body.error?.message
      throw new AccountsApiError(
        typeof message === "string" ? message : "Account request failed",
        response.status,
        typeof code === "string" ? code : undefined,
        Number.isFinite(retry_after_seconds) ? retry_after_seconds : undefined
      )
    }

    if (response.status === 204) return undefined as T
    return (await response.json()) as T
  }

  async function login(credentials: AccountCredentials): Promise<AccountSession> {
    const { user } = await request<UserEnvelope>("/auth/login", credentials)
    return user
  }

  async function register(credentials: AccountCredentials): Promise<AccountSession> {
    await request<UserEnvelope>("/auth/register", credentials)
    return login(credentials)
  }

  return {
    /** 登录并返回当前普通用户会话。 */
    login,
    /** 注册后立即登录并返回新会话。 */
    register,
    /** 查询当前普通用户会话。 */
    async getCurrentSession(): Promise<AccountSession> {
      const { user } = await request<UserEnvelope>("/auth/me")
      return user
    },
    /** 退出当前普通用户会话。 */
    async logout(): Promise<void> {
      await request<void>("/auth/logout", {})
    },
  }
}

/** 密码最少字符数，与账号服务的规则一致。 */
export const PASSWORD_MIN_CHARS = 8

/** 密码最大 UTF-8 字节数，与账号服务的规则一致。 */
export const PASSWORD_MAX_BYTES = 128

/** 密码长度校验失败原因。 */
export type AccountPasswordIssue = "too_short" | "too_long"

/**
 * 返回账号服务一致的密码长度校验结果。
 *
 * @param password - 用户提交的密码。
 * @returns 密码无效时对应的长度原因。
 */
export function getAccountPasswordIssue(
  password: string
): AccountPasswordIssue | undefined {
  if ([...password].length < PASSWORD_MIN_CHARS) return "too_short"
  if (new TextEncoder().encode(password).length > PASSWORD_MAX_BYTES) return "too_long"
  return undefined
}

/**
 * 检查密码是否符合账号服务长度规则。
 *
 * @param password - 用户提交的密码。
 * @returns 密码同时满足字符数下限和 UTF-8 字节数上限时为 `true`。
 */
export function isValidAccountPassword(password: string): boolean {
  return getAccountPasswordIssue(password) === undefined
}

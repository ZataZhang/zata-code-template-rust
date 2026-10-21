import { apiGet, apiPost } from "./client"

/** 密码最少字符数。与后端 `tpl-core` 的规则保持一致。 */
export const PASSWORD_MIN_CHARS = 8

/** 密码最大字节数（UTF-8）。与后端 `tpl-core` 的规则保持一致：超过会被拒绝。 */
export const PASSWORD_MAX_BYTES = 128

/** Authenticated user returned by the auth API. */
export type UserSession = {
  id: number
  email: string
}

/** 后端把用户包在 `user` 字段里返回：`{"user": {"id": 1, "email": "..."}}`。 */
type SessionEnvelope = {
  user: UserSession
}

/** Payload for user login. */
export type LoginCredentials = {
  email: string
  password: string
}

/** Payload for user registration. */
export type RegisterPayload = {
  email: string
  password: string
}

/** Log in and return the current user session. */
export async function login(credentials: LoginCredentials): Promise<UserSession> {
  const { user } = await apiPost<SessionEnvelope>("/auth/login", credentials)
  return user
}

/** Log out the current user. */
export async function logout(): Promise<void> {
  await apiPost<void>("/auth/logout", {})
}

/** Fetch the current user session, if logged in. */
export async function getCurrentSession(): Promise<UserSession> {
  const { user } = await apiGet<SessionEnvelope>("/auth/me")
  return user
}

/**
 * Register a new user and sign them in.
 *
 * 后端注册接口不会自动登录，所以注册成功后紧接着登录一次，让用户直接进入应用。
 *
 * @param payload - Email and password for the new account.
 * @returns The signed-in user session.
 */
export async function register(payload: RegisterPayload): Promise<UserSession> {
  await apiPost<SessionEnvelope>("/auth/register", payload)
  return login(payload)
}

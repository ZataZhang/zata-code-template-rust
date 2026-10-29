import {
  createAccountsClient,
  PASSWORD_MAX_BYTES,
  PASSWORD_MIN_CHARS,
  type AccountCredentials,
  type AccountSession,
} from "@tpl/accounts-ui"

const accounts_client = createAccountsClient({ baseUrl: "/api" })

/** 密码最少字符数，由共享账号模块提供。 */
export { PASSWORD_MAX_BYTES, PASSWORD_MIN_CHARS }

/** 当前普通用户会话。 */
export type UserSession = AccountSession

/** 登录凭据。 */
export type LoginCredentials = AccountCredentials

/** 注册信息。 */
export type RegisterPayload = AccountCredentials

/** 登录并返回当前用户会话。 */
export const login = accounts_client.login

/** 退出当前普通用户会话。 */
export const logout = accounts_client.logout

/** 查询当前普通用户会话。 */
export const getCurrentSession = accounts_client.getCurrentSession

/** 注册后立即登录并返回新用户会话。 */
export const register = accounts_client.register

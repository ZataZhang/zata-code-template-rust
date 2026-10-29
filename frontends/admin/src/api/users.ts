import { apiGet, apiPost } from './client'

export type PublicUserStatus = 'active' | 'disabled'

export type PublicUser = {
  id: string
  email: string
  status: PublicUserStatus
  /** 注册时间（UTC，ISO 8601）。 */
  created_at: string
}

export type PublicUserListResponse = {
  items: PublicUser[]
  total: number
  /** 服务端实际采用的页码（越界值已被规整）。 */
  page: number
  /** 服务端实际采用的每页条数。 */
  page_size: number
}

export type ListPublicUsersParams = {
  page?: number
  page_size?: number
  status?: PublicUserStatus
  keyword?: string
}

/** List public users with optional filtering and pagination. */
export async function listPublicUsers(
  params: ListPublicUsersParams = {}
): Promise<PublicUserListResponse> {
  return apiGet<PublicUserListResponse>('/admin/users', { params })
}

/** Enable a public user account. */
export async function enablePublicUser(userId: string): Promise<PublicUser> {
  return apiPost<PublicUser>(`/admin/users/${userId}/enable`, {})
}

/** Disable a public user account. */
export async function disablePublicUser(userId: string): Promise<PublicUser> {
  return apiPost<PublicUser>(`/admin/users/${userId}/disable`, {})
}

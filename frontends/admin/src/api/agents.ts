import {
  apiAgentDelete,
  apiAgentGet,
  apiAgentPost,
  apiAgentPut,
} from './client'

/**
 * Agent 管理 API 客户端。
 *
 * 所有请求走 `/agent-api` 同源前缀，由 Vite 代理或 nginx 注入服务 token；
 * 浏览器不持有 Agent 服务凭据。类型与 `services/agent/crates/api/src/dto.rs`
 * 及 core 序列化结构手写对齐，后端契约变更时需同步修改。
 */

type ListResponse<T> = {
  items: T[]
}

export type AdminIdentity = {
  admin_user_id: number
  display_name: string
  platform_admin: boolean
}

export type Organization = {
  id: string
  slug: string
  display_name: string
  created_at: string
}

export type OrganizationUnit = {
  id: string
  organization_id: string
  name: string
  created_at: string
}

export type OrganizationMembership = {
  organization_id: string
  user_id: number
  unit_id: string | null
  display_name: string | null
  created_at: string
}

export type AdminScope = {
  organization_id: string
  admin_user_id: number
  granted_by_admin_id: number
  created_at: string
}

type RuntimeOrigin = 'builtin' | 'external'
type RuntimeAdapterKind = 'sandbox' | 'a2a'
type RuntimeStatus = 'ready' | 'unavailable'

type RuntimeConfig = {
  allowed_tools: string[]
  max_steps: number
  a2a_protocol_version: string | null
}

export type RuntimeConnection = {
  id: string
  organization_id: string | null
  name: string
  display_name: string
  origin: RuntimeOrigin
  adapter_kind: RuntimeAdapterKind
  endpoint_url: string | null
  credential_alias: string | null
  status: RuntimeStatus
  status_detail: string | null
  config: RuntimeConfig
  created_at: string
  updated_at: string
}

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

/**
 * 规则主体：`kind` 区分组织、部门与单个用户。
 * 组织与部门使用 UUID 字符串；单个用户使用 accounts 用户 ID（JSON 数字）。
 */
export type AccessSubject =
  | { kind: 'organization'; id: string }
  | { kind: 'unit'; id: string }
  | { kind: 'user'; id: number }

export type AccessEffect = 'allow' | 'deny'

export type AccessRuleInput = {
  effect: AccessEffect
  subject: AccessSubject
}

export type AgentAccessRule = AccessRuleInput & {
  id: string
  agent_id: string
  created_at: string
}

export type EntryPolicy = {
  agent_id: string
  organization_id: string
  auto_visible: boolean
  priority: number
  updated_at: string
}

export type DelegationPolicy = {
  parent_agent_id: string
  child_agent_ids: string[]
  max_depth: number
  max_child_runs: number
}

export type AgentOverview = {
  agent: BusinessAgent
  access_rules: AgentAccessRule[]
  entry_policy: EntryPolicy | null
  delegation_policy: DelegationPolicy | null
  skill_ids: string[]
}

/** 读取当前管理员身份与平台管理员标记。 */
export async function getAgentAdminIdentity(): Promise<AdminIdentity> {
  return apiAgentGet<AdminIdentity>('/admin/api/me')
}

/** 列出当前管理员可见的组织。 */
export async function listOrganizations(): Promise<Organization[]> {
  const response = await apiAgentGet<ListResponse<Organization>>(
    '/admin/api/organizations'
  )
  return response.items
}

/** 创建组织；仅平台管理员可调用。 */
export async function createOrganization(input: {
  slug: string
  display_name: string
}): Promise<Organization> {
  return apiAgentPost<Organization>('/admin/api/organizations', input)
}

/** 列出组织部门。 */
export async function listUnits(
  organizationId: string
): Promise<OrganizationUnit[]> {
  const response = await apiAgentGet<ListResponse<OrganizationUnit>>(
    `/admin/api/organizations/${organizationId}/units`
  )
  return response.items
}

/** 创建部门。 */
export async function createUnit(
  organizationId: string,
  input: { name: string }
): Promise<OrganizationUnit> {
  return apiAgentPost<OrganizationUnit>(
    `/admin/api/organizations/${organizationId}/units`,
    input
  )
}

/** 删除部门。 */
export async function deleteUnit(
  organizationId: string,
  unitId: string
): Promise<void> {
  await apiAgentDelete<void>(
    `/admin/api/organizations/${organizationId}/units/${unitId}`
  )
}

/** 列出显式成员关系。 */
export async function listMemberships(
  organizationId: string
): Promise<OrganizationMembership[]> {
  const response = await apiAgentGet<ListResponse<OrganizationMembership>>(
    `/admin/api/organizations/${organizationId}/memberships`
  )
  return response.items
}

/** 新增或更新成员关系。 */
export async function upsertMembership(
  organizationId: string,
  input: { user_id: number; unit_id?: string | null; display_name?: string }
): Promise<OrganizationMembership> {
  return apiAgentPost<OrganizationMembership>(
    `/admin/api/organizations/${organizationId}/memberships`,
    input
  )
}

/** 移除成员关系。 */
export async function removeMembership(
  organizationId: string,
  userId: number
): Promise<void> {
  await apiAgentDelete<void>(
    `/admin/api/organizations/${organizationId}/memberships/${userId}`
  )
}

/** 列出组织管理员 scope。 */
export async function listAdminScopes(
  organizationId: string
): Promise<AdminScope[]> {
  const response = await apiAgentGet<ListResponse<AdminScope>>(
    `/admin/api/organizations/${organizationId}/admin-scopes`
  )
  return response.items
}

/** 授予管理员组织 scope。 */
export async function grantAdminScope(
  organizationId: string,
  adminUserId: number
): Promise<AdminScope> {
  return apiAgentPost<AdminScope>(
    `/admin/api/organizations/${organizationId}/admin-scopes`,
    { admin_user_id: adminUserId }
  )
}

/** 撤销管理员组织 scope。 */
export async function revokeAdminScope(
  organizationId: string,
  adminUserId: number
): Promise<void> {
  await apiAgentDelete<void>(
    `/admin/api/organizations/${organizationId}/admin-scopes/${adminUserId}`
  )
}

/** 列出组织可见的 Runtime（内置与外部）。 */
export async function listRuntimes(
  organizationId: string
): Promise<RuntimeConnection[]> {
  const response = await apiAgentGet<ListResponse<RuntimeConnection>>(
    `/admin/api/organizations/${organizationId}/runtimes`
  )
  return response.items
}

/** 登记外部 A2A Runtime。 */
export async function createExternalRuntime(
  organizationId: string,
  input: {
    name: string
    display_name: string
    endpoint_url: string
    credential_alias?: string
  }
): Promise<RuntimeConnection> {
  return apiAgentPost<RuntimeConnection>(
    `/admin/api/organizations/${organizationId}/runtimes`,
    input
  )
}

/** 通过重新读取 Agent Card 刷新外部 Runtime 状态。 */
export async function refreshRuntime(
  organizationId: string,
  runtimeId: string
): Promise<RuntimeConnection> {
  return apiAgentPost<RuntimeConnection>(
    `/admin/api/organizations/${organizationId}/runtimes/${runtimeId}/refresh`,
    {}
  )
}

/** 列出组织内 Business Agent。 */
export async function listAgents(
  organizationId: string
): Promise<BusinessAgent[]> {
  const response = await apiAgentGet<ListResponse<BusinessAgent>>(
    `/admin/api/organizations/${organizationId}/agents`
  )
  return response.items
}

/** 创建 Agent；Runtime 在此刻绑定且之后不可更换。 */
export async function createAgent(
  organizationId: string,
  input: {
    runtime_connection_id: string
    name: string
    display_name: string
    description?: string
    prompt: string
  }
): Promise<BusinessAgent> {
  return apiAgentPost<BusinessAgent>(
    `/admin/api/organizations/${organizationId}/agents`,
    input
  )
}

/** 更新 Agent descriptor；不含 Runtime binding。 */
export async function updateAgent(
  organizationId: string,
  agentId: string,
  input: {
    display_name: string
    description?: string
    prompt: string
    enabled: boolean
  }
): Promise<BusinessAgent> {
  return apiAgentPut<BusinessAgent>(
    `/admin/api/organizations/${organizationId}/agents/${agentId}`,
    input
  )
}

/** 读取 Agent 概览（规则、策略、Skill 授权）。 */
export async function getAgentOverview(
  organizationId: string,
  agentId: string
): Promise<AgentOverview> {
  return apiAgentGet<AgentOverview>(
    `/admin/api/organizations/${organizationId}/agents/${agentId}/overview`
  )
}

/** 全量替换 Agent 访问规则。 */
export async function replaceAccessRules(
  organizationId: string,
  agentId: string,
  rules: AccessRuleInput[]
): Promise<AgentAccessRule[]> {
  const response = await apiAgentPut<ListResponse<AgentAccessRule>>(
    `/admin/api/organizations/${organizationId}/agents/${agentId}/access-rules`,
    { rules }
  )
  return response.items
}

/** 新增或更新 Agent 的 Auto 入口策略。 */
export async function upsertEntryPolicy(
  organizationId: string,
  agentId: string,
  input: { auto_visible: boolean; priority: number }
): Promise<EntryPolicy> {
  return apiAgentPut<EntryPolicy>(
    `/admin/api/organizations/${organizationId}/agents/${agentId}/entry-policy`,
    input
  )
}

/** 新增或更新 Agent 委派策略。 */
export async function upsertDelegationPolicy(
  organizationId: string,
  agentId: string,
  input: {
    child_agent_ids: string[]
    max_depth: number
    max_child_runs: number
  }
): Promise<DelegationPolicy> {
  return apiAgentPut<DelegationPolicy>(
    `/admin/api/organizations/${organizationId}/agents/${agentId}/delegation-policy`,
    input
  )
}

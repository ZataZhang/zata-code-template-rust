import { useState } from 'react'
import { useQuery } from '@tanstack/react-query'
import { useTranslation } from 'react-i18next'
import {
  type Organization,
  getAgentAdminIdentity,
  listOrganizations,
} from '@/api/agents'
import { Label } from '@/components/ui/label'
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from '@/components/ui/select'

export const ORGANIZATIONS_QUERY_KEY = ['agent', 'organizations'] as const
const AGENT_ADMIN_QUERY_KEY = ['agent', 'admin-identity'] as const

/** Format an ISO timestamp for display. */
export function formatDateTime(value: string): string {
  const parsedDate = new Date(value)
  return Number.isNaN(parsedDate.getTime()) ? '-' : parsedDate.toLocaleString()
}

/** 加载当前管理员可见的组织列表。 */
export function useOrganizationsQuery() {
  return useQuery({
    queryKey: ORGANIZATIONS_QUERY_KEY,
    queryFn: listOrganizations,
  })
}

/** 加载当前管理员的身份与平台管理员标记。 */
export function useAgentAdminQuery() {
  return useQuery({
    queryKey: AGENT_ADMIN_QUERY_KEY,
    queryFn: getAgentAdminIdentity,
  })
}

/** 维护当前选中的组织；选择失效时回退到第一个可用组织。 */
export function useOrganizationSelection(organizations: Organization[]) {
  const [selectedId, setSelectedId] = useState('')

  const selectionIsValid = organizations.some(
    (organization) => organization.id === selectedId
  )
  const effectiveId = selectionIsValid
    ? selectedId
    : (organizations[0]?.id ?? '')

  return { selectedId: effectiveId, setSelectedId }
}

type OrganizationPickerProps = {
  organizations: Organization[]
  selectedId: string
  onChange: (organizationId: string) => void
}

/** 渲染组织选择器；无组织时显示提示文案。 */
export function OrganizationPicker({
  organizations,
  selectedId,
  onChange,
}: OrganizationPickerProps) {
  const { t } = useTranslation()

  if (organizations.length === 0) {
    return (
      <p className='text-sm text-muted-foreground' data-testid='agent-no-org'>
        {t('agentCommon.noOrganizations')}
      </p>
    )
  }

  return (
    <div className='flex items-center gap-2'>
      <Label htmlFor='agent-organization-select' className='text-sm'>
        {t('agentCommon.organization')}
      </Label>
      <Select value={selectedId} onValueChange={onChange}>
        <SelectTrigger
          id='agent-organization-select'
          className='h-9 w-64'
          data-testid='agent-organization-select'
        >
          <SelectValue />
        </SelectTrigger>
        <SelectContent>
          {organizations.map((organization) => (
            <SelectItem key={organization.id} value={organization.id}>
              {organization.display_name} ({organization.slug})
            </SelectItem>
          ))}
        </SelectContent>
      </Select>
    </div>
  )
}

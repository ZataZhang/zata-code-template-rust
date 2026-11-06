import { useState } from 'react'
import { useMutation, useQueryClient } from '@tanstack/react-query'
import { useTranslation } from 'react-i18next'
import { toast } from 'sonner'
import {
  type DelegationPolicy,
  type EntryPolicy,
  upsertDelegationPolicy,
  upsertEntryPolicy,
} from '@/api/agents'
import { Button } from '@/components/ui/button'
import { Input } from '@/components/ui/input'
import { Label } from '@/components/ui/label'
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from '@/components/ui/select'

type PolicyFormProps = {
  organizationId: string
  agentId: string
}

function overviewQueryKey(organizationId: string, agentId: string) {
  return ['agent', 'agent-overview', organizationId, agentId] as const
}

/** Auto 入口策略表单；priority 越小越优先。 */
export function EntryPolicyForm({
  organizationId,
  agentId,
  policy,
}: PolicyFormProps & { policy: EntryPolicy | null }) {
  const { t } = useTranslation()
  const queryClient = useQueryClient()
  const [autoVisible, setAutoVisible] = useState(
    policy?.auto_visible ? 'true' : 'false'
  )
  const [priority, setPriority] = useState(
    policy ? String(policy.priority) : '100'
  )

  const saveMutation = useMutation({
    mutationFn: () =>
      upsertEntryPolicy(organizationId, agentId, {
        auto_visible: autoVisible === 'true',
        priority: Number(priority),
      }),
    onSuccess: () => {
      toast.success(t('agentCommon.savedToast'))
      void queryClient.invalidateQueries({
        queryKey: overviewQueryKey(organizationId, agentId),
      })
    },
    onError: (error: Error) => toast.error(error.message),
  })

  return (
    <div className='flex flex-col gap-3'>
      <h3 className='text-sm font-medium'>{t('agentAgents.entryTitle')}</h3>
      <div className='flex flex-wrap items-end gap-2'>
        <div className='grid gap-1'>
          <Label>{t('agentAgents.autoVisibleLabel')}</Label>
          <Select value={autoVisible} onValueChange={setAutoVisible}>
            <SelectTrigger
              className='h-9 w-40'
              data-testid='admin-agent-entry-visible'
            >
              <SelectValue />
            </SelectTrigger>
            <SelectContent>
              <SelectItem value='true'>{t('agentCommon.yes')}</SelectItem>
              <SelectItem value='false'>{t('agentCommon.no')}</SelectItem>
            </SelectContent>
          </Select>
        </div>
        <div className='grid gap-1'>
          <Label htmlFor='agent-entry-priority'>
            {t('agentAgents.priorityLabel')}
          </Label>
          <Input
            id='agent-entry-priority'
            className='h-9 w-32'
            type='number'
            data-testid='admin-agent-entry-priority'
            value={priority}
            onChange={(event) => setPriority(event.target.value)}
          />
        </div>
        <Button
          type='button'
          size='sm'
          data-testid='admin-agent-entry-save'
          disabled={saveMutation.isPending}
          onClick={() => saveMutation.mutate()}
        >
          {t('agentCommon.save')}
        </Button>
      </div>
    </div>
  )
}

/** 委派策略表单；子 Agent 为逗号分隔的 UUID。 */
export function DelegationPolicyForm({
  organizationId,
  agentId,
  policy,
}: PolicyFormProps & { policy: DelegationPolicy | null }) {
  const { t } = useTranslation()
  const queryClient = useQueryClient()
  const [children, setChildren] = useState(
    policy ? policy.child_agent_ids.join(', ') : ''
  )
  const [maxDepth, setMaxDepth] = useState(
    policy ? String(policy.max_depth) : '0'
  )
  const [maxChildRuns, setMaxChildRuns] = useState(
    policy ? String(policy.max_child_runs) : '0'
  )

  const saveMutation = useMutation({
    mutationFn: () =>
      upsertDelegationPolicy(organizationId, agentId, {
        child_agent_ids: children
          .split(',')
          .map((value) => value.trim())
          .filter((value) => value !== ''),
        max_depth: Number(maxDepth),
        max_child_runs: Number(maxChildRuns),
      }),
    onSuccess: () => {
      toast.success(t('agentCommon.savedToast'))
      void queryClient.invalidateQueries({
        queryKey: overviewQueryKey(organizationId, agentId),
      })
    },
    onError: (error: Error) => toast.error(error.message),
  })

  return (
    <div className='flex flex-col gap-3'>
      <h3 className='text-sm font-medium'>
        {t('agentAgents.delegationTitle')}
      </h3>
      <div className='flex flex-wrap items-end gap-2'>
        <div className='grid min-w-64 flex-1 gap-1'>
          <Label htmlFor='agent-delegation-children'>
            {t('agentAgents.childrenLabel')}
          </Label>
          <Input
            id='agent-delegation-children'
            className='h-9'
            data-testid='admin-agent-delegation-children'
            placeholder={t('agentAgents.childrenPlaceholder')}
            value={children}
            onChange={(event) => setChildren(event.target.value)}
          />
        </div>
        <div className='grid gap-1'>
          <Label htmlFor='agent-delegation-depth'>
            {t('agentAgents.maxDepthLabel')}
          </Label>
          <Input
            id='agent-delegation-depth'
            className='h-9 w-24'
            type='number'
            min={0}
            max={3}
            data-testid='admin-agent-delegation-depth'
            value={maxDepth}
            onChange={(event) => setMaxDepth(event.target.value)}
          />
        </div>
        <div className='grid gap-1'>
          <Label htmlFor='agent-delegation-budget'>
            {t('agentAgents.maxChildRunsLabel')}
          </Label>
          <Input
            id='agent-delegation-budget'
            className='h-9 w-24'
            type='number'
            min={0}
            max={8}
            data-testid='admin-agent-delegation-budget'
            value={maxChildRuns}
            onChange={(event) => setMaxChildRuns(event.target.value)}
          />
        </div>
        <Button
          type='button'
          size='sm'
          data-testid='admin-agent-delegation-save'
          disabled={saveMutation.isPending}
          onClick={() => saveMutation.mutate()}
        >
          {t('agentCommon.save')}
        </Button>
      </div>
    </div>
  )
}

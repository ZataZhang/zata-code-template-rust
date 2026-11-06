import { useState } from 'react'
import { useMutation, useQueryClient } from '@tanstack/react-query'
import { useTranslation } from 'react-i18next'
import { toast } from 'sonner'
import {
  type AccessEffect,
  type AccessRuleInput,
  type AccessSubject,
  type AgentAccessRule,
  replaceAccessRules,
} from '@/api/agents'
import { Button } from '@/components/ui/button'
import { Input } from '@/components/ui/input'
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from '@/components/ui/select'
import {
  Table,
  TableBody,
  TableCell,
  TableHead,
  TableHeader,
  TableRow,
} from '@/components/ui/table'

type RuleKind = AccessSubject['kind']

type RuleRow = {
  effect: AccessEffect
  kind: RuleKind
  /** 组织规则忽略此值；用户规则填 accounts 用户 ID。 */
  subjectId: string
}

function toRuleRows(rules: AgentAccessRule[]): RuleRow[] {
  return rules.map((rule) => ({
    effect: rule.effect,
    kind: rule.subject.kind,
    subjectId:
      rule.subject.kind === 'organization' ? '' : String(rule.subject.id),
  }))
}

function toRuleInputs(
  rows: RuleRow[],
  organizationId: string
): AccessRuleInput[] {
  return rows.map((row) => {
    if (row.kind === 'organization') {
      return {
        effect: row.effect,
        subject: { kind: 'organization', id: organizationId },
      }
    }
    if (row.kind === 'unit') {
      return {
        effect: row.effect,
        subject: { kind: 'unit', id: row.subjectId.trim() },
      }
    }
    return {
      effect: row.effect,
      subject: { kind: 'user', id: Number(row.subjectId) },
    }
  })
}

type AccessRulesEditorProps = {
  organizationId: string
  agentId: string
  rules: AgentAccessRule[]
}

/** Agent 访问规则编辑器；deny 只能针对单个用户。 */
export function AccessRulesEditor({
  organizationId,
  agentId,
  rules,
}: AccessRulesEditorProps) {
  const { t } = useTranslation()
  const queryClient = useQueryClient()
  const [rows, setRows] = useState<RuleRow[]>(() => toRuleRows(rules))

  const saveMutation = useMutation({
    mutationFn: () =>
      replaceAccessRules(
        organizationId,
        agentId,
        toRuleInputs(rows, organizationId)
      ),
    onSuccess: () => {
      toast.success(t('agentCommon.savedToast'))
      void queryClient.invalidateQueries({
        queryKey: ['agent', 'agent-overview', organizationId, agentId],
      })
      void queryClient.invalidateQueries({
        queryKey: ['agent', 'access-rules', organizationId, agentId],
      })
    },
    onError: (error: Error) => toast.error(error.message),
  })

  /** 更新一行规则字段。 */
  function updateRow(index: number, patch: Partial<RuleRow>) {
    setRows((current) =>
      current.map((row, rowIndex) =>
        rowIndex === index ? { ...row, ...patch } : row
      )
    )
  }

  return (
    <div className='flex flex-col gap-3'>
      <div className='flex items-center justify-between'>
        <h3 className='text-sm font-medium'>{t('agentAgents.rulesTitle')}</h3>
        <div className='flex gap-2'>
          <Button
            type='button'
            variant='outline'
            size='sm'
            data-testid='admin-agent-rule-add'
            onClick={() =>
              setRows((current) => [
                ...current,
                { effect: 'allow', kind: 'user', subjectId: '' },
              ])
            }
          >
            {t('agentAgents.addRule')}
          </Button>
          <Button
            type='button'
            size='sm'
            data-testid='admin-agent-rules-save'
            disabled={saveMutation.isPending}
            onClick={() => saveMutation.mutate()}
          >
            {t('agentCommon.save')}
          </Button>
        </div>
      </div>
      <div className='overflow-hidden rounded-md border'>
        <Table>
          <TableHeader>
            <TableRow>
              <TableHead>{t('agentAgents.colEffect')}</TableHead>
              <TableHead>{t('agentAgents.colSubjectKind')}</TableHead>
              <TableHead>{t('agentAgents.colSubjectId')}</TableHead>
              <TableHead className='text-end'>
                {t('agentCommon.actions')}
              </TableHead>
            </TableRow>
          </TableHeader>
          <TableBody>
            {rows.length === 0 ? (
              <TableRow>
                <TableCell colSpan={4} className='h-16 text-center'>
                  {t('agentAgents.noRules')}
                </TableCell>
              </TableRow>
            ) : (
              rows.map((row, index) => (
                <TableRow key={`${row.kind}-${index}`}>
                  <TableCell>
                    <Select
                      value={row.effect}
                      onValueChange={(value) =>
                        updateRow(index, {
                          effect: value as AccessEffect,
                        })
                      }
                    >
                      <SelectTrigger
                        className='h-8 w-28'
                        data-testid='admin-agent-rule-effect'
                      >
                        <SelectValue />
                      </SelectTrigger>
                      <SelectContent>
                        <SelectItem value='allow'>
                          {t('agentAgents.effectAllow')}
                        </SelectItem>
                        <SelectItem value='deny'>
                          {t('agentAgents.effectDeny')}
                        </SelectItem>
                      </SelectContent>
                    </Select>
                  </TableCell>
                  <TableCell>
                    <Select
                      value={row.kind}
                      onValueChange={(value) =>
                        updateRow(index, {
                          kind: value as RuleKind,
                          subjectId: '',
                        })
                      }
                    >
                      <SelectTrigger
                        className='h-8 w-40'
                        data-testid='admin-agent-rule-kind'
                      >
                        <SelectValue />
                      </SelectTrigger>
                      <SelectContent>
                        <SelectItem value='organization'>
                          {t('agentAgents.subjectOrganization')}
                        </SelectItem>
                        <SelectItem value='unit'>
                          {t('agentAgents.subjectUnit')}
                        </SelectItem>
                        <SelectItem value='user'>
                          {t('agentAgents.subjectUser')}
                        </SelectItem>
                      </SelectContent>
                    </Select>
                  </TableCell>
                  <TableCell>
                    {row.kind === 'organization' ? (
                      <span className='text-xs text-muted-foreground'>
                        {organizationId}
                      </span>
                    ) : (
                      <Input
                        className='h-8 w-64'
                        data-testid='admin-agent-rule-subject'
                        type={row.kind === 'user' ? 'number' : 'text'}
                        placeholder={
                          row.kind === 'user'
                            ? t('agentOrganizations.userIdLabel')
                            : t('agentAgents.unitIdPlaceholder')
                        }
                        value={row.subjectId}
                        onChange={(event) =>
                          updateRow(index, { subjectId: event.target.value })
                        }
                      />
                    )}
                  </TableCell>
                  <TableCell className='text-end'>
                    <Button
                      type='button'
                      variant='outline'
                      size='sm'
                      onClick={() =>
                        setRows((current) =>
                          current.filter((_, rowIndex) => rowIndex !== index)
                        )
                      }
                    >
                      {t('agentCommon.remove')}
                    </Button>
                  </TableCell>
                </TableRow>
              ))
            )}
          </TableBody>
        </Table>
      </div>
    </div>
  )
}

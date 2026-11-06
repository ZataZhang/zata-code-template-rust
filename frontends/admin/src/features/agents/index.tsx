import { useState } from 'react'
import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query'
import { useTranslation } from 'react-i18next'
import { toast } from 'sonner'
import {
  type BusinessAgent,
  type RuntimeConnection,
  createAgent,
  getAgentOverview,
  listAgents,
  listRuntimes,
  updateAgent,
} from '@/api/agents'
import { Badge } from '@/components/ui/badge'
import { Button } from '@/components/ui/button'
import {
  Card,
  CardContent,
  CardDescription,
  CardHeader,
  CardTitle,
} from '@/components/ui/card'
import { Input } from '@/components/ui/input'
import { Label } from '@/components/ui/label'
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
import { Textarea } from '@/components/ui/textarea'
import { ConfigDrawer } from '@/components/config-drawer'
import { Header } from '@/components/layout/header'
import { Main } from '@/components/layout/main'
import { ProfileDropdown } from '@/components/profile-dropdown'
import { Search } from '@/components/search'
import { ThemeSwitch } from '@/components/theme-switch'
import {
  OrganizationPicker,
  formatDateTime,
  useOrganizationSelection,
  useOrganizationsQuery,
} from '@/features/agents/shared'
import { AccessRulesEditor } from './access-rules-editor'
import { DelegationPolicyForm, EntryPolicyForm } from './agent-policies'

function agentsQueryKey(organizationId: string) {
  return ['agent', 'agents', organizationId] as const
}

function runtimesQueryKey(organizationId: string) {
  return ['agent', 'runtimes', organizationId] as const
}

/** Business Agent 目录与治理策略管理。 */
export function AgentCatalog() {
  const { t } = useTranslation()
  const organizationsQuery = useOrganizationsQuery()
  const organizations = organizationsQuery.data ?? []
  const { selectedId, setSelectedId } = useOrganizationSelection(organizations)
  const [selectedAgentId, setSelectedAgentId] = useState('')

  const runtimesQuery = useQuery({
    queryKey: runtimesQueryKey(selectedId),
    queryFn: () => listRuntimes(selectedId),
    enabled: selectedId !== '',
  })

  return (
    <>
      <Header fixed>
        <Search
          className='me-auto'
          placeholder={t('commandMenu.placeholder')}
        />
        <ThemeSwitch />
        <ConfigDrawer />
        <ProfileDropdown />
      </Header>

      <Main className='flex flex-1 flex-col gap-4 sm:gap-6'>
        <div>
          <h2 className='text-2xl font-bold tracking-tight'>
            {t('agentAgents.title')}
          </h2>
          <p className='text-muted-foreground'>
            {t('agentAgents.description')}
          </p>
        </div>

        {organizationsQuery.isError && (
          <p className='text-sm text-destructive'>
            {t('agentCommon.loadFailed')}
          </p>
        )}

        <OrganizationPicker
          organizations={organizations}
          selectedId={selectedId}
          onChange={(organizationId) => {
            setSelectedId(organizationId)
            setSelectedAgentId('')
          }}
        />

        {selectedId !== '' && (
          <>
            <AgentsCard
              organizationId={selectedId}
              selectedAgentId={selectedAgentId}
              onSelect={setSelectedAgentId}
            />
            <CreateAgentCard
              organizationId={selectedId}
              runtimes={runtimesQuery.data ?? []}
              onCreated={(agent) => setSelectedAgentId(agent.id)}
            />
            {selectedAgentId !== '' && (
              <AgentDetail
                key={selectedAgentId}
                organizationId={selectedId}
                agentId={selectedAgentId}
              />
            )}
          </>
        )}
      </Main>
    </>
  )
}

function AgentsCard({
  organizationId,
  selectedAgentId,
  onSelect,
}: {
  organizationId: string
  selectedAgentId: string
  onSelect: (agentId: string) => void
}) {
  const { t } = useTranslation()

  const agentsQuery = useQuery({
    queryKey: agentsQueryKey(organizationId),
    queryFn: () => listAgents(organizationId),
  })

  const agents = agentsQuery.data ?? []

  return (
    <Card>
      <CardHeader>
        <CardTitle>{t('agentAgents.listTitle')}</CardTitle>
        <CardDescription>{t('agentAgents.listDescription')}</CardDescription>
      </CardHeader>
      <CardContent>
        <div className='overflow-hidden rounded-md border'>
          <Table>
            <TableHeader>
              <TableRow>
                <TableHead>{t('agentAgents.colName')}</TableHead>
                <TableHead>{t('agentAgents.colRuntime')}</TableHead>
                <TableHead>{t('agentAgents.colEnabled')}</TableHead>
                <TableHead>{t('agentAgents.colUpdatedAt')}</TableHead>
                <TableHead className='text-end'>
                  {t('agentCommon.actions')}
                </TableHead>
              </TableRow>
            </TableHeader>
            <TableBody>
              {agentsQuery.isLoading ? (
                <TableRow>
                  <TableCell colSpan={5} className='h-24 text-center'>
                    {t('agentCommon.loading')}
                  </TableCell>
                </TableRow>
              ) : agentsQuery.isError ? (
                <TableRow>
                  <TableCell
                    colSpan={5}
                    className='h-24 text-center text-destructive'
                  >
                    {t('agentCommon.loadFailed')}
                  </TableCell>
                </TableRow>
              ) : agents.length === 0 ? (
                <TableRow>
                  <TableCell colSpan={5} className='h-24 text-center'>
                    {t('agentAgents.empty')}
                  </TableCell>
                </TableRow>
              ) : (
                agents.map((agent) => (
                  <TableRow
                    key={agent.id}
                    data-testid='admin-agent-row'
                    data-selected={agent.id === selectedAgentId}
                    className={
                      agent.id === selectedAgentId ? 'bg-muted/50' : undefined
                    }
                  >
                    <TableCell>
                      <div className='font-medium'>{agent.display_name}</div>
                      <div className='text-xs text-muted-foreground'>
                        {agent.name}
                      </div>
                    </TableCell>
                    <TableCell className='font-mono text-xs'>
                      {agent.runtime_connection_id}
                    </TableCell>
                    <TableCell>
                      <Badge variant='outline'>
                        {agent.enabled
                          ? t('agentAgents.enabled')
                          : t('agentAgents.disabled')}
                      </Badge>
                    </TableCell>
                    <TableCell>{formatDateTime(agent.updated_at)}</TableCell>
                    <TableCell className='text-end'>
                      <Button
                        variant='outline'
                        size='sm'
                        data-testid='admin-agent-select'
                        onClick={() => onSelect(agent.id)}
                      >
                        {t('agentAgents.manage')}
                      </Button>
                    </TableCell>
                  </TableRow>
                ))
              )}
            </TableBody>
          </Table>
        </div>
      </CardContent>
    </Card>
  )
}

function CreateAgentCard({
  organizationId,
  runtimes,
  onCreated,
}: {
  organizationId: string
  runtimes: RuntimeConnection[]
  onCreated: (agent: BusinessAgent) => void
}) {
  const { t } = useTranslation()
  const queryClient = useQueryClient()
  const readyRuntimes = runtimes.filter((runtime) => runtime.status === 'ready')
  const [runtimeId, setRuntimeId] = useState('')
  const [name, setName] = useState('')
  const [displayName, setDisplayName] = useState('')
  const [description, setDescription] = useState('')
  const [prompt, setPrompt] = useState('')

  const createMutation = useMutation({
    mutationFn: () =>
      createAgent(organizationId, {
        runtime_connection_id: runtimeId,
        name,
        display_name: displayName,
        description,
        prompt,
      }),
    onSuccess: (agent) => {
      toast.success(t('agentAgents.createdToast', { name: agent.display_name }))
      setName('')
      setDisplayName('')
      setDescription('')
      setPrompt('')
      onCreated(agent)
      void queryClient.invalidateQueries({
        queryKey: agentsQueryKey(organizationId),
      })
    },
    onError: (error: Error) => toast.error(error.message),
  })

  return (
    <Card>
      <CardHeader>
        <CardTitle>{t('agentAgents.createTitle')}</CardTitle>
        <CardDescription>{t('agentAgents.createDescription')}</CardDescription>
      </CardHeader>
      <CardContent>
        <form
          className='grid gap-3'
          data-testid='admin-agent-create-form'
          onSubmit={(event) => {
            event.preventDefault()
            createMutation.mutate()
          }}
        >
          <div className='grid gap-1'>
            <Label>{t('agentAgents.runtimeLabel')}</Label>
            <Select value={runtimeId} onValueChange={setRuntimeId}>
              <SelectTrigger
                className='h-9 w-72'
                data-testid='admin-agent-runtime-select'
              >
                <SelectValue
                  placeholder={t('agentAgents.runtimePlaceholder')}
                />
              </SelectTrigger>
              <SelectContent>
                {readyRuntimes.map((runtime) => (
                  <SelectItem key={runtime.id} value={runtime.id}>
                    {runtime.display_name} ({runtime.adapter_kind})
                  </SelectItem>
                ))}
              </SelectContent>
            </Select>
            <p className='text-xs text-muted-foreground'>
              {t('agentAgents.runtimeHint')}
            </p>
          </div>
          <div className='flex flex-wrap gap-3'>
            <div className='grid gap-1'>
              <Label htmlFor='agent-name'>{t('agentAgents.nameLabel')}</Label>
              <Input
                id='agent-name'
                className='h-9 w-48'
                data-testid='admin-agent-name-input'
                value={name}
                onChange={(event) => setName(event.target.value)}
              />
            </div>
            <div className='grid gap-1'>
              <Label htmlFor='agent-display-name'>
                {t('agentAgents.displayNameLabel')}
              </Label>
              <Input
                id='agent-display-name'
                className='h-9 w-64'
                data-testid='admin-agent-display-input'
                value={displayName}
                onChange={(event) => setDisplayName(event.target.value)}
              />
            </div>
          </div>
          <div className='grid gap-1'>
            <Label htmlFor='agent-description'>
              {t('agentAgents.descriptionLabel')}
            </Label>
            <Input
              id='agent-description'
              className='h-9'
              data-testid='admin-agent-description-input'
              value={description}
              onChange={(event) => setDescription(event.target.value)}
            />
          </div>
          <div className='grid gap-1'>
            <Label htmlFor='agent-prompt'>{t('agentAgents.promptLabel')}</Label>
            <Textarea
              id='agent-prompt'
              rows={4}
              data-testid='admin-agent-prompt-input'
              value={prompt}
              onChange={(event) => setPrompt(event.target.value)}
            />
          </div>
          <div>
            <Button
              type='submit'
              size='sm'
              data-testid='admin-agent-create-submit'
              disabled={
                createMutation.isPending ||
                runtimeId === '' ||
                name === '' ||
                displayName === '' ||
                prompt === ''
              }
            >
              {t('agentCommon.create')}
            </Button>
          </div>
        </form>
      </CardContent>
    </Card>
  )
}

function AgentDetail({
  organizationId,
  agentId,
}: {
  organizationId: string
  agentId: string
}) {
  const { t } = useTranslation()

  const overviewQuery = useQuery({
    queryKey: ['agent', 'agent-overview', organizationId, agentId],
    queryFn: () => getAgentOverview(organizationId, agentId),
  })

  const overview = overviewQuery.data

  return (
    <Card data-testid='admin-agent-detail'>
      <CardHeader>
        <CardTitle>{t('agentAgents.detailTitle')}</CardTitle>
        <CardDescription>{t('agentAgents.detailDescription')}</CardDescription>
      </CardHeader>
      <CardContent className='flex flex-col gap-6'>
        {overviewQuery.isLoading ? (
          <p className='text-sm text-muted-foreground'>
            {t('agentCommon.loading')}
          </p>
        ) : overviewQuery.isError || !overview ? (
          <p className='text-sm text-destructive'>
            {t('agentCommon.loadFailed')}
          </p>
        ) : (
          <>
            <DescriptorForm
              organizationId={organizationId}
              agent={overview.agent}
            />
            <AccessRulesEditor
              organizationId={organizationId}
              agentId={agentId}
              rules={overview.access_rules}
            />
            <EntryPolicyForm
              organizationId={organizationId}
              agentId={agentId}
              policy={overview.entry_policy}
            />
            <DelegationPolicyForm
              organizationId={organizationId}
              agentId={agentId}
              policy={overview.delegation_policy}
            />
          </>
        )}
      </CardContent>
    </Card>
  )
}

function DescriptorForm({
  organizationId,
  agent,
}: {
  organizationId: string
  agent: BusinessAgent
}) {
  const { t } = useTranslation()
  const queryClient = useQueryClient()
  const [displayName, setDisplayName] = useState(agent.display_name)
  const [description, setDescription] = useState(agent.description)
  const [prompt, setPrompt] = useState(agent.prompt)
  const [enabled, setEnabled] = useState(agent.enabled ? 'true' : 'false')

  const saveMutation = useMutation({
    mutationFn: () =>
      updateAgent(organizationId, agent.id, {
        display_name: displayName,
        description,
        prompt,
        enabled: enabled === 'true',
      }),
    onSuccess: () => {
      toast.success(t('agentCommon.savedToast'))
      void queryClient.invalidateQueries({
        queryKey: agentsQueryKey(organizationId),
      })
      void queryClient.invalidateQueries({
        queryKey: ['agent', 'agent-overview', organizationId, agent.id],
      })
    },
    onError: (error: Error) => toast.error(error.message),
  })

  return (
    <div className='flex flex-col gap-3'>
      <h3 className='text-sm font-medium'>
        {t('agentAgents.descriptorTitle')}
      </h3>
      <div className='flex flex-wrap items-end gap-3'>
        <div className='grid gap-1'>
          <Label htmlFor='agent-detail-display-name'>
            {t('agentAgents.displayNameLabel')}
          </Label>
          <Input
            id='agent-detail-display-name'
            className='h-9 w-64'
            data-testid='admin-agent-detail-display'
            value={displayName}
            onChange={(event) => setDisplayName(event.target.value)}
          />
        </div>
        <div className='grid gap-1'>
          <Label>{t('agentAgents.colEnabled')}</Label>
          <Select value={enabled} onValueChange={setEnabled}>
            <SelectTrigger
              className='h-9 w-32'
              data-testid='admin-agent-enabled'
            >
              <SelectValue />
            </SelectTrigger>
            <SelectContent>
              <SelectItem value='true'>{t('agentAgents.enabled')}</SelectItem>
              <SelectItem value='false'>{t('agentAgents.disabled')}</SelectItem>
            </SelectContent>
          </Select>
        </div>
      </div>
      <div className='grid gap-1'>
        <Label htmlFor='agent-detail-description'>
          {t('agentAgents.descriptionLabel')}
        </Label>
        <Input
          id='agent-detail-description'
          className='h-9'
          data-testid='admin-agent-detail-description'
          value={description}
          onChange={(event) => setDescription(event.target.value)}
        />
      </div>
      <div className='grid gap-1'>
        <Label htmlFor='agent-detail-prompt'>
          {t('agentAgents.promptLabel')}
        </Label>
        <Textarea
          id='agent-detail-prompt'
          rows={4}
          data-testid='admin-agent-detail-prompt'
          value={prompt}
          onChange={(event) => setPrompt(event.target.value)}
        />
      </div>
      <div>
        <Button
          type='button'
          size='sm'
          data-testid='admin-agent-detail-save'
          disabled={saveMutation.isPending}
          onClick={() => saveMutation.mutate()}
        >
          {t('agentCommon.save')}
        </Button>
      </div>
    </div>
  )
}

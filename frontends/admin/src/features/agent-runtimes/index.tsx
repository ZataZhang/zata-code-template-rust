import { useState } from 'react'
import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query'
import { useTranslation } from 'react-i18next'
import { toast } from 'sonner'
import {
  createExternalRuntime,
  listRuntimes,
  refreshRuntime,
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
  Table,
  TableBody,
  TableCell,
  TableHead,
  TableHeader,
  TableRow,
} from '@/components/ui/table'
import { ConfigDrawer } from '@/components/config-drawer'
import { Header } from '@/components/layout/header'
import { Main } from '@/components/layout/main'
import { ProfileDropdown } from '@/components/profile-dropdown'
import { Search } from '@/components/search'
import { ThemeSwitch } from '@/components/theme-switch'
import {
  OrganizationPicker,
  useOrganizationSelection,
  useOrganizationsQuery,
} from '@/features/agents/shared'

function runtimesQueryKey(organizationId: string) {
  return ['agent', 'runtimes', organizationId] as const
}

/** Runtime 连接管理：内置只读、外部 A2A 可登记与刷新。 */
export function AgentRuntimes() {
  const { t } = useTranslation()
  const organizationsQuery = useOrganizationsQuery()
  const organizations = organizationsQuery.data ?? []
  const { selectedId, setSelectedId } = useOrganizationSelection(organizations)

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
            {t('agentRuntimes.title')}
          </h2>
          <p className='text-muted-foreground'>
            {t('agentRuntimes.description')}
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
          onChange={setSelectedId}
        />

        {selectedId !== '' && (
          <>
            <RuntimesCard organizationId={selectedId} />
            <CreateRuntimeCard organizationId={selectedId} />
          </>
        )}
      </Main>
    </>
  )
}

function RuntimesCard({ organizationId }: { organizationId: string }) {
  const { t } = useTranslation()
  const queryClient = useQueryClient()

  const runtimesQuery = useQuery({
    queryKey: runtimesQueryKey(organizationId),
    queryFn: () => listRuntimes(organizationId),
  })

  const refreshMutation = useMutation({
    mutationFn: (runtimeId: string) =>
      refreshRuntime(organizationId, runtimeId),
    onSuccess: () => {
      void queryClient.invalidateQueries({
        queryKey: runtimesQueryKey(organizationId),
      })
    },
    onError: (error: Error) => toast.error(error.message),
  })

  const runtimes = runtimesQuery.data ?? []

  return (
    <Card>
      <CardHeader>
        <CardTitle>{t('agentRuntimes.listTitle')}</CardTitle>
        <CardDescription>{t('agentRuntimes.listDescription')}</CardDescription>
      </CardHeader>
      <CardContent>
        <div className='overflow-hidden rounded-md border'>
          <Table>
            <TableHeader>
              <TableRow>
                <TableHead>{t('agentRuntimes.colName')}</TableHead>
                <TableHead>{t('agentRuntimes.colOrigin')}</TableHead>
                <TableHead>{t('agentRuntimes.colAdapter')}</TableHead>
                <TableHead>{t('agentRuntimes.colStatus')}</TableHead>
                <TableHead>{t('agentRuntimes.colEndpoint')}</TableHead>
                <TableHead className='text-end'>
                  {t('agentCommon.actions')}
                </TableHead>
              </TableRow>
            </TableHeader>
            <TableBody>
              {runtimesQuery.isLoading ? (
                <TableRow>
                  <TableCell colSpan={6} className='h-24 text-center'>
                    {t('agentCommon.loading')}
                  </TableCell>
                </TableRow>
              ) : runtimesQuery.isError ? (
                <TableRow>
                  <TableCell
                    colSpan={6}
                    className='h-24 text-center text-destructive'
                  >
                    {t('agentCommon.loadFailed')}
                  </TableCell>
                </TableRow>
              ) : runtimes.length === 0 ? (
                <TableRow>
                  <TableCell colSpan={6} className='h-24 text-center'>
                    {t('agentRuntimes.empty')}
                  </TableCell>
                </TableRow>
              ) : (
                runtimes.map((runtime) => (
                  <TableRow key={runtime.id} data-testid='admin-runtime-row'>
                    <TableCell>
                      <div className='font-medium'>{runtime.display_name}</div>
                      <div className='text-xs text-muted-foreground'>
                        {runtime.name}
                      </div>
                    </TableCell>
                    <TableCell>
                      <Badge variant='outline'>
                        {t(`agentRuntimes.origin.${runtime.origin}`)}
                      </Badge>
                    </TableCell>
                    <TableCell>{runtime.adapter_kind}</TableCell>
                    <TableCell>
                      <Badge
                        variant='outline'
                        className={
                          runtime.status === 'ready'
                            ? 'border-teal-200 bg-teal-100/30 text-teal-900 dark:text-teal-200'
                            : 'border-destructive/20 bg-destructive/10 text-destructive'
                        }
                      >
                        {t(`agentRuntimes.status.${runtime.status}`)}
                      </Badge>
                      {runtime.status_detail && (
                        <div className='mt-1 text-xs text-muted-foreground'>
                          {runtime.status_detail}
                        </div>
                      )}
                    </TableCell>
                    <TableCell className='max-w-64 truncate text-xs'>
                      {runtime.endpoint_url ?? '-'}
                    </TableCell>
                    <TableCell className='text-end'>
                      {runtime.origin === 'external' && (
                        <Button
                          variant='outline'
                          size='sm'
                          data-testid='admin-runtime-refresh'
                          disabled={refreshMutation.isPending}
                          onClick={() => refreshMutation.mutate(runtime.id)}
                        >
                          {t('agentRuntimes.refresh')}
                        </Button>
                      )}
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

function CreateRuntimeCard({ organizationId }: { organizationId: string }) {
  const { t } = useTranslation()
  const queryClient = useQueryClient()
  const [name, setName] = useState('')
  const [displayName, setDisplayName] = useState('')
  const [endpointUrl, setEndpointUrl] = useState('')
  const [credentialAlias, setCredentialAlias] = useState('')

  const createMutation = useMutation({
    mutationFn: () =>
      createExternalRuntime(organizationId, {
        name,
        display_name: displayName,
        endpoint_url: endpointUrl,
        credential_alias: credentialAlias === '' ? undefined : credentialAlias,
      }),
    onSuccess: () => {
      setName('')
      setDisplayName('')
      setEndpointUrl('')
      setCredentialAlias('')
      void queryClient.invalidateQueries({
        queryKey: runtimesQueryKey(organizationId),
      })
    },
    onError: (error: Error) => toast.error(error.message),
  })

  return (
    <Card>
      <CardHeader>
        <CardTitle>{t('agentRuntimes.createTitle')}</CardTitle>
        <CardDescription>
          {t('agentRuntimes.createDescription')}
        </CardDescription>
      </CardHeader>
      <CardContent>
        <form
          className='grid gap-3 sm:grid-cols-2 lg:grid-cols-4'
          data-testid='admin-runtime-create-form'
          onSubmit={(event) => {
            event.preventDefault()
            createMutation.mutate()
          }}
        >
          <div className='grid gap-1'>
            <Label htmlFor='runtime-name'>{t('agentRuntimes.nameLabel')}</Label>
            <Input
              id='runtime-name'
              className='h-9'
              data-testid='admin-runtime-name-input'
              value={name}
              onChange={(event) => setName(event.target.value)}
            />
          </div>
          <div className='grid gap-1'>
            <Label htmlFor='runtime-display-name'>
              {t('agentRuntimes.displayNameLabel')}
            </Label>
            <Input
              id='runtime-display-name'
              className='h-9'
              data-testid='admin-runtime-display-input'
              value={displayName}
              onChange={(event) => setDisplayName(event.target.value)}
            />
          </div>
          <div className='grid gap-1'>
            <Label htmlFor='runtime-endpoint'>
              {t('agentRuntimes.endpointLabel')}
            </Label>
            <Input
              id='runtime-endpoint'
              className='h-9'
              data-testid='admin-runtime-endpoint-input'
              placeholder='https://agent.example.com'
              value={endpointUrl}
              onChange={(event) => setEndpointUrl(event.target.value)}
            />
          </div>
          <div className='grid gap-1'>
            <Label htmlFor='runtime-credential'>
              {t('agentRuntimes.credentialLabel')}
            </Label>
            <Input
              id='runtime-credential'
              className='h-9'
              data-testid='admin-runtime-credential-input'
              placeholder={t('agentRuntimes.credentialPlaceholder')}
              value={credentialAlias}
              onChange={(event) => setCredentialAlias(event.target.value)}
            />
          </div>
          <div className='sm:col-span-2 lg:col-span-4'>
            <Button
              type='submit'
              size='sm'
              data-testid='admin-runtime-create-submit'
              disabled={
                createMutation.isPending ||
                name === '' ||
                displayName === '' ||
                endpointUrl === ''
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

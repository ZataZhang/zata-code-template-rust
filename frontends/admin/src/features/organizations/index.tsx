import { useState } from 'react'
import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query'
import { useTranslation } from 'react-i18next'
import { toast } from 'sonner'
import {
  type OrganizationUnit,
  createOrganization,
  createUnit,
  deleteUnit,
  grantAdminScope,
  listAdminScopes,
  listMemberships,
  listUnits,
  removeMembership,
  revokeAdminScope,
  upsertMembership,
} from '@/api/agents'
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
import { ConfigDrawer } from '@/components/config-drawer'
import { Header } from '@/components/layout/header'
import { Main } from '@/components/layout/main'
import { ProfileDropdown } from '@/components/profile-dropdown'
import { Search } from '@/components/search'
import { ThemeSwitch } from '@/components/theme-switch'
import {
  ORGANIZATIONS_QUERY_KEY,
  OrganizationPicker,
  formatDateTime,
  useAgentAdminQuery,
  useOrganizationSelection,
  useOrganizationsQuery,
} from '@/features/agents/shared'

const NO_UNIT = 'none'

function unitsQueryKey(organizationId: string) {
  return ['agent', 'units', organizationId] as const
}

function membershipsQueryKey(organizationId: string) {
  return ['agent', 'memberships', organizationId] as const
}

function adminScopesQueryKey(organizationId: string) {
  return ['agent', 'admin-scopes', organizationId] as const
}

/** 组织、部门、成员关系与管理员 scope 管理。 */
export function AgentOrganizations() {
  const { t } = useTranslation()
  const organizationsQuery = useOrganizationsQuery()
  const adminQuery = useAgentAdminQuery()
  const organizations = organizationsQuery.data ?? []
  const { selectedId, setSelectedId } = useOrganizationSelection(organizations)

  const unitsQuery = useQuery({
    queryKey: unitsQueryKey(selectedId),
    queryFn: () => listUnits(selectedId),
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
            {t('agentOrganizations.title')}
          </h2>
          <p className='text-muted-foreground'>
            {t('agentOrganizations.description')}
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

        {adminQuery.data?.platform_admin && (
          <CreateOrganizationCard onCreated={setSelectedId} />
        )}

        {selectedId !== '' && (
          <>
            <UnitsCard
              organizationId={selectedId}
              units={unitsQuery.data ?? []}
              loading={unitsQuery.isLoading}
            />
            <MembershipsCard
              organizationId={selectedId}
              units={unitsQuery.data ?? []}
            />
            <AdminScopesCard organizationId={selectedId} />
          </>
        )}
      </Main>
    </>
  )
}

function CreateOrganizationCard({
  onCreated,
}: {
  onCreated: (organizationId: string) => void
}) {
  const { t } = useTranslation()
  const queryClient = useQueryClient()
  const [slug, setSlug] = useState('')
  const [displayName, setDisplayName] = useState('')

  const createMutation = useMutation({
    mutationFn: () => createOrganization({ slug, display_name: displayName }),
    onSuccess: (organization) => {
      toast.success(
        t('agentOrganizations.createdToast', {
          name: organization.display_name,
        })
      )
      setSlug('')
      setDisplayName('')
      onCreated(organization.id)
      void queryClient.invalidateQueries({ queryKey: ORGANIZATIONS_QUERY_KEY })
    },
    onError: (error: Error) => toast.error(error.message),
  })

  return (
    <Card>
      <CardHeader>
        <CardTitle>{t('agentOrganizations.createTitle')}</CardTitle>
        <CardDescription>
          {t('agentOrganizations.createDescription')}
        </CardDescription>
      </CardHeader>
      <CardContent>
        <form
          className='flex flex-wrap items-end gap-2'
          data-testid='admin-org-create-form'
          onSubmit={(event) => {
            event.preventDefault()
            createMutation.mutate()
          }}
        >
          <div className='grid gap-1'>
            <Label htmlFor='agent-org-slug'>
              {t('agentOrganizations.slugLabel')}
            </Label>
            <Input
              id='agent-org-slug'
              className='h-9 w-48'
              data-testid='admin-org-slug-input'
              value={slug}
              placeholder='acme-01'
              onChange={(event) => setSlug(event.target.value)}
            />
          </div>
          <div className='grid gap-1'>
            <Label htmlFor='agent-org-name'>
              {t('agentOrganizations.displayNameLabel')}
            </Label>
            <Input
              id='agent-org-name'
              className='h-9 w-64'
              data-testid='admin-org-display-input'
              value={displayName}
              onChange={(event) => setDisplayName(event.target.value)}
            />
          </div>
          <Button
            type='submit'
            size='sm'
            data-testid='admin-org-create-submit'
            disabled={
              createMutation.isPending || slug === '' || displayName === ''
            }
          >
            {t('agentCommon.create')}
          </Button>
        </form>
      </CardContent>
    </Card>
  )
}

function UnitsCard({
  organizationId,
  units,
  loading,
}: {
  organizationId: string
  units: OrganizationUnit[]
  loading: boolean
}) {
  const { t } = useTranslation()
  const queryClient = useQueryClient()
  const [name, setName] = useState('')

  const createMutation = useMutation({
    mutationFn: () => createUnit(organizationId, { name }),
    onSuccess: () => {
      setName('')
      void queryClient.invalidateQueries({
        queryKey: unitsQueryKey(organizationId),
      })
      void queryClient.invalidateQueries({
        queryKey: membershipsQueryKey(organizationId),
      })
    },
    onError: (error: Error) => toast.error(error.message),
  })

  const deleteMutation = useMutation({
    mutationFn: (unitId: string) => deleteUnit(organizationId, unitId),
    onSuccess: () => {
      void queryClient.invalidateQueries({
        queryKey: unitsQueryKey(organizationId),
      })
      void queryClient.invalidateQueries({
        queryKey: membershipsQueryKey(organizationId),
      })
    },
    onError: (error: Error) => toast.error(error.message),
  })

  return (
    <Card>
      <CardHeader>
        <CardTitle>{t('agentOrganizations.unitsTitle')}</CardTitle>
        <CardDescription>
          {t('agentOrganizations.unitsDescription')}
        </CardDescription>
      </CardHeader>
      <CardContent className='flex flex-col gap-3'>
        <form
          className='flex items-end gap-2'
          onSubmit={(event) => {
            event.preventDefault()
            createMutation.mutate()
          }}
        >
          <div className='grid gap-1'>
            <Label htmlFor='agent-unit-name'>
              {t('agentOrganizations.unitNameLabel')}
            </Label>
            <Input
              id='agent-unit-name'
              className='h-9 w-64'
              data-testid='admin-org-unit-name-input'
              value={name}
              onChange={(event) => setName(event.target.value)}
            />
          </div>
          <Button
            type='submit'
            size='sm'
            data-testid='admin-org-unit-create-submit'
            disabled={createMutation.isPending || name === ''}
          >
            {t('agentCommon.add')}
          </Button>
        </form>
        <div className='overflow-hidden rounded-md border'>
          <Table>
            <TableHeader>
              <TableRow>
                <TableHead>{t('agentOrganizations.colUnit')}</TableHead>
                <TableHead>{t('agentOrganizations.colCreatedAt')}</TableHead>
                <TableHead className='text-end'>
                  {t('agentCommon.actions')}
                </TableHead>
              </TableRow>
            </TableHeader>
            <TableBody>
              {loading ? (
                <TableRow>
                  <TableCell colSpan={3} className='h-20 text-center'>
                    {t('agentCommon.loading')}
                  </TableCell>
                </TableRow>
              ) : units.length === 0 ? (
                <TableRow>
                  <TableCell colSpan={3} className='h-20 text-center'>
                    {t('agentOrganizations.noUnits')}
                  </TableCell>
                </TableRow>
              ) : (
                units.map((unit) => (
                  <TableRow key={unit.id} data-testid='admin-org-unit-row'>
                    <TableCell>{unit.name}</TableCell>
                    <TableCell>{formatDateTime(unit.created_at)}</TableCell>
                    <TableCell className='text-end'>
                      <Button
                        variant='outline'
                        size='sm'
                        data-testid='admin-org-unit-delete'
                        disabled={deleteMutation.isPending}
                        onClick={() => deleteMutation.mutate(unit.id)}
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
      </CardContent>
    </Card>
  )
}

function MembershipsCard({
  organizationId,
  units,
}: {
  organizationId: string
  units: OrganizationUnit[]
}) {
  const { t } = useTranslation()
  const queryClient = useQueryClient()
  const [userId, setUserId] = useState('')
  const [unitId, setUnitId] = useState(NO_UNIT)
  const [displayName, setDisplayName] = useState('')

  const membershipsQuery = useQuery({
    queryKey: membershipsQueryKey(organizationId),
    queryFn: () => listMemberships(organizationId),
  })

  const upsertMutation = useMutation({
    mutationFn: () =>
      upsertMembership(organizationId, {
        user_id: Number(userId),
        unit_id: unitId === NO_UNIT ? null : unitId,
        display_name: displayName === '' ? undefined : displayName,
      }),
    onSuccess: () => {
      setUserId('')
      setDisplayName('')
      void queryClient.invalidateQueries({
        queryKey: membershipsQueryKey(organizationId),
      })
    },
    onError: (error: Error) => toast.error(error.message),
  })

  const removeMutation = useMutation({
    mutationFn: (targetUserId: number) =>
      removeMembership(organizationId, targetUserId),
    onSuccess: () => {
      void queryClient.invalidateQueries({
        queryKey: membershipsQueryKey(organizationId),
      })
    },
    onError: (error: Error) => toast.error(error.message),
  })

  const memberships = membershipsQuery.data ?? []

  return (
    <Card>
      <CardHeader>
        <CardTitle>{t('agentOrganizations.membershipsTitle')}</CardTitle>
        <CardDescription>
          {t('agentOrganizations.membershipsDescription')}
        </CardDescription>
      </CardHeader>
      <CardContent className='flex flex-col gap-3'>
        <form
          className='flex flex-wrap items-end gap-2'
          onSubmit={(event) => {
            event.preventDefault()
            if (userId !== '') upsertMutation.mutate()
          }}
        >
          <div className='grid gap-1'>
            <Label htmlFor='agent-membership-user'>
              {t('agentOrganizations.userIdLabel')}
            </Label>
            <Input
              id='agent-membership-user'
              className='h-9 w-32'
              type='number'
              min={1}
              data-testid='admin-org-membership-user-input'
              value={userId}
              onChange={(event) => setUserId(event.target.value)}
            />
          </div>
          <div className='grid gap-1'>
            <Label>{t('agentOrganizations.unitLabel')}</Label>
            <Select value={unitId} onValueChange={setUnitId}>
              <SelectTrigger
                className='h-9 w-48'
                data-testid='admin-org-membership-unit-select'
              >
                <SelectValue />
              </SelectTrigger>
              <SelectContent>
                <SelectItem value={NO_UNIT}>
                  {t('agentOrganizations.noUnit')}
                </SelectItem>
                {units.map((unit) => (
                  <SelectItem key={unit.id} value={unit.id}>
                    {unit.name}
                  </SelectItem>
                ))}
              </SelectContent>
            </Select>
          </div>
          <div className='grid gap-1'>
            <Label htmlFor='agent-membership-name'>
              {t('agentOrganizations.displayNameLabel')}
            </Label>
            <Input
              id='agent-membership-name'
              className='h-9 w-48'
              data-testid='admin-org-membership-name-input'
              value={displayName}
              onChange={(event) => setDisplayName(event.target.value)}
            />
          </div>
          <Button
            type='submit'
            size='sm'
            data-testid='admin-org-membership-submit'
            disabled={upsertMutation.isPending || userId === ''}
          >
            {t('agentCommon.save')}
          </Button>
        </form>
        <div className='overflow-hidden rounded-md border'>
          <Table>
            <TableHeader>
              <TableRow>
                <TableHead>{t('agentOrganizations.colUserId')}</TableHead>
                <TableHead>{t('agentOrganizations.colDisplayName')}</TableHead>
                <TableHead>{t('agentOrganizations.colUnit')}</TableHead>
                <TableHead className='text-end'>
                  {t('agentCommon.actions')}
                </TableHead>
              </TableRow>
            </TableHeader>
            <TableBody>
              {membershipsQuery.isLoading ? (
                <TableRow>
                  <TableCell colSpan={4} className='h-20 text-center'>
                    {t('agentCommon.loading')}
                  </TableCell>
                </TableRow>
              ) : memberships.length === 0 ? (
                <TableRow>
                  <TableCell colSpan={4} className='h-20 text-center'>
                    {t('agentOrganizations.noMemberships')}
                  </TableCell>
                </TableRow>
              ) : (
                memberships.map((membership) => {
                  const unit = units.find(
                    (candidate) => candidate.id === membership.unit_id
                  )
                  return (
                    <TableRow
                      key={membership.user_id}
                      data-testid='admin-org-membership-row'
                    >
                      <TableCell>{membership.user_id}</TableCell>
                      <TableCell>{membership.display_name ?? '-'}</TableCell>
                      <TableCell>{unit?.name ?? '-'}</TableCell>
                      <TableCell className='text-end'>
                        <Button
                          variant='outline'
                          size='sm'
                          data-testid='admin-org-membership-remove'
                          disabled={removeMutation.isPending}
                          onClick={() =>
                            removeMutation.mutate(membership.user_id)
                          }
                        >
                          {t('agentCommon.remove')}
                        </Button>
                      </TableCell>
                    </TableRow>
                  )
                })
              )}
            </TableBody>
          </Table>
        </div>
      </CardContent>
    </Card>
  )
}

function AdminScopesCard({ organizationId }: { organizationId: string }) {
  const { t } = useTranslation()
  const queryClient = useQueryClient()
  const [adminUserId, setAdminUserId] = useState('')

  const scopesQuery = useQuery({
    queryKey: adminScopesQueryKey(organizationId),
    queryFn: () => listAdminScopes(organizationId),
  })

  const grantMutation = useMutation({
    mutationFn: () => grantAdminScope(organizationId, Number(adminUserId)),
    onSuccess: () => {
      setAdminUserId('')
      void queryClient.invalidateQueries({
        queryKey: adminScopesQueryKey(organizationId),
      })
    },
    onError: (error: Error) => toast.error(error.message),
  })

  const revokeMutation = useMutation({
    mutationFn: (targetAdminId: number) =>
      revokeAdminScope(organizationId, targetAdminId),
    onSuccess: () => {
      void queryClient.invalidateQueries({
        queryKey: adminScopesQueryKey(organizationId),
      })
    },
    onError: (error: Error) => toast.error(error.message),
  })

  const scopes = scopesQuery.data ?? []

  return (
    <Card>
      <CardHeader>
        <CardTitle>{t('agentOrganizations.scopesTitle')}</CardTitle>
        <CardDescription>
          {t('agentOrganizations.scopesDescription')}
        </CardDescription>
      </CardHeader>
      <CardContent className='flex flex-col gap-3'>
        <form
          className='flex items-end gap-2'
          onSubmit={(event) => {
            event.preventDefault()
            if (adminUserId !== '') grantMutation.mutate()
          }}
        >
          <div className='grid gap-1'>
            <Label htmlFor='agent-scope-admin'>
              {t('agentOrganizations.adminUserIdLabel')}
            </Label>
            <Input
              id='agent-scope-admin'
              className='h-9 w-32'
              type='number'
              min={1}
              data-testid='admin-org-scope-admin-input'
              value={adminUserId}
              onChange={(event) => setAdminUserId(event.target.value)}
            />
          </div>
          <Button
            type='submit'
            size='sm'
            data-testid='admin-org-scope-grant'
            disabled={grantMutation.isPending || adminUserId === ''}
          >
            {t('agentOrganizations.grant')}
          </Button>
        </form>
        <div className='overflow-hidden rounded-md border'>
          <Table>
            <TableHeader>
              <TableRow>
                <TableHead>{t('agentOrganizations.colAdminId')}</TableHead>
                <TableHead>{t('agentOrganizations.colGrantedBy')}</TableHead>
                <TableHead>{t('agentOrganizations.colCreatedAt')}</TableHead>
                <TableHead className='text-end'>
                  {t('agentCommon.actions')}
                </TableHead>
              </TableRow>
            </TableHeader>
            <TableBody>
              {scopesQuery.isLoading ? (
                <TableRow>
                  <TableCell colSpan={4} className='h-20 text-center'>
                    {t('agentCommon.loading')}
                  </TableCell>
                </TableRow>
              ) : scopes.length === 0 ? (
                <TableRow>
                  <TableCell colSpan={4} className='h-20 text-center'>
                    {t('agentOrganizations.noScopes')}
                  </TableCell>
                </TableRow>
              ) : (
                scopes.map((scope) => (
                  <TableRow
                    key={scope.admin_user_id}
                    data-testid='admin-org-scope-row'
                  >
                    <TableCell>{scope.admin_user_id}</TableCell>
                    <TableCell>{scope.granted_by_admin_id}</TableCell>
                    <TableCell>{formatDateTime(scope.created_at)}</TableCell>
                    <TableCell className='text-end'>
                      <Button
                        variant='outline'
                        size='sm'
                        data-testid='admin-org-scope-revoke'
                        disabled={revokeMutation.isPending}
                        onClick={() =>
                          revokeMutation.mutate(scope.admin_user_id)
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
      </CardContent>
    </Card>
  )
}

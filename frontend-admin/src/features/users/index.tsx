import { useState } from 'react'
import {
  keepPreviousData,
  useMutation,
  useQuery,
  useQueryClient,
} from '@tanstack/react-query'
import { useTranslation } from 'react-i18next'
import { toast } from 'sonner'
import {
  type PublicUser,
  type PublicUserStatus,
  disablePublicUser,
  enablePublicUser,
  listPublicUsers,
} from '@/api/users'
import {
  AlertDialog,
  AlertDialogAction,
  AlertDialogCancel,
  AlertDialogContent,
  AlertDialogDescription,
  AlertDialogFooter,
  AlertDialogHeader,
  AlertDialogTitle,
} from '@/components/ui/alert-dialog'
import { Badge } from '@/components/ui/badge'
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
import { ConfigDrawer } from '@/components/config-drawer'
import { Header } from '@/components/layout/header'
import { Main } from '@/components/layout/main'
import { ProfileDropdown } from '@/components/profile-dropdown'
import { Search } from '@/components/search'
import { ThemeSwitch } from '@/components/theme-switch'

const PUBLIC_USERS_QUERY_KEY = ['admin', 'public-users'] as const
const PAGE_SIZE = 20
/** 「全部状态」在下拉框里的取值（Radix 的 Select 不允许空字符串）。 */
const ALL_STATUSES = 'all'

/** Format an ISO date string for display. */
function formatDateTime(value: string): string {
  const parsedDate = new Date(value)
  return Number.isNaN(parsedDate.getTime()) ? '-' : parsedDate.toLocaleString()
}

/** Render the Users component. */
export function Users() {
  const { t } = useTranslation()
  const queryClient = useQueryClient()
  const [keywordInput, setKeywordInput] = useState('')
  const [appliedKeyword, setAppliedKeyword] = useState('')
  const [statusFilter, setStatusFilter] = useState<
    PublicUserStatus | undefined
  >()
  const [page, setPage] = useState(1)
  const [pendingUser, setPendingUser] = useState<PublicUser | null>(null)

  const usersQuery = useQuery({
    queryKey: [...PUBLIC_USERS_QUERY_KEY, appliedKeyword, statusFilter, page],
    queryFn: () =>
      listPublicUsers({
        page,
        page_size: PAGE_SIZE,
        status: statusFilter,
        keyword: appliedKeyword || undefined,
      }),
    placeholderData: keepPreviousData,
  })

  const toggleMutation = useMutation({
    mutationFn: (targetUser: PublicUser) =>
      targetUser.status === 'active'
        ? disablePublicUser(targetUser.id)
        : enablePublicUser(targetUser.id),
    onSuccess: (updatedUser) => {
      toast.success(
        t(
          updatedUser.status === 'active'
            ? 'users.enabledToast'
            : 'users.disabledToast',
          { email: updatedUser.email }
        )
      )
      void queryClient.invalidateQueries({ queryKey: PUBLIC_USERS_QUERY_KEY })
    },
    onSettled: () => setPendingUser(null),
  })

  const publicUsers: PublicUser[] = usersQuery.data?.items ?? []
  const total = usersQuery.data?.total ?? 0
  const pageCount = Math.max(1, Math.ceil(total / PAGE_SIZE))

  /** Render the table body rows. */
  function renderTableBody() {
    if (usersQuery.isLoading) {
      return (
        <TableRow>
          <TableCell colSpan={4} className='h-24 text-center'>
            {t('users.loading')}
          </TableCell>
        </TableRow>
      )
    }
    if (usersQuery.isError) {
      return (
        <TableRow>
          <TableCell colSpan={4} className='h-24 text-center text-destructive'>
            {t('users.loadFailed')}
          </TableCell>
        </TableRow>
      )
    }
    if (publicUsers.length === 0) {
      return (
        <TableRow>
          <TableCell colSpan={4} className='h-24 text-center'>
            {t('users.empty')}
          </TableCell>
        </TableRow>
      )
    }
    return publicUsers.map((publicUser) => (
      <TableRow key={publicUser.id} data-testid='admin-user-row'>
        <TableCell>{publicUser.email}</TableCell>
        <TableCell>
          <Badge
            variant='outline'
            className={
              publicUser.status === 'active'
                ? 'border-teal-200 bg-teal-100/30 text-teal-900 dark:text-teal-200'
                : 'border-destructive/20 bg-destructive/10 text-destructive'
            }
          >
            {publicUser.status === 'active'
              ? t('users.statusActive')
              : t('users.statusDisabled')}
          </Badge>
        </TableCell>
        <TableCell>{formatDateTime(publicUser.created_at)}</TableCell>
        <TableCell className='text-end'>
          <Button
            variant={publicUser.status === 'active' ? 'outline' : 'default'}
            size='sm'
            data-testid='admin-user-toggle'
            onClick={() => setPendingUser(publicUser)}
          >
            {publicUser.status === 'active'
              ? t('users.disable')
              : t('users.enable')}
          </Button>
        </TableCell>
      </TableRow>
    ))
  }

  const disabling = pendingUser?.status === 'active'

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
            {t('users.title')}
          </h2>
          <p className='text-muted-foreground'>{t('users.description')}</p>
        </div>

        <form
          className='flex flex-wrap items-center gap-2'
          onSubmit={(event) => {
            event.preventDefault()
            setAppliedKeyword(keywordInput.trim())
            setPage(1)
          }}
        >
          <Input
            placeholder={t('users.searchPlaceholder')}
            data-testid='admin-user-search-input'
            value={keywordInput}
            onChange={(event) => setKeywordInput(event.target.value)}
            className='h-9 w-64'
          />
          <Select
            value={statusFilter ?? ALL_STATUSES}
            onValueChange={(value) => {
              setStatusFilter(
                value === ALL_STATUSES ? undefined : (value as PublicUserStatus)
              )
              setPage(1)
            }}
          >
            <SelectTrigger
              className='h-9 w-40'
              data-testid='admin-user-status-filter'
            >
              <SelectValue />
            </SelectTrigger>
            <SelectContent>
              <SelectItem value={ALL_STATUSES}>
                {t('users.filterAll')}
              </SelectItem>
              <SelectItem value='active'>{t('users.filterActive')}</SelectItem>
              <SelectItem value='disabled'>
                {t('users.filterDisabled')}
              </SelectItem>
            </SelectContent>
          </Select>
          <Button type='submit' variant='outline' size='sm'>
            {t('users.search')}
          </Button>
        </form>

        <div className='overflow-hidden rounded-md border'>
          <Table>
            <TableHeader>
              <TableRow>
                <TableHead>{t('users.colEmail')}</TableHead>
                <TableHead>{t('users.colStatus')}</TableHead>
                <TableHead>{t('users.colCreatedAt')}</TableHead>
                <TableHead className='text-end'>
                  {t('users.colActions')}
                </TableHead>
              </TableRow>
            </TableHeader>
            <TableBody>{renderTableBody()}</TableBody>
          </Table>
        </div>

        <div className='flex items-center justify-between gap-2'>
          <span
            className='text-sm text-muted-foreground'
            data-testid='admin-user-page-summary'
          >
            {t('users.pageSummary', { total, page, pages: pageCount })}
          </span>
          <div className='flex gap-2'>
            <Button
              variant='outline'
              size='sm'
              disabled={page <= 1}
              onClick={() => setPage((current) => Math.max(1, current - 1))}
            >
              {t('users.previous')}
            </Button>
            <Button
              variant='outline'
              size='sm'
              disabled={page >= pageCount}
              onClick={() => setPage((current) => current + 1)}
            >
              {t('users.next')}
            </Button>
          </div>
        </div>
      </Main>

      <AlertDialog
        open={pendingUser !== null}
        onOpenChange={(open) => {
          if (!open) setPendingUser(null)
        }}
      >
        <AlertDialogContent>
          <AlertDialogHeader>
            <AlertDialogTitle>
              {disabling ? t('users.disableTitle') : t('users.enableTitle')}
            </AlertDialogTitle>
            <AlertDialogDescription>
              {t(
                disabling
                  ? 'users.disableDescription'
                  : 'users.enableDescription',
                { email: pendingUser?.email }
              )}
            </AlertDialogDescription>
          </AlertDialogHeader>
          <AlertDialogFooter>
            <AlertDialogCancel disabled={toggleMutation.isPending}>
              {t('common.cancel')}
            </AlertDialogCancel>
            <AlertDialogAction
              data-testid='admin-user-confirm'
              disabled={toggleMutation.isPending}
              onClick={() => {
                if (pendingUser) toggleMutation.mutate(pendingUser)
              }}
            >
              {t('users.confirm')}
            </AlertDialogAction>
          </AlertDialogFooter>
        </AlertDialogContent>
      </AlertDialog>
    </>
  )
}

import { useTranslation } from 'react-i18next'
import { useAuthStore } from '@/stores/auth-store'
import { getDisplayNameInitials } from '@/lib/utils'
import useDialogState from '@/hooks/use-dialog-state'
import { Avatar, AvatarFallback } from '@/components/ui/avatar'
import { Button } from '@/components/ui/button'
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuLabel,
  DropdownMenuSeparator,
  DropdownMenuTrigger,
} from '@/components/ui/dropdown-menu'
import { SignOutDialog } from '@/components/sign-out-dialog'

/** Render the ProfileDropdown component. */
export function ProfileDropdown() {
  const [open, setOpen] = useDialogState()
  const { t } = useTranslation()
  const user = useAuthStore((state) => state.auth.user)
  const displayName = user?.display_name ?? ''

  return (
    <>
      <DropdownMenu modal={false}>
        <DropdownMenuTrigger asChild>
          <Button
            variant='ghost'
            className='relative h-8 w-8 rounded-full'
            aria-label={t('userMenu.account')}
          >
            <Avatar className='h-8 w-8'>
              <AvatarFallback>
                {getDisplayNameInitials(displayName)}
              </AvatarFallback>
            </Avatar>
          </Button>
        </DropdownMenuTrigger>
        <DropdownMenuContent className='w-56' align='end' forceMount>
          <DropdownMenuLabel className='font-normal'>
            <div className='flex flex-col gap-1.5'>
              <p className='text-sm leading-none font-medium'>{displayName}</p>
              <p className='text-xs leading-none text-muted-foreground'>
                {user?.username}
              </p>
            </div>
          </DropdownMenuLabel>
          <DropdownMenuSeparator />
          <DropdownMenuItem variant='destructive' onClick={() => setOpen(true)}>
            {t('userMenu.signOut')}
          </DropdownMenuItem>
        </DropdownMenuContent>
      </DropdownMenu>

      <SignOutDialog open={!!open} onOpenChange={setOpen} />
    </>
  )
}

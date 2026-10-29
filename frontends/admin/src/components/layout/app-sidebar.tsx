import { useTranslation } from 'react-i18next'
import { useAuthStore } from '@/stores/auth-store'
import { useLayout } from '@/context/layout-provider'
import {
  Sidebar,
  SidebarContent,
  SidebarFooter,
  SidebarHeader,
  SidebarRail,
} from '@/components/ui/sidebar'
import { AppTitle } from './app-title'
import { createNavGroups } from './data/sidebar-data'
import { NavGroup } from './nav-group'
import { NavUser } from './nav-user'

/** Render the AppSidebar component. */
export function AppSidebar() {
  const { collapsible, variant } = useLayout()
  const { auth } = useAuthStore()
  const { t } = useTranslation()

  const navGroups = createNavGroups(t)

  return (
    <Sidebar collapsible={collapsible} variant={variant}>
      <SidebarHeader>
        <AppTitle />
      </SidebarHeader>
      <SidebarContent>
        {navGroups.map((props) => (
          <NavGroup key={props.title} {...props} />
        ))}
      </SidebarContent>
      <SidebarFooter>
        {auth.user && (
          <NavUser
            user={{ name: auth.user.display_name, email: auth.user.username }}
          />
        )}
      </SidebarFooter>
      <SidebarRail />
    </Sidebar>
  )
}

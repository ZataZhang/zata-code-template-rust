import { Suspense } from "react"
import { useTranslations } from "next-intl"
import { AccountsAuthClient } from "@/components/accounts/accounts-auth-client"

/** Render the shared account module inside the public site's login route. */
export default function LoginPage() {
  const tCommon = useTranslations("common")

  return (
    <Suspense
      fallback={
        <div className="py-8 text-center text-sm text-muted-foreground">
          {tCommon("loading")}
        </div>
      }
    >
      <AccountsAuthClient mode="login" />
    </Suspense>
  )
}

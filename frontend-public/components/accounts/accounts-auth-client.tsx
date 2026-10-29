"use client"

import { useRouter, useSearchParams } from "next/navigation"
import { useTranslations } from "next-intl"
import { toast } from "sonner"
import {
  AccountsAuthForm,
  type AccountsAuthCopy,
  type AccountsAuthMode,
} from "@tpl/accounts-ui"
import { useApiErrorMessage } from "@/lib/api/use-api-error-message"

/** 把认证 UI 包接入公开站点的翻译、路由和提示系统。 */
export function AccountsAuthClient({ mode }: { mode: AccountsAuthMode }) {
  const router = useRouter()
  const searchParams = useSearchParams()
  const t = useTranslations("auth")
  const tErrors = useTranslations("errors")
  const tToast = useTranslations("toast")
  const describeApiError = useApiErrorMessage()

  const copy: AccountsAuthCopy = {
    title: t(mode === "login" ? "loginTitle" : "registerTitle"),
    subtitle: t(mode === "login" ? "loginSubtitle" : "registerSubtitle"),
    emailLabel: t("emailLabel"),
    emailPlaceholder: t("emailPlaceholder"),
    passwordLabel: t("passwordLabel"),
    confirmPasswordLabel: t("confirmPasswordLabel"),
    submitLabel: t(mode === "login" ? "loginSubmit" : "registerSubmit"),
    switchPrompt: t(mode === "login" ? "noAccount" : "haveAccount"),
    switchLabel: t(mode === "login" ? "signUpNow" : "signInNow"),
    forgotPasswordLabel: mode === "login" ? t("forgotPassword") : undefined,
    emailRequired: tErrors("emailRequired"),
    emailInvalid: tErrors("emailInvalid"),
    passwordRequired: tErrors("passwordRequired"),
    passwordTooShort: tErrors("passwordTooShort"),
    passwordTooLong: tErrors("passwordTooLong"),
    confirmPasswordRequired: tErrors("confirmPasswordRequired"),
    passwordMismatch: tErrors("passwordMismatch"),
    requestFailed: tToast(mode === "login" ? "loginFailed" : "registerFailed"),
  }

  /** 登录或注册成功后显示宿主提示，并返回目标页面。 */
  function handleAuthenticated() {
    toast.success(tToast(mode === "login" ? "loginSuccess" : "registerSuccess"))
    const redirect = searchParams.get("redirect")
    router.replace(redirect || "/app/dashboard")
  }

  /** 根据模块请求切换公开站点自己的登录或注册路由。 */
  function handleSwitchMode(nextMode: AccountsAuthMode) {
    router.push(nextMode === "login" ? "/login" : "/register")
  }

  return (
    <AccountsAuthForm
      mode={mode}
      apiBaseUrl="/api"
      copy={copy}
      describeError={describeApiError}
      onAuthenticated={handleAuthenticated}
      onSwitchMode={handleSwitchMode}
      onForgotPassword={() => toast.info(t("forgotPasswordHint"))}
    />
  )
}

"use client"

import { useState, type FormEvent } from "react"

import {
  AccountsApiError,
  createAccountsClient,
  getAccountPasswordIssue,
  type AccountSession,
} from "../client.js"

/** 账号表单的模式。 */
export type AccountsAuthMode = "login" | "register"

/** 由宿主提供的全部认证页面文案。 */
export type AccountsAuthCopy = {
  title: string
  subtitle: string
  emailLabel: string
  emailPlaceholder: string
  passwordLabel: string
  confirmPasswordLabel: string
  submitLabel: string
  switchPrompt: string
  switchLabel: string
  forgotPasswordLabel?: string
  emailRequired: string
  emailInvalid: string
  passwordRequired: string
  passwordTooShort: string
  passwordTooLong: string
  confirmPasswordRequired: string
  passwordMismatch: string
  requestFailed: string
}

/** 认证表单与宿主之间的适配参数。 */
export type AccountsAuthFormProps = {
  /** 显示登录或注册表单。 */
  mode: AccountsAuthMode
  /** 宿主同源的账号 API 前缀。 */
  apiBaseUrl: string
  /** 单次请求等待上限，默认 10 秒，上限 60 秒。 */
  timeoutMs?: number
  /** 登录和注册文案。 */
  copy: AccountsAuthCopy
  /** 宿主根据稳定错误码提供本地化错误消息。 */
  describeError?: (error: AccountsApiError, fallback: string) => string
  /** 认证成功时由宿主完成 toast 与导航。 */
  onAuthenticated: (session: AccountSession, mode: AccountsAuthMode) => void
  /** 点击切换入口时由宿主决定目标路由。 */
  onSwitchMode: (mode: AccountsAuthMode) => void
  /** 可选的忘记密码提示，由宿主决定如何呈现。 */
  onForgotPassword?: () => void
  /** 宿主可附加样式类名。 */
  className?: string
}

/** 渲染可嵌入任意 React 宿主的普通用户认证表单。 */
export function AccountsAuthForm({
  mode,
  apiBaseUrl,
  timeoutMs,
  copy,
  describeError,
  onAuthenticated,
  onSwitchMode,
  onForgotPassword,
  className,
}: AccountsAuthFormProps) {
  const [email, setEmail] = useState("")
  const [password, setPassword] = useState("")
  const [confirmPassword, setConfirmPassword] = useState("")
  const [errorMessage, setErrorMessage] = useState("")
  const [isLoading, setIsLoading] = useState(false)
  const isLogin = mode === "login"
  const accountsClient = createAccountsClient({ baseUrl: apiBaseUrl, timeoutMs })

  async function handleSubmit(event: FormEvent<HTMLFormElement>) {
    event.preventDefault()
    setErrorMessage("")

    if (!email.trim()) return setErrorMessage(copy.emailRequired)
    if (!/^[^\s@]+@[^\s@]+\.[^\s@]+$/.test(email.trim())) {
      return setErrorMessage(copy.emailInvalid)
    }
    if (!password) return setErrorMessage(copy.passwordRequired)
    if (!isLogin) {
      const password_issue = getAccountPasswordIssue(password)
      if (password_issue === "too_short") return setErrorMessage(copy.passwordTooShort)
      if (password_issue === "too_long") return setErrorMessage(copy.passwordTooLong)
    }
    if (!isLogin && !confirmPassword) {
      return setErrorMessage(copy.confirmPasswordRequired)
    }
    if (!isLogin && password !== confirmPassword) {
      return setErrorMessage(copy.passwordMismatch)
    }

    setIsLoading(true)
    try {
      const credentials = { email: email.trim(), password }
      const session = isLogin
        ? await accountsClient.login(credentials)
        : await accountsClient.register(credentials)
      onAuthenticated(session, mode)
    } catch (error) {
      const fallback = copy.requestFailed
      const message =
        error instanceof AccountsApiError
          ? (describeError?.(error, fallback) ?? error.message)
          : fallback
      setErrorMessage(message)
    } finally {
      setIsLoading(false)
    }
  }

  const switchMode = isLogin ? "register" : "login"

  return (
    <section className={`accounts-auth ${className ?? ""}`.trim()}>
      <header className="accounts-auth__header">
        <h1>{copy.title}</h1>
        <p>{copy.subtitle}</p>
      </header>
      <form className="accounts-auth__form" onSubmit={handleSubmit} noValidate>
        <label className="accounts-auth__field">
          <span>{copy.emailLabel}</span>
          <input
            data-testid={isLogin ? "login-email-input" : "register-email-input"}
            type="email"
            autoComplete="email"
            placeholder={copy.emailPlaceholder}
            value={email}
            onChange={(event) => setEmail(event.currentTarget.value)}
          />
        </label>
        <label className="accounts-auth__field">
          <span className="accounts-auth__field-heading">
            <span>{copy.passwordLabel}</span>
            {isLogin && copy.forgotPasswordLabel && onForgotPassword && (
              <button
                className="accounts-auth__forgot"
                type="button"
                onClick={onForgotPassword}
              >
                {copy.forgotPasswordLabel}
              </button>
            )}
          </span>
          <input
            data-testid={isLogin ? "login-password-input" : "register-password-input"}
            type="password"
            autoComplete={isLogin ? "current-password" : "new-password"}
            placeholder="********"
            value={password}
            onChange={(event) => setPassword(event.currentTarget.value)}
          />
        </label>
        {!isLogin && (
          <label className="accounts-auth__field">
            <span>{copy.confirmPasswordLabel}</span>
            <input
              data-testid="register-confirm-password-input"
              type="password"
              autoComplete="new-password"
              placeholder="********"
              value={confirmPassword}
              onChange={(event) => setConfirmPassword(event.currentTarget.value)}
            />
          </label>
        )}
        {errorMessage && (
          <p className="accounts-auth__error" role="alert" aria-live="polite">
            {errorMessage}
          </p>
        )}
        <button
          className="accounts-auth__submit"
          data-testid={isLogin ? "login-submit-button" : "register-submit-button"}
          type="submit"
          disabled={isLoading}
        >
          {isLoading ? <span className="accounts-auth__spinner" aria-hidden="true" /> : null}
          {copy.submitLabel}
        </button>
      </form>
      <p className="accounts-auth__switch">
        {copy.switchPrompt}{" "}
        <button type="button" onClick={() => onSwitchMode(switchMode)}>
          {copy.switchLabel}
        </button>
      </p>
    </section>
  )
}

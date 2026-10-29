import { useEffect, useState } from "react"
import { createRoot } from "react-dom/client"
import {
  AccountsAuthForm,
  createAccountsClient,
} from "@tpl/accounts-ui"
import type {
  AccountsApiError,
  AccountSession,
  AccountsAuthCopy,
  AccountsAuthMode,
} from "@tpl/accounts-ui"
import "@tpl/accounts-ui/styles.css"
import "./styles.css"

const accounts_client = createAccountsClient({ baseUrl: "/api" })

const auth_copy: AccountsAuthCopy = {
  title: "Example application",
  subtitle: "This host consumes the packed accounts UI module",
  emailLabel: "Email",
  emailPlaceholder: "you@example.com",
  passwordLabel: "Password",
  confirmPasswordLabel: "Confirm password",
  submitLabel: "Continue",
  switchPrompt: "Need an account?",
  switchLabel: "Create one",
  emailRequired: "Enter your email",
  emailInvalid: "Enter a valid email",
  passwordRequired: "Enter your password",
  passwordTooShort: "Password must be at least 8 characters",
  passwordTooLong: "Password is too long (at most 128 bytes)",
  confirmPasswordRequired: "Confirm your password",
  passwordMismatch: "The passwords do not match",
  requestFailed: "Account request failed",
}

function mode_from_location(pathname: string): AccountsAuthMode {
  return pathname.endsWith("/register") ? "register" : "login"
}

/** 演示其他 React 应用安装并使用账号模块 tarball。 */
export function Main() {
  const [mode, setMode] = useState<AccountsAuthMode>(() =>
    mode_from_location(window.location.pathname)
  )
  const [session, setSession] = useState<AccountSession>()
  const [sessionLoaded, setSessionLoaded] = useState(false)

  async function refreshSession() {
    try {
      const fresh_session = await accounts_client.getCurrentSession()
      setSession(fresh_session)
    } catch {
      setSession(undefined)
    } finally {
      setSessionLoaded(true)
    }
  }

  useEffect(() => {
    void refreshSession()
  }, [])

  useEffect(() => {
    function sync_mode_from_location() {
      setMode(mode_from_location(window.location.pathname))
    }

    window.addEventListener("popstate", sync_mode_from_location)
    return () => window.removeEventListener("popstate", sync_mode_from_location)
  }, [])

  async function handleAuthenticated() {
    // Read from a new HTTP request so the example demonstrates the persisted cookie session.
    await refreshSession()
  }

  async function handleLogout() {
    await accounts_client.logout()
    handleModeChange("login")
    await refreshSession()
  }

  function handleModeChange(next_mode: AccountsAuthMode) {
    window.history.pushState({}, "", next_mode === "register" ? "/register" : "/login")
    setMode(next_mode)
  }

  function describeError(error: AccountsApiError, fallback: string) {
    if (error.code === "invalid_credentials") return "Incorrect email or password"
    if (error.code === "email_taken") return "This email is already registered"
    if (error.code === "too_many_attempts") {
      const minutes = Math.max(1, Math.ceil((error.retryAfterSeconds ?? 60) / 60))
      return `Too many attempts. Please try again in ${minutes} minutes.`
    }
    return fallback
  }

  return (
    <main className="host-shell">
      <section className="host-intro">
        <p className="host-eyebrow">Second host</p>
        <h1>Accounts module integration</h1>
        <p>
          This app installs <code>@tpl/accounts-ui</code> from the generated npm tarball.
        </p>
      </section>
      {!sessionLoaded ? (
        <p role="status">Loading session…</p>
      ) : session ? (
        <section className="host-session" aria-label="Account session">
          <h2>Signed in</h2>
          <p data-testid="current-user-email">{session.email}</p>
          <button type="button" onClick={() => void handleLogout()}>
            Sign out
          </button>
        </section>
      ) : (
        <AccountsAuthForm
          mode={mode}
          apiBaseUrl="/api"
          copy={{
            ...auth_copy,
            switchPrompt:
              mode === "login" ? "Need an account?" : "Already have an account?",
            switchLabel: mode === "login" ? "Create one" : "Sign in",
          }}
          describeError={describeError}
          onAuthenticated={handleAuthenticated}
          onSwitchMode={handleModeChange}
        />
      )}
    </main>
  )
}

const root_element = document.getElementById("root")
if (!root_element) throw new Error("Missing #root host element")
createRoot(root_element).render(<Main />)

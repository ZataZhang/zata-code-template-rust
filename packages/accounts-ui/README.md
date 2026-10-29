# @tpl/accounts-ui

Reusable public account registration and sign-in UI for applications that use `accounts-service`.

This is a private, local versioned package artifact; it is not published. The repository has not selected a license for independent redistribution. A maintainer must choose the license before public distribution.

The package owns the email/password form rules and HTTP calls. The host provides its same-origin API prefix, translated copy, route changes, notifications, and styling overrides. It uses the public-user `/auth/*` endpoints and `session` cookie only; administrator sessions and APIs are intentionally not included.

```tsx
import {
  AccountsAuthForm,
  type AccountsAuthCopy,
} from "@tpl/accounts-ui"
import "@tpl/accounts-ui/styles.css"

const copy: AccountsAuthCopy = {
  title: "Sign in",
  subtitle: "Enter your account details",
  emailLabel: "Email",
  emailPlaceholder: "you@example.com",
  passwordLabel: "Password",
  confirmPasswordLabel: "Confirm password",
  submitLabel: "Sign in",
  switchPrompt: "Need an account?",
  switchLabel: "Register",
  emailRequired: "Enter your email",
  emailInvalid: "Enter a valid email",
  passwordRequired: "Enter your password",
  passwordTooShort: "Use at least 8 characters",
  passwordTooLong: "Use at most 128 UTF-8 bytes",
  confirmPasswordRequired: "Confirm your password",
  passwordMismatch: "Passwords do not match",
  requestFailed: "Sign in failed",
}

<AccountsAuthForm
  mode="login"
  apiBaseUrl="/api"
  copy={copy}
  onAuthenticated={(session) => console.info(session.email)}
  onSwitchMode={() => window.location.assign("/register")}
/>
```

Build and create the versioned package archive with `pnpm run pack:tarball`. Requests use `credentials: "include"` and time out after 10 seconds by default; a host can pass `timeoutMs` to `createAccountsClient` or override the form client. See `examples/accounts-host/README.md` for installing the actual archive into a separate React application.

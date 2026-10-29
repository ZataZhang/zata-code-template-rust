import { mkdir } from "node:fs/promises"
import { chromium } from "playwright-core"

const base_url = process.env.HOST_URL
if (!base_url) {
  console.error("Set HOST_URL or run through the accounts-host e2e command wrapper")
  process.exit(2)
}

const screenshot_dir = process.env.SHOT_DIR
if (screenshot_dir) await mkdir(screenshot_dir, { recursive: true })

const launch_options = { headless: true }
if (process.env.CHROME_PATH) launch_options.executablePath = process.env.CHROME_PATH
else launch_options.channel = "chrome"

const browser = await chromium.launch(launch_options)
const context = await browser.newContext({ locale: "en-US" })
const page = await context.newPage()
const me_statuses = []
const failures = []
page.on("response", (response) => {
  if (response.url().includes("/api/auth/me")) me_statuses.push(response.status())
})
page.on("pageerror", (error) => failures.push(String(error)))

function check(name, passed, detail = "") {
  console.log(`${passed ? "PASS" : "FAIL"} ${name}${passed ? "" : `: ${detail}`}`)
  if (!passed) process.exitCode = 1
}

async function screenshot(name) {
  if (screenshot_dir) await page.screenshot({ path: `${screenshot_dir}/${name}.png` })
}

const email = `accounts-host-${Date.now()}@example.test`
const password = "correct-horse-9"

try {
  await page.goto(base_url)
  await page.getByRole("button", { name: "Create one" }).click()
  await page.getByLabel("Confirm password").waitFor({ timeout: 8000 })
  await page.goBack()
  await page.getByLabel("Confirm password").waitFor({ state: "detached", timeout: 8000 })
  check("browser back restores the login form and URL", !page.url().endsWith("/register"))
  await page.goForward()
  await page.getByLabel("Confirm password").waitFor({ timeout: 8000 })
  check("browser forward restores the registration form and URL", page.url().endsWith("/register"))
  await page.getByLabel("Email").fill(email)
  await page.getByLabel("Password", { exact: true }).fill(password)
  await page.getByLabel("Confirm password").fill(password)
  await screenshot("accounts-host-register")
  await page.getByRole("button", { name: "Continue" }).click()
  await page.getByTestId("current-user-email").waitFor({ timeout: 15000 })
  check("tarball package registers a user through accounts-service", (await page.getByTestId("current-user-email").textContent()) === email)
  check("fresh /auth/me returns the new persisted session", me_statuses.includes(200), me_statuses.join(","))
  await screenshot("accounts-host-signed-in")

  await page.getByRole("button", { name: "Sign out" }).click()
  await page.getByRole("button", { name: "Continue" }).waitFor({ timeout: 8000 })
  check("fresh /auth/me returns 401 after logout", me_statuses.at(-1) === 401, me_statuses.join(","))

  await page.getByLabel("Email").fill(email)
  await page.getByLabel("Password", { exact: true }).fill("wrong-password")
  const failed_login_response = page.waitForResponse((response) =>
    response.url().includes("/api/auth/login") &&
    response.request().method() === "POST"
  )
  await page.getByRole("button", { name: "Continue" }).click()
  const failed_login = await failed_login_response
  await page.getByText("Incorrect email or password").waitFor({ timeout: 8000 })
  const fresh_me_status = await page.evaluate(async () => {
    const response = await fetch("/api/auth/me", { credentials: "include" })
    return response.status
  })
  check("wrong credentials are rejected with HTTP 401", failed_login.status() === 401)
  check("fresh /auth/me remains 401 after failed login", fresh_me_status === 401)
  check(
    "failed login leaves no signed-in account in the host",
    (await page.getByTestId("current-user-email").count()) === 0
  )
  check("no browser runtime exception", failures.length === 0, failures.join(" | "))
} finally {
  await context.close()
  await browser.close()
}

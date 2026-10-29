// 管理后台的真实浏览器端到端测试：Chrome -> vite preview（/api 代理）-> Rust 后端 -> PostgreSQL / Redis。
//
// 不要直接运行它：用 `just web-e2e`，它会启动数据库、后端和两个前端，再运行本脚本。
// 本脚本假定：
//   - 后端用引导账号 ADMIN_USERNAME / ADMIN_PASSWORD（见下），且 `APP_ADMIN__LOGIN_MAX_FAILURES_PER_ACCOUNT=3`；
//   - 界面语言默认是英文。
//
// 环境变量：
//   BASE_URL        管理后台地址（必填）
//   BACKEND_URL     后端地址（必填；用来直接注册/登录普通用户，验证后台操作对他们的真实影响）
//   ADMIN_USERNAME / ADMIN_PASSWORD  引导管理员（必填）
//   CHROME_PATH     Chrome 可执行文件路径；不设时使用 Playwright 的 `chrome` 渠道
//   SHOT_DIR        设置后会在关键步骤保存截图

import { mkdirSync } from "node:fs"

import { chromium } from "playwright"

const BASE = process.env.BASE_URL
const BACKEND = process.env.BACKEND_URL
const ADMIN_USERNAME = process.env.ADMIN_USERNAME
const ADMIN_PASSWORD = process.env.ADMIN_PASSWORD
if (!BASE || !BACKEND || !ADMIN_USERNAME || !ADMIN_PASSWORD) {
  console.error("请设置 BASE_URL、BACKEND_URL、ADMIN_USERNAME、ADMIN_PASSWORD，或直接用 `just web-e2e`")
  process.exit(2)
}
const SHOT_DIR = process.env.SHOT_DIR
if (SHOT_DIR) mkdirSync(SHOT_DIR, { recursive: true })

const results = []
/** 记录并打印一条检查结果。 */
function check(name, ok, detail = "") {
  results.push({ name, ok })
  console.log(`${ok ? "PASS" : "FAIL"}  ${name}${ok ? "" : `   <-- ${detail}`}`)
}
/** 在设置了 SHOT_DIR 时截图。 */
async function shot(page, name) {
  if (SHOT_DIR) await page.screenshot({ path: `${SHOT_DIR}/admin-${name}.png` })
}
/** 等待文本出现；没出现也不抛错，由随后的 check 判定。 */
async function appears(page, text, timeout = 8000) {
  return await page
    .waitForFunction(
      (expectedText) => {
        const matchingElements = Array.from(document.querySelectorAll("body *")).filter(
          (element) => element.children.length === 0 && element.textContent?.includes(expectedText),
        )
        return matchingElements.some((element) => {
          for (let parent = element; parent; parent = parent.parentElement) {
            const style = window.getComputedStyle(parent)
            if (style.display === "none" || style.visibility === "hidden" || Number(style.opacity) < 0.95) {
              return false
            }
          }
          const bounds = element.getBoundingClientRect()
          return bounds.width > 0 && bounds.height > 0
        })
      },
      text,
      { timeout },
    )
    .then(() => true)
    .catch(() => false)
}
/** 轮询直到条件成立或超时。 */
async function eventually(predicate, timeout = 8000) {
  const deadline = Date.now() + timeout
  while (Date.now() < deadline) {
    if (await predicate()) return true
    await new Promise((resolve) => setTimeout(resolve, 100))
  }
  return predicate()
}

const stamp = Date.now()
const password = "correct-horse-9"
const emailA = `browser-adm-a-${stamp}@example.test`
const emailB = `browser-adm-b-${stamp}@example.test`

/** 直接调用后端（不经过管理后台），模拟「普通用户」一侧。 */
async function backend(path, body, cookie) {
  const response = await fetch(`${BACKEND}${path}`, {
    method: body === undefined ? "GET" : "POST",
    headers: { "Content-Type": "application/json", ...(cookie ? { Cookie: cookie } : {}) },
    body: body === undefined ? undefined : JSON.stringify(body),
  })
  const text = await response.text()
  return {
    status: response.status,
    body: text ? JSON.parse(text) : null,
    cookie: response.headers.get("set-cookie")?.split(";")[0] ?? null,
  }
}

const launchOptions = { headless: true }
if (process.env.CHROME_PATH) launchOptions.executablePath = process.env.CHROME_PATH
else launchOptions.channel = "chrome"
const browser = await chromium.launch(launchOptions)

const consoleErrors = []
/** 新建页面并收集控制台错误。 */
async function newPage(context) {
  const page = await context.newPage()
  page.on("console", (message) => {
    if (message.type() === "error") consoleErrors.push(message.text())
  })
  page.on("pageerror", (error) => consoleErrors.push(String(error)))
  return page
}

/** 填写并提交登录表单。 */
async function tryLogin(page, username, pw) {
  await page.getByTestId("admin-login-identifier-input").fill(username)
  await page.getByTestId("admin-login-password-input").fill(pw)
  await page.getByTestId("admin-login-submit-button").click()
}

/** 表格里某个邮箱所在的行。 */
function rowOf(page, email) {
  return page.getByTestId("admin-user-row").filter({ hasText: email })
}

try {
  // 准备两个普通用户，其中 A 已登录（持有一个有效会话）。
  for (const email of [emailA, emailB]) {
    const registered = await backend("/auth/register", { email, password })
    if (registered.status !== 201) throw new Error(`注册 ${email} 失败：${registered.status}`)
  }
  const userALogin = await backend("/auth/login", { email: emailA, password })
  const userACookie = userALogin.cookie
  check("准备：普通用户 A 能登录并拿到会话", userALogin.status === 200 && !!userACookie)

  const context = await browser.newContext({ viewport: { width: 1280, height: 800 } })
  const page = await newPage(context)

  // 1. 未登录访问受保护页面：被带到登录页，并记住来源。
  await page.goto(`${BASE}/users`)
  await page.waitForURL(/\/sign-in/, { timeout: 8000 }).catch(() => {})
  check("未登录访问 /users 被重定向到登录页", page.url().includes("/sign-in"), page.url())
  check("重定向保留来源（redirect 参数）", page.url().includes("redirect="), page.url())
  await shot(page, "01-sign-in")

  // 2. 密码错误：给出翻译后的错误，不进入后台。
  await tryLogin(page, ADMIN_USERNAME, "definitely-wrong-1")
  check("密码错误时显示翻译后的提示", await appears(page, "Incorrect username or password"))
  check("密码错误时仍停留在登录页", page.url().includes("/sign-in"), page.url())

  // 3. 普通用户的凭据不能登录后台（两个域隔离）。
  await tryLogin(page, emailA, password)
  check("普通用户的邮箱+密码无法登录后台", await appears(page, "Incorrect username or password"))

  // 4. 正确登录：回到来源页面，看到用户列表。
  await tryLogin(page, ADMIN_USERNAME, ADMIN_PASSWORD)
  await page.waitForURL(/\/users/, { timeout: 10000 }).catch(() => {})
  check("登录后进入用户管理页", page.url().includes("/users") && !page.url().includes("/sign-in"), page.url())
  check("页面标题显示 Users", await appears(page, "Manage the status of registered users."))
  check("侧边栏显示当前管理员", (await page.getByText(ADMIN_USERNAME).count()) > 0)
  const cookies = await context.cookies()
  const adminCookie = cookies.find((cookie) => cookie.name === "admin_session")
  check("管理员 Cookie 是 HttpOnly", adminCookie?.httpOnly === true, JSON.stringify(adminCookie))
  check("浏览器里没有普通用户的 session Cookie", !cookies.some((cookie) => cookie.name === "session"))
  await shot(page, "02-users")

  // 5. 关键字搜索与状态筛选。
  await page.getByTestId("admin-user-search-input").fill(`browser-adm-a-${stamp}`)
  await page.keyboard.press("Enter")
  check(
    "按邮箱关键字搜索只剩用户 A",
    await eventually(async () => (await rowOf(page, emailA).count()) === 1 && (await page.getByTestId("admin-user-row").count()) === 1),
  )
  await page.getByTestId("admin-user-search-input").fill(`browser-adm-`)
  await page.keyboard.press("Enter")
  check(
    "关键字匹配到两个测试用户，且汇总里的总数是 2",
    await eventually(async () => (await page.getByTestId("admin-user-row").count()) === 2) &&
      (await page.getByTestId("admin-user-page-summary").innerText()).includes("2 users"),
  )

  // 6. 禁用用户 A：先取消一次不应有任何影响，再确认。
  await rowOf(page, emailA).getByTestId("admin-user-toggle").click()
  await page.getByRole("button", { name: "Cancel" }).click()
  check(
    "取消禁用后用户 A 仍是 Active，且其会话仍有效",
    (await rowOf(page, emailA).innerText()).includes("Active") && (await backend("/auth/me", undefined, userACookie)).status === 200,
  )
  await rowOf(page, emailA).getByTestId("admin-user-toggle").click()
  check("禁用前有确认弹窗", await appears(page, "can no longer sign in"))
  await shot(page, "03-confirm-disable")
  await page.getByTestId("admin-user-confirm").click()
  check(
    "确认后列表里用户 A 变为 Disabled",
    await eventually(async () => (await rowOf(page, emailA).innerText()).includes("Disabled")),
  )
  check("禁用后用户 B 不受影响", (await rowOf(page, emailB).innerText()).includes("Active"))

  // 刷新后由全新的页面状态读取：禁用结果来自服务端，而不是前端缓存。
  await page.reload()
  await page.getByTestId("admin-user-search-input").fill(`browser-adm-a-${stamp}`)
  await page.keyboard.press("Enter")
  check(
    "刷新页面后用户 A 仍显示 Disabled",
    await eventually(async () => (await rowOf(page, emailA).count()) === 1 && (await rowOf(page, emailA).innerText()).includes("Disabled")),
  )
  await shot(page, "03b-after-reload")
  await page.getByTestId("admin-user-search-input").fill("browser-adm-")
  await page.keyboard.press("Enter")
  await eventually(async () => (await page.getByTestId("admin-user-row").count()) === 2)

  // 7. 禁用的真实效果：已有会话立即失效，新登录被拒绝且原因明确。
  const meAfter = await backend("/auth/me", undefined, userACookie)
  check("禁用后用户 A 已有的会话立即失效", meAfter.status === 401, String(meAfter.status))
  const loginAfter = await backend("/auth/login", { email: emailA, password })
  check(
    "禁用后用户 A 无法登录（account_disabled）",
    loginAfter.status === 403 && loginAfter.body?.error?.code === "account_disabled",
    JSON.stringify(loginAfter),
  )
  check("用户 B 仍能登录", (await backend("/auth/login", { email: emailB, password })).status === 200)

  // 8. 状态筛选 + 重新启用。
  await page.getByTestId("admin-user-status-filter").click()
  await page.getByRole("option", { name: "Disabled" }).click()
  check(
    "按「Disabled」筛选后只剩用户 A",
    await eventually(async () => (await page.getByTestId("admin-user-row").count()) === 1 && (await rowOf(page, emailA).count()) === 1),
  )
  await rowOf(page, emailA).getByTestId("admin-user-toggle").click()
  await page.getByTestId("admin-user-confirm").click()
  await page.getByTestId("admin-user-confirm").waitFor({ state: "hidden" })
  check(
    "启用后用户 A 从「Disabled」列表里消失",
    await eventually(async () => (await page.getByTestId("admin-user-row").count()) === 0),
  )
  const loginRestored = await backend("/auth/login", { email: emailA, password })
  check("启用后用户 A 能重新登录", loginRestored.status === 200, String(loginRestored.status))
  await shot(page, "04-after-enable")

  // 9. 语言切换：立即生效，刷新后保持。
  await page.getByTestId("language-option-zh").click()
  check("切换到中文后页面显示「用户」", await appears(page, "管理注册用户的状态。"))
  await page.reload()
  check("刷新后仍是中文", await appears(page, "管理注册用户的状态。"))
  await page.getByTestId("language-option-en").click()
  check("切回英文", await appears(page, "Manage the status of registered users."))

  // 10. 退出：服务端会话作废，再访问受保护页面回到登录页。
  const tokenBeforeLogout = (await context.cookies()).find((cookie) => cookie.name === "admin_session")?.value
  await page.getByRole("button", { name: "Account menu" }).click()
  await page.getByRole("menuitem", { name: "Sign out" }).click()
  await page.getByRole("button", { name: "Sign out" }).click()
  await page.waitForURL(/\/sign-in/, { timeout: 8000 }).catch(() => {})
  check("退出后回到登录页", page.url().includes("/sign-in"), page.url())
  const replay = await backend("/admin/users", undefined, `admin_session=${tokenBeforeLogout}`)
  check("用退出前的令牌重放被服务端拒绝", replay.status === 401, String(replay.status))
  await page.goto(`${BASE}/users`)
  await page.waitForURL(/\/sign-in/, { timeout: 8000 }).catch(() => {})
  check("退出后访问 /users 仍被拦到登录页", page.url().includes("/sign-in"), page.url())

  // 11. 登录限流（后端把单账号失败上限调成了 3）：第 4 次连正确密码也被拒绝，并给出提示。
  for (let attempt = 0; attempt < 3; attempt += 1) {
    await tryLogin(page, ADMIN_USERNAME, `wrong-password-${attempt}`)
    await appears(page, "Incorrect username or password")
    await page.waitForTimeout(300)
  }
  // 刷新只清掉前几次失败留下的提示；Redis 里的限流计数仍由真实服务保留。
  await page.reload()
  await page.getByTestId("admin-login-identifier-input").waitFor({ state: "visible" })
  await tryLogin(page, ADMIN_USERNAME, ADMIN_PASSWORD)
  const throttleToast = page
    .locator('[data-sonner-toast][data-front="true"]')
    .filter({ hasText: "Too many attempts" })
  await throttleToast.waitFor({ state: "visible" })
  await page.waitForFunction(
    (expectedText) => {
      const frontToast = document.querySelector('[data-sonner-toast][data-front="true"]')
      return (
        frontToast?.textContent?.includes(expectedText) &&
        Number(window.getComputedStyle(frontToast).opacity) >= 0.95
      )
    },
    "Too many attempts",
  )
  check("连续失败后被限流（正确密码也不行）", (await throttleToast.innerText()).includes("Too many attempts"))
  check("被限流后没有进入后台", page.url().includes("/sign-in"), page.url())
  await shot(page, "05-throttled")

  const unexpectedErrors = consoleErrors.filter(
    // 预期内的失败请求（401/403/429）会被浏览器记录为控制台错误，这里只关心其它错误。
    (text) => !/status of (401|403|429)|401 \(|403 \(|429 \(/.test(text),
  )
  check("没有意外的控制台错误", unexpectedErrors.length === 0, unexpectedErrors.join(" | "))
  await context.close()
} catch (error) {
  check("脚本执行未抛出异常", false, String(error?.stack ?? error))
} finally {
  await browser.close()
}

const failed = results.filter((result) => !result.ok)
console.log(`\n${results.length - failed.length}/${results.length} 项通过`)
process.exit(failed.length === 0 ? 0 : 1)

// 真实浏览器端到端测试：Chrome -> Next（rewrite 代理）-> Rust 后端 -> PostgreSQL / Redis。
//
// 不要直接运行它：用 `just web-e2e`，它会启动数据库、后端（账号限流调成 3 次）和前端，再运行本脚本。
// 本脚本假定后端的 `APP_AUTH__LOGIN_MAX_FAILURES_PER_ACCOUNT=3`，且界面语言默认是英文。
//
// 环境变量：
//   BASE_URL     前端地址（必填，如 http://localhost:13100）
//   CHROME_PATH  Chrome 可执行文件路径；不设时使用 Playwright 的 `chrome` 渠道（需要本机装了 Chrome）
//   SHOT_DIR     设置后会在关键步骤保存截图，便于人工查看

import { mkdirSync } from "node:fs"

import { chromium } from "playwright-core"

const BASE = process.env.BASE_URL
if (!BASE) {
  console.error("请设置 BASE_URL，或直接用 `just web-e2e`")
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
  if (SHOT_DIR) await page.screenshot({ path: `${SHOT_DIR}/${name}.png` })
}
/** 等待文本出现；没出现也不抛错，由随后的 check 判定。 */
async function appears(page, text, timeout = 8000) {
  await page
    .getByText(text)
    .first()
    .waitFor({ timeout })
    .catch(() => {})
  return (await page.getByText(text).count()) > 0
}

const launchOptions = { headless: true }
if (process.env.CHROME_PATH) launchOptions.executablePath = process.env.CHROME_PATH
else launchOptions.channel = "chrome"
const browser = await chromium.launch(launchOptions)

const stamp = Date.now()
const emailA = `browser-a-${stamp}@example.test`
const emailB = `browser-b-${stamp}@example.test`
const password = "correct-horse-9"

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

/** 填写并提交注册表单。 */
async function fillRegister(page, email, pw, confirm = pw) {
  await page.getByLabel("Email").fill(email)
  await page.getByLabel("Password", { exact: true }).fill(pw)
  await page.getByLabel("Confirm password").fill(confirm)
  await page.getByRole("button", { name: "Register" }).click()
}

/** 填写并提交登录表单。 */
async function tryLogin(page, email, pw) {
  await page.getByTestId("login-email-input").fill(email)
  await page.getByTestId("login-password-input").fill(pw)
  await page.getByTestId("login-submit-button").click()
}

try {
  const context = await browser.newContext({
    locale: "en-US",
    viewport: { width: 1280, height: 800 },
  })
  const page = await newPage(context)

  // 1) 未登录访问受保护页面 -> 跳到登录页
  await page.goto(`${BASE}/app/dashboard`)
  await page.waitForURL("**/login", { timeout: 8000 }).catch(() => {})
  check("未登录访问 /app/dashboard 被带到 /login", page.url().endsWith("/login"), page.url())
  await shot(page, "01-login")

  // 2) 注册：密码太短时只做客户端校验，不会请求后端
  await page.goto(`${BASE}/register`)
  let registerRequests = 0
  page.on("request", (request) => {
    if (request.url().includes("/api/auth/register")) registerRequests++
  })
  await fillRegister(page, emailA, "short")
  const sawLengthHint = await appears(page, "Password must be at least 8 characters", 5000)
  check(
    "密码少于 8 位：显示校验提示且没有请求后端",
    sawLengthHint && registerRequests === 0,
    `hint=${sawLengthHint} requests=${registerRequests}`
  )
  await shot(page, "02-register-validation")

  // 3) 注册成功 -> 自动登录 -> 进入 dashboard 并显示邮箱
  await page.getByLabel("Password", { exact: true }).fill(password)
  await page.getByLabel("Confirm password").fill(password)
  await page.getByRole("button", { name: "Register" }).click()
  await page.waitForURL("**/app/dashboard", { timeout: 15000 }).catch(() => {})
  check("注册后自动登录并进入 /app/dashboard", page.url().endsWith("/app/dashboard"), page.url())
  check("dashboard 显示用户邮箱（后端返回结构已适配）", await appears(page, emailA))
  await shot(page, "03-dashboard")

  // 4) Cookie 属性：HttpOnly、SameSite=Lax，JavaScript 读不到
  const cookies = await context.cookies(BASE)
  const session = cookies.find((cookie) => cookie.name === "session")
  check("存在 session Cookie", !!session)
  check("Cookie 是 HttpOnly", session?.httpOnly === true)
  check("Cookie 是 SameSite=Lax", session?.sameSite === "Lax", String(session?.sameSite))
  check("Cookie 的 Path 是 /", session?.path === "/")
  const visibleToJs = await page.evaluate(() => document.cookie)
  check("document.cookie 读不到会话令牌（防 XSS 窃取）", !visibleToJs.includes("session="), visibleToJs)

  // 5) 刷新后仍保持登录
  await page.reload()
  check("刷新页面后仍是登录状态", await appears(page, emailA))

  // 6) 退出 -> 回首页，且再访问受保护页面要重新登录
  await page.goto(`${BASE}/app/settings`)
  await page.getByRole("button", { name: "Sign out" }).click()
  await page.waitForURL(`${BASE}/`, { timeout: 8000 }).catch(() => {})
  check("退出后回到首页", page.url() === `${BASE}/`, page.url())
  await page.goto(`${BASE}/app/dashboard`)
  await page.waitForURL("**/login", { timeout: 8000 }).catch(() => {})
  check("退出后访问 /app/dashboard 又被带到 /login", page.url().endsWith("/login"), page.url())

  // 7) 登录失败：显示翻译后的错误，并且页面没有被整体刷新（401 拦截器不能吞掉错误提示）
  await page.goto(`${BASE}/login`)
  await page.evaluate(() => {
    window.__marker = "still-here"
  })
  await tryLogin(page, emailA, "wrong-password-1")
  check("密码错误：显示 “Incorrect email or password”", await appears(page, "Incorrect email or password", 5000))
  check(
    "密码错误时页面没有被整体刷新（401 拦截器不再吞掉错误提示）",
    (await page.evaluate(() => window.__marker)) === "still-here"
  )
  await shot(page, "04-login-error")

  // 8) 连续失败触发限流（后端把账号上限调成了 3）：第 4 次即使密码正确也被拒
  await tryLogin(page, emailA, "wrong-password-2")
  await page.waitForTimeout(400)
  await tryLogin(page, emailA, "wrong-password-3")
  await page.waitForTimeout(400)
  await tryLogin(page, emailA, password)
  check(
    "连续失败后显示限流提示（含等待分钟数）",
    await appears(page, "Too many attempts. Please try again in 15 minutes.", 5000)
  )
  check("被限流时即使密码正确也没有登录成功", page.url().endsWith("/login"), page.url())
  await shot(page, "05-throttled")

  // 9) 中文界面：同一个错误按当前语言翻译
  await context.addCookies([{ name: "NEXT_LOCALE", value: "zh", url: BASE }])
  await page.goto(`${BASE}/login`)
  await tryLogin(page, emailA, password)
  check("中文界面下限流提示是中文", await appears(page, "尝试次数过多，请 15 分钟后再试", 5000))
  await shot(page, "06-throttled-zh")

  // 10) 邮箱已被注册 -> 提示（新的无 Cookie 上下文，英文）
  const otherContext = await browser.newContext({ locale: "en-US" })
  const otherPage = await newPage(otherContext)
  await otherPage.goto(`${BASE}/register`)
  await fillRegister(otherPage, emailA.toUpperCase(), password)
  check("重复注册（大小写不同）提示邮箱已注册", await appears(otherPage, "This email is already registered"))

  // 11) 另一个账号不受影响：账号维度的限流不会殃及别人
  await otherPage.goto(`${BASE}/register`)
  await fillRegister(otherPage, emailB, password)
  await otherPage.waitForURL("**/app/dashboard", { timeout: 15000 }).catch(() => {})
  check("账号 A 被限流时，账号 B 仍能正常注册并登录", otherPage.url().endsWith("/app/dashboard"), otherPage.url())
  await otherContext.close()
  await context.close()

  // 4xx 响应会让浏览器打一条 "Failed to load resource"，属预期；其余控制台错误都不应出现。
  const unexpectedErrors = consoleErrors.filter((text) => !text.includes("Failed to load resource"))
  check("浏览器控制台没有意外的 JS 错误", unexpectedErrors.length === 0, unexpectedErrors.slice(0, 3).join(" | "))
} finally {
  await browser.close()
}

const failed = results.filter((result) => !result.ok)
console.log(`\n${results.length - failed.length}/${results.length} passed`)
process.exit(failed.length ? 1 : 0)

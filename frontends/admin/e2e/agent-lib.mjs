// Agent 服务真实端到端场景的共享工具。
//
// 不要直接运行：由 scripts/agent-e2e.sh 启动完整栈后调用场景脚本。
// 这里只做断言、真实 HTTP 请求、SQL 独立查询与浏览器操作；
// 不伪造身份，也不跳过任何 Agent 服务入口。

import { execFileSync } from "node:child_process"
import { mkdirSync } from "node:fs"
import { dirname } from "node:path"

import { chromium } from "playwright"

export const REPO_ROOT = process.env.REPO_ROOT
export const PUBLIC_URL = process.env.PUBLIC_URL
export const ADMIN_URL = process.env.ADMIN_URL
export const TRACING_UI_URL = process.env.TRACING_UI_URL
export const AGENT_SERVICE_URL = process.env.AGENT_SERVICE_URL
export const TRACING_API_URL = process.env.TRACING_API_URL
export const AGENT_SERVICE_TOKEN = process.env.AGENT_SERVICE_TOKEN
export const A2A_ENDPOINT = process.env.A2A_ENDPOINT
export const A2A_STATE_URL = process.env.A2A_STATE_URL
export const A2A_CREDENTIAL_ALIAS = process.env.A2A_CREDENTIAL_ALIAS
export const ADMIN_USERNAME = process.env.ADMIN_USERNAME
export const ADMIN_PASSWORD = process.env.ADMIN_PASSWORD
export const ADMIN_USER_ID = process.env.ADMIN_USER_ID
export const ALT_ACCOUNTS_URL = process.env.ALT_ACCOUNTS_URL
export const ALT_ADMIN_USERNAME = process.env.ALT_ADMIN_USERNAME
export const ALT_ADMIN_PASSWORD = process.env.ALT_ADMIN_PASSWORD
export const ALT_ADMIN_USER_ID = process.env.ALT_ADMIN_USER_ID
export const MEMBER_EMAIL = process.env.MEMBER_EMAIL
export const MEMBER_PASSWORD = process.env.MEMBER_PASSWORD
export const MEMBER_USER_ID = process.env.MEMBER_USER_ID
export const OTHER_EMAIL = process.env.OTHER_EMAIL
export const OTHER_PASSWORD = process.env.OTHER_PASSWORD
export const OTHER_USER_ID = process.env.OTHER_USER_ID
export const SCENARIO_STAMP = process.env.SCENARIO_STAMP
export const AGENT_LOG_FILE = process.env.AGENT_LOG_FILE
export const TRACING_LOG_FILE = process.env.TRACING_LOG_FILE
export const TRACING_PID_FILE = process.env.TRACING_PID_FILE
export const TRACING_START_SCRIPT = process.env.TRACING_START_SCRIPT
export const SAMPLE_SERVICE_URL = process.env.SAMPLE_SERVICE_URL
export const SAMPLE_SERVICE_TOKEN = process.env.SAMPLE_SERVICE_TOKEN
export const REAL_RUNTIME_ENABLED = process.env.REAL_RUNTIME_ENABLED
export const REAL_RUNTIME_URL = process.env.REAL_RUNTIME_URL
export const REAL_RUNTIME_VERSION = process.env.REAL_RUNTIME_VERSION
export const REAL_RUNTIME_CREDENTIAL_ALIAS = process.env.REAL_RUNTIME_CREDENTIAL_ALIAS
export const REAL_RUNTIME_SOURCE = process.env.REAL_RUNTIME_SOURCE

export const results = []

/** 要求一组环境变量存在，否则直接退出（避免在空配置上产生假通过）。 */
export function requireEnv(names) {
  const missing = names.filter((name) => !process.env[name])
  if (missing.length > 0) {
    console.error(`缺少环境变量：${missing.join(", ")}；请用 just agent-e2e 运行`)
    process.exit(2)
  }
}

/** 记录并打印一条检查结果。 */
export function check(name, ok, detail = "") {
  results.push({ name, ok })
  console.log(`${ok ? "PASS" : "FAIL"}  ${name}${ok ? "" : `   <-- ${detail}`}`)
}

/** 输出汇总并返回进程退出码。 */
export function exitCode() {
  const failed = results.filter((result) => !result.ok)
  console.log(`\n${results.length - failed.length}/${results.length} 项通过`)
  return failed.length === 0 ? 0 : 1
}

export const sleep = (milliseconds) => new Promise((resolve) => setTimeout(resolve, milliseconds))

/** 轮询直到条件成立或超时。 */
export async function eventually(predicate, timeout = 20000, interval = 200) {
  const deadline = Date.now() + timeout
  let lastError = null
  while (Date.now() < deadline) {
    try {
      if (await predicate()) return true
    } catch (error) {
      lastError = error
    }
    await sleep(interval)
  }
  try {
    return Boolean(await predicate())
  } catch (error) {
    lastError = error
    return false
  } finally {
    if (lastError) console.error(`eventually 最后一次异常：${lastError}`)
  }
}

/** 启动浏览器；优先使用系统 Chrome，与既有 e2e 一致。 */
export async function launchBrowser() {
  const launchOptions = { headless: true }
  if (process.env.CHROME_PATH) launchOptions.executablePath = process.env.CHROME_PATH
  else launchOptions.channel = "chrome"
  return chromium.launch(launchOptions)
}

/** 新建页面并收集控制台错误。 */
export async function newPage(context, consoleErrors = []) {
  const page = await context.newPage()
  page.on("console", (message) => {
    if (message.type() === "error") consoleErrors.push(message.text())
  })
  page.on("pageerror", (error) => consoleErrors.push(String(error)))
  return page
}

/** 保存截图到给定路径。 */
export async function shot(page, path) {
  mkdirSync(dirname(path), { recursive: true })
  await page.screenshot({ path })
  console.log(`SHOT  ${path}`)
}

/** 等待可见文本出现；超时返回 false，由调用方的 check 判定。 */
export async function appears(page, text, timeout = 10000) {
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

/** 登录管理后台。 */
export async function loginAdmin(page) {
  await page.goto(`${ADMIN_URL}/sign-in`)
  await page.getByTestId("admin-login-identifier-input").fill(ADMIN_USERNAME)
  await page.getByTestId("admin-login-password-input").fill(ADMIN_PASSWORD)
  await page.getByTestId("admin-login-submit-button").click()
  await page.waitForURL((url) => !url.pathname.includes("/sign-in"), { timeout: 15000 })
}

/** 登录公开站点。 */
export async function loginUser(page, email, password) {
  await page.goto(`${PUBLIC_URL}/login`)
  await page.getByTestId("login-email-input").fill(email)
  await page.getByTestId("login-password-input").fill(password)
  await page.getByTestId("login-submit-button").click()
  await page.waitForURL((url) => !url.pathname.includes("/login"), { timeout: 15000 })
}

/** 在 Playwright 上下文中取某个 Cookie 的 `name=value` 形式。 */
export async function cookieHeader(context, name) {
  const cookies = await context.cookies()
  const cookie = cookies.find((candidate) => candidate.name === name)
  return cookie ? `${cookie.name}=${cookie.value}` : ""
}

/** 选择 Radix Select 的选项。 */
export async function selectRadix(page, testId, optionName) {
  await page.getByTestId(testId).click()
  await page.getByRole("option", { name: optionName, exact: true }).click()
}

/** 基础 HTTP 请求；返回状态码、解析后的 JSON、原始文本与 Set-Cookie。 */
export async function http(base, path, options = {}) {
  const headers = { ...(options.headers ?? {}) }
  if (options.token) headers.authorization = `Bearer ${options.token}`
  if (options.cookie) headers.cookie = options.cookie
  let body
  if (options.formData) body = options.formData
  else if (options.body !== undefined) {
    headers["content-type"] = "application/json"
    body = JSON.stringify(options.body)
  }
  const method = options.method ?? (body === undefined ? "GET" : "POST")
  const response = await fetch(`${base}${path}`, {
    method,
    headers,
    body,
    redirect: "manual",
    signal: AbortSignal.timeout(options.timeoutMs ?? 30000),
  })
  const text = await response.text()
  let parsed = null
  if (text) {
    try {
      parsed = JSON.parse(text)
    } catch {
      parsed = null
    }
  }
  const setCookie = response.headers.getSetCookie?.() ?? []
  return { status: response.status, body: parsed, text, headers: response.headers, setCookie }
}

/** 直接调用 Agent public API（服务凭据 + 用户 Cookie）。 */
export function agentDirect(path, options = {}) {
  return http(AGENT_SERVICE_URL, path, { ...options, token: AGENT_SERVICE_TOKEN })
}

/** 直接调用 Agent admin API（服务凭据 + 管理员 Cookie）。 */
export function agentAdminDirect(path, options = {}) {
  return http(AGENT_SERVICE_URL, path, { ...options, token: AGENT_SERVICE_TOKEN })
}

/** 经公开站点 Next 代理调用 Agent public API。 */
export function publicProxy(path, options = {}) {
  return http(PUBLIC_URL, `/api/agent/v1${path}`, options)
}

/** 经管理后台 Vite 代理调用 Agent admin API。 */
export function adminProxy(path, options = {}) {
  return http(ADMIN_URL, `/agent-api${path}`, options)
}

/** 直接调用 tracing admin API（管理员 Cookie）。 */
export function tracingApi(path, cookie, options = {}) {
  return http(TRACING_API_URL, path, { ...options, cookie })
}

/** 通过管理后台登录代理获取管理员 Cookie。 */
export async function adminLoginHttp() {
  const login = await http(ADMIN_URL, "/api/admin/auth/login", {
    body: { identifier: ADMIN_USERNAME, password: ADMIN_PASSWORD },
  })
  const cookieLine = login.setCookie.find((value) => value.startsWith("admin_session="))
  return {
    status: login.status,
    cookie: cookieLine ? cookieLine.split(";")[0] : "",
  }
}

/** 通过公开站点登录代理获取用户 Cookie。 */
export async function userLoginHttp(email, password) {
  const login = await http(PUBLIC_URL, "/api/auth/login", { body: { email, password } })
  const cookieLine = login.setCookie.find((value) => value.startsWith("session="))
  return {
    status: login.status,
    cookie: cookieLine ? cookieLine.split(";")[0] : "",
  }
}

/** 通过第二个 accounts 实例登录非平台管理员。 */
export async function altAdminLoginHttp() {
  const login = await http(ALT_ACCOUNTS_URL, "/admin/auth/login", {
    body: { identifier: ALT_ADMIN_USERNAME, password: ALT_ADMIN_PASSWORD },
  })
  const cookieLine = login.setCookie.find((value) => value.startsWith("admin_session="))
  return {
    status: login.status,
    cookie: cookieLine ? cookieLine.split(";")[0] : "",
  }
}

function psql(container, user, database, sql) {
  return execFileSync(
    "docker",
    [
      "compose",
      "-f",
      `${REPO_ROOT}/docker-compose.testing.yml`,
      "exec",
      "-T",
      container,
      "psql",
      "-U",
      user,
      "-d",
      database,
      "-tAc",
      sql,
    ],
    { encoding: "utf8" },
  ).trim()
}

/** 在 Agent 测试库上执行只读/场景准备 SQL，返回单值文本。 */
export function psqlAgent(sql) {
  return psql("agent-postgres", "agent", "agent_dev", sql)
}

/** 在 tracing 测试库上执行只读 SQL，返回单值文本。 */
export function psqlTracing(sql) {
  return psql("tracing-postgres", "agent_tracing", "agent_tracing_dev", sql)
}

/** 读取 Agent 库的单个标量。 */
export function agentScalar(sql) {
  return psqlAgent(sql)
}

/** 读取 Agent 库的 JSON 聚合结果。 */
export function agentJson(sql) {
  const value = psqlAgent(sql)
  return value ? JSON.parse(value) : null
}

/** 读取 tracing 库的单个标量。 */
export function tracingScalar(sql) {
  return psqlTracing(sql)
}

/** 读取 tracing 库的 JSON 聚合结果。 */
export function tracingJson(sql) {
  const value = psqlTracing(sql)
  return value ? JSON.parse(value) : null
}

/** 读取文件文本（用于日志断言）。 */
export function readTextFile(path) {
  try {
    return execFileSync("cat", [path], { encoding: "utf8" })
  } catch {
    return ""
  }
}

/** 读取 A2A 协议替身的计数器。 */
export async function a2aState() {
  const response = await http(A2A_STATE_URL, "", {})
  return response.body
}

/** 读取 SSE 并返回事件列表；默认在终态事件后停止。 */
export async function readSse(url, cookie, options = {}) {
  const controller = new AbortController()
  const timer = setTimeout(() => controller.abort(), options.timeoutMs ?? 15000)
  const events = []
  try {
    const response = await fetch(url, {
      headers: { accept: "text/event-stream", cookie },
      signal: controller.signal,
    })
    if (!response.ok || !response.body) {
      return { status: response.status, events }
    }
    const reader = response.body.getReader()
    const decoder = new TextDecoder()
    let buffer = ""
    let eventName = ""
    for (;;) {
      const { done, value } = await reader.read()
      if (done) break
      buffer += decoder.decode(value, { stream: true })
      let boundary = buffer.indexOf("\n\n")
      while (boundary >= 0) {
        const chunk = buffer.slice(0, boundary)
        buffer = buffer.slice(boundary + 2)
        let data = ""
        eventName = ""
        for (const line of chunk.split("\n")) {
          if (line.startsWith("event:")) eventName = line.slice(6).trim()
          if (line.startsWith("data:")) data += line.slice(5).trimStart()
        }
        if (data) {
          try {
            events.push({ event: eventName, payload: JSON.parse(data) })
          } catch {
            events.push({ event: eventName, payload: data })
          }
          if (
            options.stopAfterTerminal !== false &&
            ["run.completed", "run.failed", "run.cancelled"].includes(eventName)
          ) {
            return { status: response.status, events }
          }
        }
        boundary = buffer.indexOf("\n\n")
      }
    }
    return { status: response.status, events }
  } catch (error) {
    if (controller.signal.aborted) return { status: 0, events, timedOut: true }
    throw error
  } finally {
    clearTimeout(timer)
  }
}

/** 创建场景可用的管理员与成员 Cookie。 */
export async function prepareAdminSession() {
  const adminLogin = await adminLoginHttp()
  if (adminLogin.status !== 200 || !adminLogin.cookie) {
    throw new Error(`管理员登录失败：${adminLogin.status}`)
  }
  return adminLogin.cookie
}

/** 创建场景可用的成员 Cookie。 */
export async function prepareUserSession(email, password) {
  const userLogin = await userLoginHttp(email, password)
  if (userLogin.status !== 200 || !userLogin.cookie) {
    throw new Error(`用户登录失败：${userLogin.status}`)
  }
  return userLogin.cookie
}

// rv-1：组织管理员接入 Runtime 与 Agent、用户完成 Session/Run 生命周期。
//
// 真实入口：浏览器 → 公开站点 Next 代理 → Agent HTTP API → accounts 身份边界 →
// Agent DB/outbox → tracing producer/query API → tracing 前端。
// 外部 A2A 端点由本地协议替身模拟（仅 endpoint），Agent adapter、持久化与
// tracing 全部走真实服务。

import {
  A2A_CREDENTIAL_ALIAS,
  A2A_ENDPOINT,
  A2A_STATE_URL,
  ADMIN_URL,
  ADMIN_USER_ID,
  AGENT_LOG_FILE,
  AGENT_SERVICE_URL,
  MEMBER_EMAIL,
  MEMBER_PASSWORD,
  MEMBER_USER_ID,
  PUBLIC_URL,
  REPO_ROOT,
  REAL_RUNTIME_CREDENTIAL_ALIAS,
  REAL_RUNTIME_ENABLED,
  REAL_RUNTIME_SOURCE,
  REAL_RUNTIME_URL,
  REAL_RUNTIME_VERSION,
  SCENARIO_STAMP,
  TRACING_UI_URL,
  a2aState,
  adminProxy,
  agentAdminDirect,
  agentDirect,
  agentJson,
  agentScalar,
  appears,
  check,
  cookieHeader,
  eventually,
  exitCode,
  launchBrowser,
  loginAdmin,
  loginUser,
  newPage,
  publicProxy,
  readSse,
  requireEnv,
  selectRadix,
  shot,
  sleep,
  tracingApi,
} from "./agent-lib.mjs"

requireEnv([
  "REPO_ROOT",
  "PUBLIC_URL",
  "ADMIN_URL",
  "TRACING_UI_URL",
  "AGENT_SERVICE_URL",
  "AGENT_SERVICE_TOKEN",
  "A2A_ENDPOINT",
  "A2A_CREDENTIAL_ALIAS",
  "ADMIN_USERNAME",
  "ADMIN_PASSWORD",
  "MEMBER_EMAIL",
  "MEMBER_PASSWORD",
  "MEMBER_USER_ID",
  "SCENARIO_STAMP",
])

const stamp = SCENARIO_STAMP
const shotDir = process.env.SHOT_DIR || `${REPO_ROOT}/tasks/evidence/P1-FEAT-20261030-092543-agent-service-runtime/rv-1`
const slug = `rv1-${stamp}`
const orgDisplay = `RV1 Organization ${stamp}`
const unitName = `RV1 Unit ${stamp}`
const a2aAgentDisplay = "RV1 A2A Agent"
const sandboxParentDisplay = "RV1 Sandbox Parent"
const sandboxChildDisplay = "RV1 Sandbox Child"
const skillInstructions = "Use the RV1 skill instructions when summarizing."
const resourceMarker = `RV1_RESOURCE_MARKER_${stamp}`
const artifactFilename = `rv1-artifact-${stamp}.txt`

const consoleErrors = []
const unexpectedConsole = (text) => !/status of (401|403|404|409)|401 \(|403 \(|404 \(|409 \(/.test(text)

function jsonText(value) {
  return JSON.stringify(value)
}

function runDbRow(runId) {
  return agentJson(
    `SELECT row_to_json(t) FROM (
       SELECT id::text AS run_id, status, origin, agent_id::text, runtime_connection_id::text,
              parent_run_id::text, agent_snapshot_checksum, request_digest,
              idempotency_key, last_event_seq, trace_delivered_through_seq
       FROM agent_runs WHERE id = '${runId}'
     ) t`,
  )
}

async function adminApi(cookie, path) {
  const response = await adminProxy(path, { cookie })
  if (response.status !== 200) throw new Error(`${path} -> ${response.status} ${response.text}`)
  return response.body
}

async function sendPublic(cookie, sessionId, content, idempotencyKey, extra = {}) {
  return publicProxy(`/sessions/${sessionId}/messages`, {
    method: "POST",
    cookie,
    headers: { "idempotency-key": idempotencyKey },
    body: { agent_id: null, content, resource_ids: [], ...extra },
  })
}

async function waitRunTerminal(cookie, runId, timeout = 60000) {
  return eventually(async () => {
    const run = await publicProxy(`/runs/${runId}`, { cookie })
    return run.status === 200 && ["succeeded", "failed", "cancelled"].includes(run.body?.status)
  }, timeout)
}

async function waitTracingExport(cookie, runId, timeout = 60000) {
  let lastStatus = 0
  const ready = await eventually(async () => {
    const response = await tracingApi(`/admin/agent-traces/${runId}/export`, cookie)
    lastStatus = response.status
    return response.status === 200
  }, timeout, 500)
  if (!ready) return { ready: false, lastStatus, export: null, raw: "" }
  const response = await tracingApi(`/admin/agent-traces/${runId}/export`, cookie)
  return { ready: true, lastStatus: response.status, export: response.body, raw: response.text }
}

const browser = await launchBrowser()
try {
  const adminContext = await browser.newContext({ viewport: { width: 1500, height: 1100 } })
  const adminPage = await newPage(adminContext, consoleErrors)
  await loginAdmin(adminPage)
  const adminCookie = await cookieHeader(adminContext, "admin_session")
  check("管理员通过真实登录页进入后台", adminCookie !== "")

  // 1. 组织、部门与显式成员关系。
  await adminPage.goto(`${ADMIN_URL}/organizations`)
  await adminPage.getByTestId("admin-org-create-form").waitFor({ timeout: 20000 })
  await adminPage.getByTestId("admin-org-slug-input").fill(slug)
  await adminPage.getByTestId("admin-org-display-input").fill(orgDisplay)
  await adminPage.getByTestId("admin-org-create-submit").click()
  const orgPickerText = await eventually(async () => {
    const text = await adminPage.getByTestId("agent-organization-select").innerText()
    return text.includes(orgDisplay)
  })
  check("组织创建后出现在管理页组织选择器", orgPickerText)

  const organizations = await adminApi(adminCookie, "/admin/api/organizations")
  const organization = organizations.items.find((item) => item.slug === slug)
  check("组织写入后可独立读取", Boolean(organization), jsonText(organizations))
  const organizationId = organization?.id ?? ""

  await adminPage.getByTestId("admin-org-scope-admin-input").fill(String(ADMIN_USER_ID))
  await adminPage.getByTestId("admin-org-scope-grant").click()
  check(
    "平台管理员通过管理页授予自己组织 scope",
    await eventually(async () => {
      const rows = await adminPage.getByTestId("admin-org-scope-row").allInnerTexts()
      return rows.some((text) => text.includes(String(ADMIN_USER_ID)))
    }, 10000),
  )

  await adminPage.getByTestId("admin-org-unit-name-input").fill(unitName)
  await adminPage.getByTestId("admin-org-unit-create-submit").click()
  check(
    "部门创建后显示在部门列表",
    await eventually(async () => (await adminPage.getByTestId("admin-org-unit-row").filter({ hasText: unitName }).count()) === 1),
  )
  const units = await adminApi(adminCookie, `/admin/api/organizations/${organizationId}/units`)
  const unit = units.items.find((item) => item.name === unitName)
  check("部门写入后可独立读取", Boolean(unit), jsonText(units))

  await adminPage.getByTestId("admin-org-membership-user-input").fill(String(MEMBER_USER_ID))
  await selectRadix(adminPage, "admin-org-membership-unit-select", unitName)
  await adminPage.getByTestId("admin-org-membership-name-input").fill(`RV1 Member ${stamp}`)
  await adminPage.getByTestId("admin-org-membership-submit").click()
  check(
    "显式成员关系显示用户与部门",
    await eventually(async () => {
      const row = adminPage.getByTestId("admin-org-membership-row").filter({ hasText: String(MEMBER_USER_ID) })
      return (await row.count()) === 1 && (await row.innerText()).includes(unitName)
    }),
  )
  const memberships = await adminApi(adminCookie, `/admin/api/organizations/${organizationId}/memberships`)
  const membership = memberships.items.find((item) => item.user_id === Number(MEMBER_USER_ID))
  check(
    "成员关系独立读取到指定部门",
    membership?.unit_id === unit?.id,
    jsonText(memberships),
  )

  // 2. Runtime：内置只读 + 外部 A2A 登记与刷新。
  await adminPage.goto(`${ADMIN_URL}/agent-runtimes`)
  await adminPage.getByTestId("admin-runtime-create-form").waitFor({ timeout: 20000 })
  const pickerText = await adminPage.getByTestId("agent-organization-select").innerText()
  if (!pickerText.includes(orgDisplay)) {
    await selectRadix(adminPage, "agent-organization-select", `${orgDisplay} (${slug})`)
  }
  await adminPage.getByTestId("admin-runtime-row").first().waitFor({ timeout: 20000 })
  const builtinRow = adminPage.getByTestId("admin-runtime-row").filter({ hasText: "platform-sandbox" })
  check(
    "内置 Runtime 以 Built-in / sandbox / Ready 只读展示",
    (await builtinRow.count()) === 1 &&
      (await builtinRow.innerText()).includes("Built-in") &&
      (await builtinRow.innerText()).includes("sandbox") &&
      (await builtinRow.innerText()).includes("Ready"),
  )
  check("内置 Runtime 不提供刷新等写操作", (await builtinRow.getByTestId("admin-runtime-refresh").count()) === 0)

  const externalRuntimeName = `a2a-fixture-${stamp}`
  await adminPage.getByTestId("admin-runtime-name-input").fill(externalRuntimeName)
  await adminPage.getByTestId("admin-runtime-display-input").fill("RV1 A2A Fixture")
  await adminPage.getByTestId("admin-runtime-endpoint-input").fill(A2A_ENDPOINT)
  await adminPage.getByTestId("admin-runtime-credential-input").fill(A2A_CREDENTIAL_ALIAS)
  await adminPage.getByTestId("admin-runtime-create-submit").click()
  const externalRow = adminPage.getByTestId("admin-runtime-row").filter({ hasText: "RV1 A2A Fixture" })
  check(
    "外部 A2A Runtime 登记后状态为 Ready",
    await eventually(async () => (await externalRow.count()) === 1 && (await externalRow.innerText()).includes("Ready"), 30000),
  )
  await externalRow.getByTestId("admin-runtime-refresh").click()
  check(
    "刷新外部 Runtime 后仍为 Ready",
    await eventually(async () => (await externalRow.innerText()).includes("Ready"), 30000),
  )
  const runtimes = await adminApi(adminCookie, `/admin/api/organizations/${organizationId}/runtimes`)
  const builtin = runtimes.items.find((item) => item.name === "platform-sandbox")
  const external = runtimes.items.find((item) => item.name === externalRuntimeName)
  check(
    "Runtime 独立读取到 builtin 与 external 的来源/协议",
    builtin?.origin === "builtin" &&
      builtin?.adapter_kind === "sandbox" &&
      external?.origin === "external" &&
      external?.adapter_kind === "a2a" &&
      external?.credential_alias === A2A_CREDENTIAL_ALIAS,
    jsonText(runtimes),
  )
  const builtinId = builtin?.id ?? ""
  const externalId = external?.id ?? ""

  // 3. 通过管理页创建绑定外部 Runtime 的 Agent，并配置授权与 Auto 入口。
  await adminPage.goto(`${ADMIN_URL}/agents`)
  await adminPage.getByTestId("admin-agent-create-form").waitFor({ timeout: 20000 })
  const agentPickerText = await adminPage.getByTestId("agent-organization-select").innerText()
  if (!agentPickerText.includes(orgDisplay)) {
    await selectRadix(adminPage, "agent-organization-select", `${orgDisplay} (${slug})`)
  }
  await selectRadix(adminPage, "admin-agent-runtime-select", "RV1 A2A Fixture (a2a)")
  await adminPage.getByTestId("admin-agent-name-input").fill(`rv1-a2a-${stamp}`)
  await adminPage.getByTestId("admin-agent-display-input").fill(a2aAgentDisplay)
  await adminPage.getByTestId("admin-agent-description-input").fill("RV1 A2A lifecycle agent")
  await adminPage.getByTestId("admin-agent-prompt-input").fill("You are the RV1 A2A lifecycle agent.")
  await adminPage.getByTestId("admin-agent-create-submit").click()
  const a2aAgentRow = adminPage.getByTestId("admin-agent-row").filter({ hasText: a2aAgentDisplay })
  check("Agent 创建后绑定所选外部 Runtime", await eventually(async () => (await a2aAgentRow.count()) === 1, 30000))
  await a2aAgentRow.getByTestId("admin-agent-select").click()
  await adminPage.getByTestId("admin-agent-detail").waitFor({ timeout: 20000 })

  await adminPage.getByTestId("admin-agent-rule-add").click()
  await selectRadix(adminPage, "admin-agent-rule-kind", "Organization")
  await adminPage.getByTestId("admin-agent-rules-save").click()
  check("组织访问规则通过管理页保存", await appears(adminPage, "Saved", 10000))

  await selectRadix(adminPage, "admin-agent-entry-visible", "Yes")
  await adminPage.getByTestId("admin-agent-entry-priority").fill("10")
  await adminPage.getByTestId("admin-agent-entry-save").click()
  check("Auto 入口策略保存成功", await appears(adminPage, "Saved", 10000))

  const agentsList = await adminApi(adminCookie, `/admin/api/organizations/${organizationId}/agents`)
  const a2aAgent = agentsList.items.find((item) => item.display_name === a2aAgentDisplay)
  check("Agent 独立读取到绑定外部 Runtime", Boolean(a2aAgent) && a2aAgent?.runtime_connection_id === externalId, jsonText(agentsList))
  const a2aAgentId = a2aAgent?.id ?? ""
  const overview = await adminApi(adminCookie, `/admin/api/organizations/${organizationId}/agents/${a2aAgentId}/overview`)
  check(
    "Agent 独立读取到组织 allow 规则与 Auto 入口",
    overview.access_rules.some(
      (rule) => rule.effect === "allow" && rule.subject.kind === "organization" && rule.subject.id === organizationId,
    ) && overview.entry_policy?.auto_visible === true,
    jsonText(overview),
  )

  // 4. 通过管理 API 准备 Sandbox 父子 Agent、Skill 与委派策略。
  async function createAgentApi(body) {
    const response = await adminProxy(`/admin/api/organizations/${organizationId}/agents`, {
      method: "POST",
      cookie: adminCookie,
      body,
    })
    if (response.status !== 201) throw new Error(`创建 Agent 失败：${response.status} ${response.text}`)
    return response.body
  }
  const childAgent = await createAgentApi({
    runtime_connection_id: builtinId,
    name: `rv1-child-${stamp}`,
    display_name: sandboxChildDisplay,
    description: "RV1 sandbox child",
    prompt: "You are the RV1 sandbox child.",
  })
  const parentAgent = await createAgentApi({
    runtime_connection_id: builtinId,
    name: `rv1-parent-${stamp}`,
    display_name: sandboxParentDisplay,
    description: "RV1 sandbox parent",
    prompt: "You are the RV1 sandbox parent.",
  })
  for (const agentId of [childAgent.id, parentAgent.id]) {
    const rules = await adminProxy(`/admin/api/organizations/${organizationId}/agents/${agentId}/access-rules`, {
      method: "PUT",
      cookie: adminCookie,
      body: { rules: [{ effect: "allow", subject: { kind: "organization", id: organizationId } }] },
    })
    if (rules.status !== 200) throw new Error(`写入访问规则失败：${rules.status} ${rules.text}`)
    const entry = await adminProxy(`/admin/api/organizations/${organizationId}/agents/${agentId}/entry-policy`, {
      method: "PUT",
      cookie: adminCookie,
      body: { auto_visible: true, priority: 20 },
    })
    if (entry.status !== 200) throw new Error(`写入入口策略失败：${entry.status} ${entry.text}`)
  }
  const delegation = await adminProxy(
    `/admin/api/organizations/${organizationId}/agents/${parentAgent.id}/delegation-policy`,
    {
      method: "PUT",
      cookie: adminCookie,
      body: { child_agent_ids: [childAgent.id], max_depth: 1, max_child_runs: 2 },
    },
  )
  check("父 Agent 委派策略保存成功", delegation.status === 200, delegation.text)

  const skillResponse = await adminProxy(`/admin/api/organizations/${organizationId}/skills`, {
    method: "POST",
    cookie: adminCookie,
    body: { name: `rv1-skill-${stamp}`, description: "RV1 skill", instructions: skillInstructions },
  })
  check("Skill 创建成功", skillResponse.status === 201, skillResponse.text)
  const skillId = skillResponse.body?.id ?? ""
  const skillAssign = await adminProxy(`/admin/api/organizations/${organizationId}/agents/${parentAgent.id}/skills`, {
    method: "PUT",
    cookie: adminCookie,
    body: { skill_ids: [skillId] },
  })
  check("Skill 授权给父 Agent", skillAssign.status === 200, skillAssign.text)

  // 5. 用户通过公开站点完成 Session/Run。
  const memberContext = await browser.newContext({ viewport: { width: 1500, height: 1100 } })
  const memberPage = await newPage(memberContext, consoleErrors)
  await loginUser(memberPage, MEMBER_EMAIL, MEMBER_PASSWORD)
  const memberCookie = await cookieHeader(memberContext, "session")
  check("成员通过公开站点登录拿到用户会话", memberCookie !== "")
  const proxyOrganizations = await publicProxy("/organizations", { cookie: memberCookie })
  check(
    "公开站点代理携带用户会话读取组织目录",
    proxyOrganizations.status === 200 && (proxyOrganizations.body?.items?.length ?? 0) > 0,
    `${proxyOrganizations.status} ${proxyOrganizations.text}`,
  )

  await memberPage.goto(`${PUBLIC_URL}/app/agents`)
  await memberPage.getByTestId("agent-org-select").waitFor({ timeout: 30000 })
  const orgOptionsLoaded = await eventually(async () => {
    const options = await memberPage.getByTestId("agent-org-select").locator("option").count()
    return options > 0
  }, 30000)
  check(
    "公开工作台加载组织目录",
    orgOptionsLoaded,
    (await memberPage.locator("body").innerText()).slice(0, 800),
  )
  await memberPage.getByTestId("agent-org-select").selectOption(organizationId)
  check(
    "公开工作台列出组织内可用 Agent",
    await eventually(async () => {
      const options = await memberPage.getByTestId("agent-select").locator("option").allInnerTexts()
      return options.some((text) => text.includes(a2aAgentDisplay)) && options.some((text) => text.includes(sandboxParentDisplay))
    }, 30000),
  )
  await memberPage.getByTestId("agent-select").selectOption(a2aAgentId)
  await memberPage.getByTestId("agent-new-session").click()
  check(
    "创建 Session 后消息输入可用",
    await eventually(async () => !(await memberPage.getByTestId("agent-message-input").isDisabled()), 20000),
  )

  const sessions = await publicProxy(`/organizations/${organizationId}/sessions`, { cookie: memberCookie })
  const session = sessions.body?.items?.[0]
  check("Session 独立读取成功", sessions.status === 200 && Boolean(session?.id), sessions.text)
  const sessionId = session?.id ?? ""

  const firstMessage = `hello rv1 ${stamp}`
  await memberPage.getByTestId("agent-message-input").fill(firstMessage)
  await memberPage.getByTestId("agent-send").click()
  const firstSucceeded = await eventually(
    async () => (await memberPage.getByTestId("agent-run-status").innerText().catch(() => "")) === "succeeded",
    60000,
  )
  const firstRunId = await memberPage.getByTestId("agent-run-id").innerText()
  check("首次 Run 在 UI 到达 succeeded", firstSucceeded, await memberPage.getByTestId("agent-run-status").innerText().catch(() => ""))
  check("助手回复通过真实 A2A adapter 显示", await appears(memberPage, "E2E A2A reply", 15000))
  await shot(memberPage, `${shotDir}/user-session-run.png`)

  const firstRunApi = await agentDirect(`/api/v1/runs/${firstRunId}`, { cookie: memberCookie })
  const firstRunDb = runDbRow(firstRunId)
  check(
    "UI Run ID 与 Agent API/DB 一致且状态成功",
    firstRunApi.status === 200 && firstRunApi.body.status === "succeeded" && firstRunDb?.status === "succeeded",
    jsonText({ api: firstRunApi.body?.status, db: firstRunDb }),
  )
  check(
    "首个 Run 快照绑定创建时选定的外部 Runtime",
    firstRunApi.body?.snapshot?.agent?.runtime?.runtime_id === externalId &&
      firstRunApi.body?.snapshot?.agent?.runtime?.adapter_kind === "a2a",
    jsonText(firstRunApi.body?.snapshot?.agent?.runtime),
  )

  const tracingFirst = await waitTracingExport(adminCookie, firstRunId)
  const firstSeqs = tracingFirst.export?.events?.map((event) => event.seq) ?? []
  check(
    "tracing 独立查询到同一 Run 的完整有序事件",
    tracingFirst.ready &&
      tracingFirst.export?.run_id === firstRunId &&
      tracingFirst.export?.status === "succeeded" &&
      firstSeqs.length === tracingFirst.export?.event_count &&
      firstSeqs.every((seq, index) => seq === index + 1) &&
      new Set(firstSeqs).size === firstSeqs.length &&
      tracingFirst.export?.events?.at(-1)?.event_type === "run.completed",
    `${tracingFirst.lastStatus} ${firstSeqs.join(",")}`,
  )

  const sse = await readSse(
    `${PUBLIC_URL}/api/agent/v1/runs/${firstRunId}/stream?after_seq=${Math.max(1, firstSeqs.length - 1)}`,
    memberCookie,
  )
  check(
    "SSE 从最近持久序号续读并返回终态事件",
    sse.status === 200 &&
      sse.events.length === 1 &&
      sse.events[0].event === "run.completed",
    jsonText(sse.events),
  )

  // 6. 同一幂等键返回同一 Run，且不重复消息或事件。
  const idempotencyKey = `rv1-idem-${stamp}`
  const idemContent = `idempotent rv1 ${stamp}`
  const runsBefore = Number(agentScalar(`SELECT count(*) FROM agent_runs WHERE session_id = '${sessionId}'`))
  const messagesBefore = Number(agentScalar(`SELECT count(*) FROM chat_messages WHERE session_id = '${sessionId}'`))
  const firstSend = await sendPublic(memberCookie, sessionId, idemContent, idempotencyKey)
  check(
    "首次幂等请求创建 Run",
    firstSend.status === 201 && firstSend.body?.created === true,
    `${firstSend.status} ${firstSend.text}`,
  )
  const idemRunId = firstSend.body?.run?.run_id ?? ""
  await waitRunTerminal(memberCookie, idemRunId, 60000)
  const eventsAfterFirst = Number(agentScalar(`SELECT count(*) FROM agent_run_events WHERE run_id = '${idemRunId}'`))
  const replay = await sendPublic(memberCookie, sessionId, idemContent, idempotencyKey)
  await sleep(800)
  const runsAfter = Number(agentScalar(`SELECT count(*) FROM agent_runs WHERE session_id = '${sessionId}'`))
  const messagesAfter = Number(agentScalar(`SELECT count(*) FROM chat_messages WHERE session_id = '${sessionId}'`))
  const eventsAfterReplay = Number(agentScalar(`SELECT count(*) FROM agent_run_events WHERE run_id = '${idemRunId}'`))
  check(
    "同一幂等键返回同一 Run 且不新增记录",
    replay.status === 200 &&
      replay.body?.created === false &&
      replay.body?.run?.run_id === idemRunId &&
      runsAfter === runsBefore + 1 &&
      messagesAfter === messagesBefore + 2 &&
      eventsAfterReplay === eventsAfterFirst,
    jsonText({ replay: replay.body, runsBefore, runsAfter, messagesBefore, messagesAfter, eventsAfterFirst, eventsAfterReplay }),
  )

  // 7. 用户从 UI 取消 SLOW Run；取消后不再产生新事件。
  const fixtureBefore = await a2aState()
  const slowMessage = `SLOW rv1 ${stamp}`
  await memberPage.getByTestId("agent-message-input").fill(slowMessage)
  await memberPage.getByTestId("agent-send").click()
  let slowRunId = ""
  const slowAppeared = await eventually(async () => {
    const text = await memberPage.getByTestId("agent-run-id").innerText().catch(() => "")
    if (text !== firstRunId && text.length > 0) {
      slowRunId = text
      return true
    }
    return false
  }, 20000)
  check("SLOW Run 出现在 UI", slowAppeared && Boolean(slowRunId), slowRunId || "未读取到新的 Run ID")
  const active = await eventually(async () => {
    const status = await memberPage.getByTestId("agent-run-status").innerText().catch(() => "")
    return ["running", "cancelling"].includes(status)
  }, 30000)
  const slowUiStatus = await memberPage.getByTestId("agent-run-status").innerText().catch(() => "")
  const slowDbEvents = agentScalar(
    `SELECT coalesce(string_agg(event_type, ',' ORDER BY seq), '') FROM agent_run_events WHERE run_id = '${slowRunId}'`,
  )
  check(
    "SLOW Run 进入进行中状态",
    active,
    jsonText({ uiStatus: slowUiStatus, dbStatus: runDbRow(slowRunId)?.status, dbEvents: slowDbEvents }),
  )
  const cancelEnabled = await eventually(async () => !(await memberPage.getByTestId("agent-cancel-run").isDisabled()), 30000)
  check("取消按钮在 Run 进行中可用", cancelEnabled)
  await memberPage.getByTestId("agent-cancel-run").click()
  const cancelled = await eventually(
    async () => (await memberPage.getByTestId("agent-run-status").innerText().catch(() => "")) === "cancelled",
    30000,
  )
  check("UI 取消后 Run 到达 cancelled", cancelled)
  const slowEventCount = Number(agentScalar(`SELECT count(*) FROM agent_run_events WHERE run_id = '${slowRunId}'`))
  await sleep(2500)
  const slowEventCountLater = Number(agentScalar(`SELECT count(*) FROM agent_run_events WHERE run_id = '${slowRunId}'`))
  const fixtureAfter = await a2aState()
  const slowDb = runDbRow(slowRunId)
  const slowEvents = agentJson(
    `SELECT json_agg(json_build_object('seq', seq, 'event_type', event_type) ORDER BY seq)::text
       FROM agent_run_events WHERE run_id = '${slowRunId}'`,
  )
  check(
    "取消确认传到外部 Runtime 且不再启动新工作",
    slowDb?.status === "cancelled" &&
      slowEventCountLater === slowEventCount &&
      slowEvents?.at(-1)?.event_type === "run.cancelled" &&
      fixtureAfter.cancelTaskCalls > fixtureBefore.cancelTaskCalls,
    jsonText({ slowDb, slowEventCount, slowEventCountLater, events: slowEvents, fixtureBefore, fixtureAfter }),
  )

  // 8. 负控：协议替身返回受控失败，真实 adapter 必须写 Failed 终态。
  const failRunResponse = await sendPublic(memberCookie, sessionId, `FAIL rv1 ${stamp}`, `rv1-fail-${stamp}`)
  const failRunId = failRunResponse.body?.run?.run_id ?? ""
  check("失败场景 Run 创建成功", failRunResponse.status === 201 && Boolean(failRunId), failRunResponse.text)
  const failTerminal = await waitRunTerminal(memberCookie, failRunId, 60000)
  const failRun = await publicProxy(`/runs/${failRunId}`, { cookie: memberCookie })
  const failDb = runDbRow(failRunId)
  const failEvents = agentJson(
    `SELECT json_agg(json_build_object('seq', seq, 'event_type', event_type, 'payload', payload) ORDER BY seq)::text
       FROM agent_run_events WHERE run_id = '${failRunId}'`,
  )
  check(
    "受控 A2A 失败被真实 adapter 写成 Failed 终态",
    failTerminal &&
      failRun.body?.status === "failed" &&
      failDb?.status === "failed" &&
      failEvents?.at(-1)?.event_type === "run.failed" &&
      Boolean(failRun.body?.error_code),
    jsonText({ status: failRun.body?.status, error: failRun.body?.error_code, db: failDb, events: failEvents }),
  )
  const failTrace = await waitTracingExport(adminCookie, failRunId)
  check(
    "失败 Run 的 tracing 状态也是 failed",
    failTrace.ready && failTrace.export?.status === "failed" && failTrace.export?.events?.at(-1)?.event_type === "run.failed",
    `${failTrace.lastStatus} ${failTrace.export?.status ?? ""}`,
  )

  // 9. Agent 更新不改写历史快照；后续 Run 使用新快照。
  const runsBeforeUpdate = Number(agentScalar(`SELECT count(*) FROM agent_runs WHERE agent_id = '${a2aAgentId}'`))
  await adminPage.goto(`${ADMIN_URL}/agents`)
  await adminPage.getByTestId("admin-agent-create-form").waitFor({ timeout: 20000 })
  await adminPage.getByTestId("admin-agent-row").filter({ hasText: a2aAgentDisplay }).getByTestId("admin-agent-select").click()
  await adminPage.getByTestId("admin-agent-detail").waitFor({ timeout: 20000 })
  await adminPage.getByTestId("admin-agent-detail-prompt").fill(`Updated RV1 prompt ${stamp}`)
  await adminPage.getByTestId("admin-agent-detail-save").click()
  check("Agent descriptor 更新保存成功", await appears(adminPage, "Saved", 10000))
  const updatedAgents = await adminApi(adminCookie, `/admin/api/organizations/${organizationId}/agents`)
  const updatedAgent = updatedAgents.items.find((item) => item.id === a2aAgentId)
  const oldChecksum = firstRunApi.body?.snapshot?.agent?.descriptor_checksum
  check(
    "Agent 更新后 descriptor checksum 变化",
    Boolean(updatedAgent) && updatedAgent.descriptor_checksum !== oldChecksum,
    jsonText({ old: oldChecksum, next: updatedAgent?.descriptor_checksum }),
  )
  const replayRuntime = await adminProxy(
    `/admin/api/organizations/${organizationId}/runtimes/${externalId}/refresh`,
    { method: "POST", cookie: adminCookie, body: {} },
  )
  check("外部 Runtime 刷新成功", replayRuntime.status === 200, replayRuntime.text)

  const updatedSend = await sendPublic(memberCookie, sessionId, `after update ${stamp}`, `rv1-updated-${stamp}`)
  const updatedRunId = updatedSend.body?.run?.run_id ?? ""
  await waitRunTerminal(memberCookie, updatedRunId, 60000)
  const updatedRun = await agentDirect(`/api/v1/runs/${updatedRunId}`, { cookie: memberCookie })
  const firstRunAgain = await agentDirect(`/api/v1/runs/${firstRunId}`, { cookie: memberCookie })
  check(
    "新 Run 使用更新后的快照，旧 Run 快照保持不变",
    updatedRun.body?.snapshot?.agent?.descriptor_checksum === updatedAgent?.descriptor_checksum &&
      firstRunAgain.body?.snapshot?.agent?.descriptor_checksum === oldChecksum &&
      updatedRun.body?.snapshot?.agent?.runtime?.runtime_id === externalId &&
      firstRunAgain.body?.snapshot?.agent?.runtime?.runtime_id === externalId,
    jsonText({
      updated: updatedRun.body?.snapshot?.agent?.descriptor_checksum,
      expected: updatedAgent?.descriptor_checksum,
      old: firstRunAgain.body?.snapshot?.agent?.descriptor_checksum,
    }),
  )
  check(
    "Agent 更新不改变 Agent 与 Runtime 的绑定关系数",
    Number(agentScalar(`SELECT count(*) FROM agent_runs WHERE agent_id = '${a2aAgentId}'`)) === runsBeforeUpdate + 1,
  )

  // 10. Sandbox：Skill、资源、产物与受治理 child Run。
  const upload = new FormData()
  upload.append("file", new Blob([`resource body ${resourceMarker}`], { type: "text/plain" }), `rv1-resource-${stamp}.txt`)
  const uploaded = await publicProxy(`/organizations/${organizationId}/resources`, {
    method: "POST",
    cookie: memberCookie,
    formData: upload,
    timeoutMs: 60000,
  })
  check("用户资源通过公开入口上传", uploaded.status === 201 && Boolean(uploaded.body?.id), uploaded.text)

  const sandboxSessionResponse = await publicProxy(`/organizations/${organizationId}/sessions`, {
    method: "POST",
    cookie: memberCookie,
    body: { agent_id: parentAgent.id },
  })
  const sandboxSessionId = sandboxSessionResponse.body?.id ?? ""
  check("Sandbox 父 Agent 创建独立 Session", sandboxSessionResponse.status === 201 && Boolean(sandboxSessionId), sandboxSessionResponse.text)

  const sandboxContent = [
    `Summarize the RV1 note ${stamp}.`,
    `\`\`\`artifact filename="${artifactFilename}" media_type="text/plain"`,
    `artifact body ${stamp}`,
    "```",
    `@delegate ${sandboxChildDisplay}: child task ${stamp}`,
  ].join("\n")
  const sandboxSend = await sendPublic(
    memberCookie,
    sandboxSessionId,
    sandboxContent,
    `rv1-sandbox-${stamp}`,
    { agent_id: parentAgent.id, resource_ids: [uploaded.body.id], skill_ids: [skillId] },
  )
  const sandboxRunId = sandboxSend.body?.run?.run_id ?? ""
  check(
    "Sandbox Run 创建并引用资源与 Skill",
    sandboxSend.status === 201 &&
      sandboxSend.body?.run?.snapshot?.skills?.some((item) => item.skill_id === skillId) &&
      sandboxSend.body?.run?.snapshot?.resources?.some((item) => item.resource_id === uploaded.body.id),
    sandboxSend.text,
  )
  const sandboxTerminal = await waitRunTerminal(memberCookie, sandboxRunId, 60000)
  const sandboxRun = await publicProxy(`/runs/${sandboxRunId}`, { cookie: memberCookie })
  const artifacts = await publicProxy(`/runs/${sandboxRunId}/artifacts`, { cookie: memberCookie })
  const artifactDownload = await publicProxy(`/artifacts/${artifacts.body?.items?.[0]?.id ?? "missing"}`, { cookie: memberCookie })
  const childRun = agentJson(
    `SELECT row_to_json(t) FROM (
       SELECT id::text AS run_id, origin, status, parent_run_id::text, agent_id::text, delegation_depth
       FROM agent_runs WHERE parent_run_id = '${sandboxRunId}'
     ) t`,
  )
  const sandboxEvents = agentJson(
    `SELECT json_agg(json_build_object('seq', seq, 'event_type', event_type) ORDER BY seq)::text
       FROM agent_run_events WHERE run_id = '${sandboxRunId}'`,
  )
  check(
    "Sandbox Run 产出产物并下载真实内容",
    sandboxTerminal &&
      sandboxRun.body?.status === "succeeded" &&
      artifacts.status === 200 &&
      artifacts.body?.items?.some((item) => item.filename === artifactFilename) &&
      artifactDownload.status === 200 &&
      artifactDownload.text.includes(`artifact body ${stamp}`),
    jsonText({ status: sandboxRun.body?.status, artifacts: artifacts.body, downloadStatus: artifactDownload.status }),
  )
  check(
    "受治理委派产生 child Run 并记录事件",
    childRun?.origin === "delegation" &&
      childRun?.status === "succeeded" &&
      childRun?.parent_run_id === sandboxRunId &&
      childRun?.agent_id === childAgent.id &&
      sandboxEvents?.some((event) => event.event_type === "delegation.completed") &&
      sandboxEvents?.some((event) => event.event_type === "artifact.created"),
    jsonText({ childRun, events: sandboxEvents }),
  )

  // 11. Auto 入口：无显式 Agent 时按授权与入口策略选择。
  const autoSessionResponse = await publicProxy(`/organizations/${organizationId}/sessions`, {
    method: "POST",
    cookie: memberCookie,
    body: {},
  })
  const autoSessionId = autoSessionResponse.body?.id ?? ""
  const autoSend = await sendPublic(memberCookie, autoSessionId, `auto rv1 ${stamp}`, `rv1-auto-${stamp}`)
  const autoRunId = autoSend.body?.run?.run_id ?? ""
  await waitRunTerminal(memberCookie, autoRunId, 60000)
  const autoRun = await publicProxy(`/runs/${autoRunId}`, { cookie: memberCookie })
  check(
    "Auto 入口从授权候选选择 Agent 并记录 origin",
    autoSessionResponse.status === 201 &&
      autoRun.status === 200 &&
      autoRun.body?.origin === "auto" &&
      [a2aAgentId, parentAgent.id].includes(autoRun.body?.agent_id),
    jsonText({ session: autoSessionResponse.status, run: autoRun.body?.origin, agent: autoRun.body?.agent_id }),
  )

  // 11.5 可选：真实部署的 A2A Runtime smoke。与本地协议替身分开登记，
  // 记录实际实现、配置来源与结果；没有配置时不产生假通过。
  if (REAL_RUNTIME_ENABLED === "1") {
    check("真实 Runtime 配置来源已记录", Boolean(REAL_RUNTIME_SOURCE) && Boolean(REAL_RUNTIME_URL))
    const realRuntimeResponse = await adminProxy(`/admin/api/organizations/${organizationId}/runtimes`, {
      method: "POST",
      cookie: adminCookie,
      body: {
        name: `rv1-real-${stamp}`,
        display_name: "RV1 Real A2A Runtime",
        endpoint_url: REAL_RUNTIME_URL,
        ...(REAL_RUNTIME_CREDENTIAL_ALIAS ? { credential_alias: REAL_RUNTIME_CREDENTIAL_ALIAS } : {}),
      },
    })
    const realRuntime = realRuntimeResponse.body
    check(
      "真实 Runtime 登记成功",
      realRuntimeResponse.status === 201 && Boolean(realRuntime?.id),
      realRuntimeResponse.text,
    )
    const realReady = await eventually(async () => {
      const list = await adminApi(adminCookie, `/admin/api/organizations/${organizationId}/runtimes`)
      const row = (list.items ?? []).find((item) => item.id === realRuntime?.id)
      return row?.status === "ready"
    }, 30000, 500)
    check("真实 Runtime 探测为 Ready", realReady)

    const realAgentResponse = await adminProxy(`/admin/api/organizations/${organizationId}/agents`, {
      method: "POST",
      cookie: adminCookie,
      body: {
        runtime_connection_id: realRuntime?.id,
        name: `rv1-real-agent-${stamp}`,
        display_name: "RV1 Real Runtime Agent",
        description: "RV1 real-runtime smoke agent",
        prompt: "Reply with the real runtime echo for the RV1 smoke.",
      },
    })
    const realAgent = realAgentResponse.body
    check(
      "真实 Runtime Agent 创建成功",
      realAgentResponse.status === 201 && Boolean(realAgent?.id),
      realAgentResponse.text,
    )
    await adminProxy(`/admin/api/organizations/${organizationId}/agents/${realAgent?.id}/access-rules`, {
      method: "PUT",
      cookie: adminCookie,
      body: { rules: [{ effect: "allow", subject: { kind: "organization", id: organizationId } }] },
    })
    const realSessionResponse = await publicProxy(`/organizations/${organizationId}/sessions`, {
      method: "POST",
      cookie: memberCookie,
      body: { agent_id: realAgent?.id },
    })
    const realSessionId = realSessionResponse.body?.id ?? ""
    const realContent = `real runtime rv1 ${stamp}`
    const realSend = await sendPublic(
      memberCookie,
      realSessionId,
      realContent,
      `rv1-real-${stamp}`,
      { agent_id: realAgent?.id },
    )
    const realRunId = realSend.body?.run?.run_id ?? ""
    check(
      "真实 Runtime Run 创建成功",
      realSessionResponse.status === 201 && realSend.status === 201 && Boolean(realRunId),
      jsonText({ session: realSessionResponse.text, send: realSend.text }),
    )
    const realTerminal = await waitRunTerminal(memberCookie, realRunId, 90000)
    const realRun = await publicProxy(`/runs/${realRunId}`, { cookie: memberCookie })
    const realDetail = await publicProxy(`/sessions/${realSessionId}`, { cookie: memberCookie })
    const realAssistant = (realDetail.body?.messages ?? []).find((message) => message.role === "assistant")
    const realSucceeded =
      realTerminal &&
      realRun.status === 200 &&
      realRun.body?.status === "succeeded" &&
      Boolean(realAssistant) &&
      realAssistant.content.includes("Real A2A SDK reply")
    check(
      "真实 Runtime 完成 Run 并返回独立实现回复",
      realSucceeded,
      jsonText({ status: realRun.body?.status, assistant: realAssistant?.content }),
    )
    console.log(
      `REAL_RUNTIME_RESULT type=a2a implementation=a2a-sdk runtime_version=${REAL_RUNTIME_VERSION} ` +
        `source=${REAL_RUNTIME_SOURCE} runtime_id=${realRuntime?.id} agent_id=${realAgent?.id} ` +
        `run_id=${realRunId} status=${realRun.body?.status ?? "unknown"} reply_contains=Real A2A SDK reply ` +
        `result=${realSucceeded ? "succeeded" : "failed"}`,
    )
  }

  // 12. tracing 前端显示同一 Run 的脱敏事件。
  const tracingPage = await newPage(adminContext, consoleErrors)
  await tracingPage.goto(TRACING_UI_URL)
  const loginVisible = await tracingPage
    .locator('input[placeholder="输入用户名"]')
    .isVisible()
    .catch(() => false)
  if (loginVisible) {
    await tracingPage.locator('input[placeholder="输入用户名"]').fill(String(process.env.ADMIN_USERNAME))
    await tracingPage.locator('input[placeholder="输入密码"]').fill(String(process.env.ADMIN_PASSWORD))
    await tracingPage.getByRole("button", { name: /进入控制台/ }).click()
  }
  check("tracing 控制台可用", await appears(tracingPage, "Agent Runs", 20000))
  await tracingPage.locator('input[placeholder="搜索 Run、Agent、来源或项目"]').fill(firstRunId)
  check(
    "tracing 搜索到 UI 返回的同一 Run",
    await eventually(async () => (await tracingPage.getByText(firstRunId).count()) > 0, 20000),
  )
  await tracingPage.getByText(firstRunId).first().click()
  check(
    "tracing Run 详情显示成功状态与完整 ID",
    await eventually(async () => {
      const body = await tracingPage.locator("body").innerText()
      return body.includes(firstRunId) && body.includes("成功")
    }, 20000),
  )
  await shot(tracingPage, `${shotDir}/tracing-run.png`)

  // 13. 管理页截图：组织、Runtime 下拉与已创建 Agent 同屏。
  await adminPage.goto(`${ADMIN_URL}/agents`)
  await adminPage.getByTestId("admin-agent-create-form").waitFor({ timeout: 20000 })
  await adminPage.getByTestId("admin-agent-row").filter({ hasText: a2aAgentDisplay }).getByTestId("admin-agent-select").click()
  await adminPage.getByTestId("admin-agent-detail").waitFor({ timeout: 20000 })
  await adminPage.getByTestId("admin-agent-runtime-select").click()
  check(
    "Runtime 下拉同时展示内置与外部 Runtime",
    await eventually(async () => {
      const options = await adminPage.getByRole("option").allInnerTexts()
      return options.some((text) => text.includes("Platform Sandbox (sandbox)")) && options.some((text) => text.includes("RV1 A2A Fixture (a2a)"))
    }, 10000),
  )
  await shot(adminPage, `${shotDir}/organization-runtime-agent.png`)

  const unexpectedErrors = consoleErrors.filter(unexpectedConsole)
  check("没有意外的浏览器控制台错误", unexpectedErrors.length === 0, unexpectedErrors.join(" | "))

  // 最终以独立查询复核关键事实，而不是只看前端状态。
  const finalRunCount = Number(agentScalar(`SELECT count(*) FROM agent_runs WHERE organization_id = '${organizationId}'`))
  const finalEventCount = Number(agentScalar(`SELECT count(*) FROM agent_run_events WHERE run_id = '${firstRunId}'`))
  const outboxDelivered = Number(
    agentScalar(`SELECT count(*) FROM agent_trace_outbox WHERE run_id = '${firstRunId}' AND status = 'delivered'`),
  )
  const finalFirstRunDb = runDbRow(firstRunId)
  check(
    "最终独立查询确认 Run/事件/outbox 状态",
    finalRunCount >= 6 &&
      finalEventCount === firstSeqs.length &&
      outboxDelivered === firstSeqs.length &&
      finalFirstRunDb?.trace_delivered_through_seq === firstSeqs.length,
    jsonText({ finalRunCount, finalEventCount, outboxDelivered, firstSeqs: firstSeqs.length, finalFirstRunDb }),
  )
} catch (error) {
  check(
    "脚本执行未抛出异常",
    false,
    `${String(error?.stack ?? error)} console=${consoleErrors.join(" | ")}`,
  )
} finally {
  await browser.close()
}

process.exit(exitCode())

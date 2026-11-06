// rv-2：服务/用户身份边界、组织授权语义、owner ACL 与 tracing 脱敏。
//
// 真实入口：Agent HTTP API（服务凭据 + accounts Cookie）→ accounts 身份验证 →
// 组织 membership / admin scope / Agent ACL → Agent DB/outbox → tracing producer/query。
// 所有敏感标记只用于断言，不打印原值；证据输出只有状态码、错误码与布尔结果。

import {
  ADMIN_USER_ID,
  AGENT_LOG_FILE,
  AGENT_SERVICE_TOKEN,
  AGENT_SERVICE_URL,
  ALT_ADMIN_USER_ID,
  MEMBER_EMAIL,
  MEMBER_PASSWORD,
  MEMBER_USER_ID,
  OTHER_EMAIL,
  OTHER_PASSWORD,
  OTHER_USER_ID,
  SCENARIO_STAMP,
  adminProxy,
  agentScalar,
  altAdminLoginHttp,
  check,
  http,
  exitCode,
  prepareAdminSession,
  prepareUserSession,
  readTextFile,
  requireEnv,
  tracingScalar,
  eventually,
} from "./agent-lib.mjs"

requireEnv([
  "AGENT_SERVICE_URL",
  "AGENT_SERVICE_TOKEN",
  "TRACING_API_URL",
  "ALT_ACCOUNTS_URL",
  "ALT_ADMIN_USERNAME",
  "ALT_ADMIN_PASSWORD",
  "ALT_ADMIN_USER_ID",
  "ADMIN_USER_ID",
  "MEMBER_EMAIL",
  "MEMBER_PASSWORD",
  "MEMBER_USER_ID",
  "OTHER_EMAIL",
  "OTHER_PASSWORD",
  "OTHER_USER_ID",
  "SCENARIO_STAMP",
])

const stamp = SCENARIO_STAMP
const marker = `RV2_SECRET_MARKER_${stamp}`
const promptMarker = `RV2_PROMPT_MARKER_${stamp}`
const artifactMarker = `RV2_ARTIFACT_MARKER_${stamp}`
const slugBase = `rv2-${stamp}`

function jsonText(value) {
  return JSON.stringify(value)
}

async function adminWrite(cookie, path, body, method = "POST") {
  const response = await adminProxy(path, { method, cookie, body })
  if (![200, 201].includes(response.status)) {
    throw new Error(`${path} -> ${response.status} ${response.text}`)
  }
  return response
}

function agentCounts() {
  return {
    runs: Number(agentScalar("SELECT count(*) FROM agent_runs")),
    sessions: Number(agentScalar("SELECT count(*) FROM chat_sessions")),
    events: Number(agentScalar("SELECT count(*) FROM agent_run_events")),
    outbox: Number(agentScalar("SELECT count(*) FROM agent_trace_outbox")),
  }
}

const adminCookie = await prepareAdminSession()
const altAdmin = await altAdminLoginHttp()
check("非平台管理员通过第二个 accounts 实例登录", altAdmin.status === 200 && altAdmin.cookie !== "")

const memberCookie = await prepareUserSession(MEMBER_EMAIL, MEMBER_PASSWORD)
const otherCookie = await prepareUserSession(OTHER_EMAIL, OTHER_PASSWORD)
check(
  "成员与另一用户均取得真实用户会话",
  memberCookie !== "" && otherCookie !== "" && Number(MEMBER_USER_ID) !== Number(OTHER_USER_ID),
)

// 1. 组织、部门、成员与 Agent。全部经真实 admin API 写入。
const orgA = (await adminWrite(adminCookie, "/admin/api/organizations", {
  slug: `${slugBase}-a`,
  display_name: `RV2 Org A ${stamp}`,
})).body
const orgB = (await adminWrite(adminCookie, "/admin/api/organizations", {
  slug: `${slugBase}-b`,
  display_name: `RV2 Org B ${stamp}`,
})).body
await adminWrite(adminCookie, `/admin/api/organizations/${orgA.id}/admin-scopes`, {
  admin_user_id: Number(ADMIN_USER_ID),
})
const unitOne = (await adminWrite(adminCookie, `/admin/api/organizations/${orgA.id}/units`, {
  name: `RV2 Unit One ${stamp}`,
})).body
const unitTwo = (await adminWrite(adminCookie, `/admin/api/organizations/${orgA.id}/units`, {
  name: `RV2 Unit Two ${stamp}`,
})).body
await adminWrite(adminCookie, `/admin/api/organizations/${orgA.id}/memberships`, {
  user_id: Number(MEMBER_USER_ID),
  unit_id: unitOne.id,
  display_name: `RV2 Member ${stamp}`,
})
check("组织、部门与显式成员关系写入成功", Boolean(orgA.id && orgB.id && unitOne.id && unitTwo.id))

const runtimes = (await adminProxy(`/admin/api/organizations/${orgA.id}/runtimes`, { cookie: adminCookie })).body
const builtinRuntime = runtimes.items.find((item) => item.name === "platform-sandbox")
check("组织内可读取内置 Runtime", Boolean(builtinRuntime?.id), jsonText(runtimes))

async function createAgent(displayName, prompt) {
  const response = await adminWrite(adminCookie, `/admin/api/organizations/${orgA.id}/agents`, {
    runtime_connection_id: builtinRuntime.id,
    name: `${slugBase}-${displayName.toLowerCase().replaceAll(" ", "-")}`,
    display_name: displayName,
    description: `${displayName} ${stamp}`,
    prompt,
  })
  return response.body
}

async function setRules(agentId, rules) {
  await adminWrite(
    adminCookie,
    `/admin/api/organizations/${orgA.id}/agents/${agentId}/access-rules`,
    { rules },
    "PUT",
  )
}

const orgAgent = await createAgent("RV2 Org Agent", `You are the RV2 org agent. ${promptMarker}`)
const unitAgent = await createAgent("RV2 Unit Agent", `You are the RV2 unit agent. ${promptMarker}`)
const denyAgent = await createAgent("RV2 Deny Agent", `You are the RV2 deny agent. ${promptMarker}`)
const secretAgent = await createAgent("RV2 Secret Agent", `You are the RV2 secret agent. ${promptMarker}`)

await setRules(orgAgent.id, [{ effect: "allow", subject: { kind: "organization", id: orgA.id } }])
await setRules(unitAgent.id, [{ effect: "allow", subject: { kind: "unit", id: unitTwo.id } }])
await setRules(denyAgent.id, [
  { effect: "allow", subject: { kind: "organization", id: orgA.id } },
  { effect: "deny", subject: { kind: "user", id: Number(MEMBER_USER_ID) } },
])
await setRules(secretAgent.id, [{ effect: "allow", subject: { kind: "organization", id: orgA.id } }])
check("四个受控 Agent 创建并写入 allow/deny 规则", Boolean(orgAgent.id && unitAgent.id && denyAgent.id && secretAgent.id))

// 2. 身份负控：每个请求都必须被拒绝，且 Agent/tracing 持久化无副作用。
const baseline = agentCounts()
const tracingBaseline = Number(tracingScalar("SELECT count(*) FROM agent_runs"))

const noServiceToken = await http(AGENT_SERVICE_URL, "/api/v1/organizations", { cookie: memberCookie })
check(
  "缺少服务凭据的 Agent 请求返回 401 service_unauthorized",
  noServiceToken.status === 401 && noServiceToken.body?.error?.code === "service_unauthorized",
  `${noServiceToken.status} ${noServiceToken.body?.error?.code ?? ""}`,
)
const invalidServiceToken = await http(AGENT_SERVICE_URL, "/api/v1/organizations", {
  headers: { authorization: "Bearer invalid-service-token" },
  cookie: memberCookie,
})
check(
  "无效服务凭据返回 401 service_unauthorized",
  invalidServiceToken.status === 401 && invalidServiceToken.body?.error?.code === "service_unauthorized",
  `${invalidServiceToken.status} ${invalidServiceToken.body?.error?.code ?? ""}`,
)
const forgedUserHeader = await http(AGENT_SERVICE_URL, "/api/v1/organizations", {
  token: AGENT_SERVICE_TOKEN,
  headers: { "x-user-id": String(MEMBER_USER_ID) },
})
check(
  "有效服务凭据但无用户会话、仅伪造 X-User-Id 返回 401",
  forgedUserHeader.status === 401 && forgedUserHeader.body?.error?.code === "unauthenticated",
  `${forgedUserHeader.status} ${forgedUserHeader.body?.error?.code ?? ""}`,
)
const forgedCookie = await http(AGENT_SERVICE_URL, "/api/v1/organizations", {
  token: AGENT_SERVICE_TOKEN,
  cookie: "session=forged-user-session",
})
check(
  "伪造用户 Cookie 返回 401 unauthenticated",
  forgedCookie.status === 401 && forgedCookie.body?.error?.code === "unauthenticated",
  `${forgedCookie.status} ${forgedCookie.body?.error?.code ?? ""}`,
)
const memberOnAdmin = await adminProxy("/admin/api/organizations", { cookie: memberCookie })
check(
  "public 用户 Cookie 不能访问 admin API",
  memberOnAdmin.status === 401 && memberOnAdmin.body?.error?.code === "unauthenticated",
  `${memberOnAdmin.status} ${memberOnAdmin.body?.error?.code ?? ""}`,
)
const altAdminOnPublic = await http(AGENT_SERVICE_URL, "/api/v1/organizations", {
  token: AGENT_SERVICE_TOKEN,
  cookie: altAdmin.cookie,
})
check(
  "admin Cookie 不能冒充 public 用户身份",
  altAdminOnPublic.status === 401 && altAdminOnPublic.body?.error?.code === "unauthenticated",
  `${altAdminOnPublic.status} ${altAdminOnPublic.body?.error?.code ?? ""}`,
)

const afterAuthNegatives = agentCounts()
const tracingAfterNegatives = Number(tracingScalar("SELECT count(*) FROM agent_runs"))
check(
  "身份负控没有创建 Session/Run/事件/outbox 或 tracing 记录",
  jsonText(afterAuthNegatives) === jsonText(baseline) && tracingAfterNegatives === tracingBaseline,
  jsonText({ baseline, afterAuthNegatives, tracingBaseline, tracingAfterNegatives }),
)

// 3. 组织授权语义：非成员、无部门显式授权、个人 deny、owner ACL。
const otherOrgAgents = await http(AGENT_SERVICE_URL, `/api/v1/organizations/${orgA.id}/agents`, {
  token: AGENT_SERVICE_TOKEN,
  cookie: otherCookie,
})
check(
  "非组织成员读取 Agent 目录被拒 403 organization_membership_required",
  otherOrgAgents.status === 403 && otherOrgAgents.body?.error?.code === "organization_membership_required",
  `${otherOrgAgents.status} ${otherOrgAgents.body?.error?.code ?? ""}`,
)
const otherCreateSession = await http(AGENT_SERVICE_URL, `/api/v1/organizations/${orgA.id}/sessions`, {
  token: AGENT_SERVICE_TOKEN,
  cookie: otherCookie,
  body: { agent_id: orgAgent.id },
})
check(
  "非组织成员创建 Session 被拒且无副作用",
  otherCreateSession.status === 403 &&
    otherCreateSession.body?.error?.code === "organization_membership_required" &&
    jsonText(agentCounts()) === jsonText(afterAuthNegatives),
  `${otherCreateSession.status} ${otherCreateSession.body?.error?.code ?? ""}`,
)

const memberAgents = await http(AGENT_SERVICE_URL, `/api/v1/organizations/${orgA.id}/agents`, {
  token: AGENT_SERVICE_TOKEN,
  cookie: memberCookie,
})
const memberVisibleIds = memberAgents.body?.items?.map((item) => item.agent?.id) ?? []
check(
  "组织 allow 可见；unit 未授权的 Agent 与个人 deny 的 Agent 不可见",
  memberAgents.status === 200 &&
    memberVisibleIds.includes(orgAgent.id) &&
    !memberVisibleIds.includes(unitAgent.id) &&
    !memberVisibleIds.includes(denyAgent.id),
  jsonText(memberVisibleIds),
)
const unitDenied = await http(AGENT_SERVICE_URL, `/api/v1/organizations/${orgA.id}/sessions`, {
  token: AGENT_SERVICE_TOKEN,
  cookie: memberCookie,
  body: { agent_id: unitAgent.id },
})
check(
  "部门规则不继承：成员所属部门之外的单位授权返回 403 agent_access_denied",
  unitDenied.status === 403 && unitDenied.body?.error?.code === "agent_access_denied",
  `${unitDenied.status} ${unitDenied.body?.error?.code ?? ""}`,
)
const denyDenied = await http(AGENT_SERVICE_URL, `/api/v1/organizations/${orgA.id}/sessions`, {
  token: AGENT_SERVICE_TOKEN,
  cookie: memberCookie,
  body: { agent_id: denyAgent.id },
})
check(
  "组织 allow 之上的个人 deny 返回 403 agent_access_denied",
  denyDenied.status === 403 && denyDenied.body?.error?.code === "agent_access_denied",
  `${denyDenied.status} ${denyDenied.body?.error?.code ?? ""}`,
)
check(
  "ACL 拒绝没有创建额外 Session",
  Number(agentScalar("SELECT count(*) FROM chat_sessions")) === afterAuthNegatives.sessions,
)

const createdSession = await http(AGENT_SERVICE_URL, `/api/v1/organizations/${orgA.id}/sessions`, {
  token: AGENT_SERVICE_TOKEN,
  cookie: memberCookie,
  body: { agent_id: orgAgent.id },
})
const sessionId = createdSession.body?.id ?? ""
check("组织 allow 的成员成功创建 Session", createdSession.status === 201 && Boolean(sessionId), createdSession.text)

const sent = await http(AGENT_SERVICE_URL, `/api/v1/sessions/${sessionId}/messages`, {
  token: AGENT_SERVICE_TOKEN,
  cookie: memberCookie,
  headers: { "idempotency-key": `rv2-valid-${stamp}` },
  body: {
    agent_id: orgAgent.id,
    content: `valid rv2 ${stamp}`,
    resource_ids: [],
    skill_ids: [],
  },
})
const runId = sent.body?.run?.run_id ?? ""
const terminal = await eventually(async () => {
  const run = await http(AGENT_SERVICE_URL, `/api/v1/runs/${runId}`, {
    token: AGENT_SERVICE_TOKEN,
    cookie: memberCookie,
  })
  return run.status === 200 && ["succeeded", "failed", "cancelled"].includes(run.body?.status)
}, 60000)
check("有效 Run 创建并到达终态", sent.status === 201 && terminal, `${sent.status} ${sent.text}`)

// 加入组织后的另一成员仍不是 Session/Run 所有者。
await adminWrite(adminCookie, `/admin/api/organizations/${orgA.id}/memberships`, {
  user_id: Number(OTHER_USER_ID),
  display_name: `RV2 Other ${stamp}`,
})
const otherSessionRead = await http(AGENT_SERVICE_URL, `/api/v1/sessions/${sessionId}`, {
  token: AGENT_SERVICE_TOKEN,
  cookie: otherCookie,
})
const otherRunRead = await http(AGENT_SERVICE_URL, `/api/v1/runs/${runId}`, {
  token: AGENT_SERVICE_TOKEN,
  cookie: otherCookie,
})
const otherRunCancel = await http(AGENT_SERVICE_URL, `/api/v1/runs/${runId}/cancel`, {
  token: AGENT_SERVICE_TOKEN,
  cookie: otherCookie,
  method: "POST",
  body: {},
})
const otherSessionList = await http(AGENT_SERVICE_URL, `/api/v1/organizations/${orgA.id}/sessions`, {
  token: AGENT_SERVICE_TOKEN,
  cookie: otherCookie,
})
check(
  "Session/Run owner ACL：其他组织成员读取/取消被当作不存在",
  otherSessionRead.status === 404 &&
    otherSessionRead.body?.error?.code === "session_not_found" &&
    otherRunRead.status === 404 &&
    otherRunRead.body?.error?.code === "run_not_found" &&
    otherRunCancel.status === 404 &&
    otherRunCancel.body?.error?.code === "run_not_found",
  jsonText({
    session: [otherSessionRead.status, otherSessionRead.body?.error?.code],
    run: [otherRunRead.status, otherRunRead.body?.error?.code],
    cancel: [otherRunCancel.status, otherRunCancel.body?.error?.code],
  }),
)
check(
  "其他成员列表不包含他人 Session",
  !(otherSessionList.body?.items ?? []).some((item) => item.id === sessionId),
  otherSessionList.text,
)

// 4. 有效 Run 的 tracing 投影与日志不含标记秘密。
const secretSession = await http(AGENT_SERVICE_URL, `/api/v1/organizations/${orgA.id}/sessions`, {
  token: AGENT_SERVICE_TOKEN,
  cookie: memberCookie,
  body: { agent_id: secretAgent.id },
})
const secretSessionId = secretSession.body?.id ?? ""
check("含标记秘密的 Agent 创建独立 Session", secretSession.status === 201 && Boolean(secretSessionId), secretSession.text)

const secretContent = [
  `secret input ${marker}`,
  `\`\`\`artifact filename="${artifactMarker}.txt" media_type="text/plain"`,
  `artifact body ${artifactMarker}`,
  "```",
].join("\n")
const secretSend = await http(AGENT_SERVICE_URL, `/api/v1/sessions/${secretSessionId}/messages`, {
  token: AGENT_SERVICE_TOKEN,
  cookie: memberCookie,
  headers: { "idempotency-key": `rv2-secret-${stamp}` },
  body: {
    agent_id: secretAgent.id,
    content: secretContent,
    resource_ids: [],
    skill_ids: [],
  },
})
const secretRunId = secretSend.body?.run?.run_id ?? ""
const secretTerminal = await eventually(async () => {
  const run = await http(AGENT_SERVICE_URL, `/api/v1/runs/${secretRunId}`, {
    token: AGENT_SERVICE_TOKEN,
    cookie: memberCookie,
  })
  return run.status === 200 && ["succeeded", "failed", "cancelled"].includes(run.body?.status)
}, 60000)
check("含标记秘密的有效 Run 成功完成", secretSend.status === 201 && secretTerminal, `${secretSend.status} ${secretSend.text}`)

const secretEvents = agentScalar(
  `SELECT COALESCE(json_agg(payload ORDER BY seq)::text, '[]') FROM agent_run_events WHERE run_id = '${secretRunId}'`,
)
const secretChat = agentScalar(`SELECT content FROM chat_messages WHERE run_id = '${secretRunId}' AND role = 'user'`)
const secretArtifacts = agentScalar(
  `SELECT COALESCE(json_agg(filename)::text, '[]') FROM run_artifacts WHERE run_id = '${secretRunId}'`,
)
check(
  "标记秘密保存在 owner 可见的消息中，但不出现在 Run 事件投影",
  secretChat.includes(marker) &&
    !secretEvents.includes(marker) &&
    !secretEvents.includes(artifactMarker) &&
    !secretEvents.includes(promptMarker) &&
    secretEvents.includes("[已脱敏]"),
  jsonText({ chatHasMarker: secretChat.includes(marker), eventsRedacted: secretEvents.includes("[已脱敏]") }),
)
check(
  "产物文件名在事件中脱敏但仍以真实文件保存",
  !secretEvents.includes(`${artifactMarker}.txt`) && secretArtifacts.includes(`${artifactMarker}.txt`),
  jsonText({ storedFilenames: JSON.parse(secretArtifacts || "[]") }),
)

let tracingExport = null
const exportReady = await eventually(async () => {
  const response = await http(process.env.TRACING_API_URL, `/admin/agent-traces/${secretRunId}/export`, {
    cookie: adminCookie,
  })
  if (response.status !== 200) return false
  tracingExport = response
  return true
}, 60000, 500)
check(
  "tracing 查询到同一 Run 且响应不含标记秘密",
  exportReady &&
    tracingExport?.body?.run_id === secretRunId &&
    !tracingExport.text.includes(marker) &&
    !tracingExport.text.includes(artifactMarker) &&
    !tracingExport.text.includes(promptMarker) &&
    tracingExport.body?.events?.at(-1)?.event_type === "run.completed",
  jsonText({ status: tracingExport?.status, runId: tracingExport?.body?.run_id, events: tracingExport?.body?.event_count }),
)

const agentLog = readTextFile(AGENT_LOG_FILE)
const secretsInLog = [marker, artifactMarker, promptMarker, AGENT_SERVICE_TOKEN].some((value) =>
  agentLog.includes(value),
)
check("Agent 日志不包含标记秘密、完整提示词或服务凭据", agentLog.length > 0 && !secretsInLog)

// 5. admin scope 与 public 身份隔离，跨组织管理被拒。
const noScopeRead = await adminProxy(`/admin/api/organizations/${orgA.id}`, { cookie: altAdmin.cookie })
check(
  "未获组织 scope 的非平台管理员读取组织被拒 403 organization_scope_denied",
  noScopeRead.status === 403 && noScopeRead.body?.error?.code === "organization_scope_denied",
  `${noScopeRead.status} ${noScopeRead.body?.error?.code ?? ""}`,
)
await adminWrite(adminCookie, `/admin/api/organizations/${orgA.id}/admin-scopes`, {
  admin_user_id: Number(ALT_ADMIN_USER_ID),
})
const scopedRead = await adminProxy(`/admin/api/organizations/${orgA.id}`, { cookie: altAdmin.cookie })
const crossOrgRead = await adminProxy(`/admin/api/organizations/${orgB.id}`, { cookie: altAdmin.cookie })
const crossOrgCreate = await adminProxy(`/admin/api/organizations/${orgB.id}/agents`, {
  method: "POST",
  cookie: altAdmin.cookie,
  body: {
    runtime_connection_id: builtinRuntime.id,
    name: `rv2-cross-${stamp}`,
    display_name: `RV2 Cross ${stamp}`,
    description: "",
    prompt: "cross org",
  },
})
const altAdminList = await adminProxy("/admin/api/organizations", { cookie: altAdmin.cookie })
check(
  "授予 scope 后可管理本组织，但跨组织读取/创建被拒",
  scopedRead.status === 200 &&
    crossOrgRead.status === 403 &&
    crossOrgRead.body?.error?.code === "organization_scope_denied" &&
    crossOrgCreate.status === 403 &&
    crossOrgCreate.body?.error?.code === "organization_scope_denied" &&
    (altAdminList.body?.items ?? []).some((item) => item.id === orgA.id) &&
    !(altAdminList.body?.items ?? []).some((item) => item.id === orgB.id),
  jsonText({
    scoped: scopedRead.status,
    crossRead: [crossOrgRead.status, crossOrgRead.body?.error?.code],
    crossCreate: [crossOrgCreate.status, crossOrgCreate.body?.error?.code],
    list: (altAdminList.body?.items ?? []).map((item) => item.slug),
  }),
)

// 6. 最终独立计数：只有两条有效 Run，负控没有留下任何记录。
const finalCounts = agentCounts()
const tracingRunCount = Number(tracingScalar("SELECT count(*) FROM agent_runs"))
const newTracingRunCount = tracingRunCount - tracingBaseline
const invalidAgentCount = Number(
  agentScalar(
    `SELECT count(*) FROM agent_runs WHERE agent_id IN ('${unitAgent.id}','${denyAgent.id}')`,
  ),
)
const otherOwnedRuns = Number(
  agentScalar(`SELECT count(*) FROM agent_runs WHERE owner_user_id = ${Number(OTHER_USER_ID)}`),
)
check(
  "最终 Agent/tracing 记录与成功 Run 一一对应且无越权副作用",
  finalCounts.runs === baseline.runs + 2 &&
    finalCounts.sessions === baseline.sessions + 2 &&
    newTracingRunCount >= 1 &&
    newTracingRunCount <= finalCounts.runs - baseline.runs &&
    invalidAgentCount === 0 &&
    otherOwnedRuns === 0,
  jsonText({ baseline, tracingBaseline, finalCounts, tracingRunCount, newTracingRunCount, invalidAgentCount, otherOwnedRuns }),
)
check(
  "管理员 ID 与平台管理员不同（scope 隔离前提）",
  Number(ALT_ADMIN_USER_ID) !== Number(ADMIN_USER_ID) && Number(ALT_ADMIN_USER_ID) > 0,
)

process.exit(exitCode())

// rv-5：独立业务微服务只调用 Agent HTTP 契约。
//
// 真实入口：浏览器外的业务 HTTP 入口 → 示例服务 → Agent public HTTP API →
// 真实 A2A adapter/DB/tracing。示例服务与 Agent 使用同一用户 Cookie 分别调用，
// 比对创建、重放、查询、取消响应和稳定错误码，并检查示例依赖树没有数据库配置。

import { execFileSync } from "node:child_process"
import { readFileSync } from "node:fs"

import {
  A2A_CREDENTIAL_ALIAS,
  A2A_ENDPOINT,
  AGENT_SERVICE_TOKEN,
  AGENT_SERVICE_URL,
  MEMBER_EMAIL,
  MEMBER_PASSWORD,
  MEMBER_USER_ID,
  REPO_ROOT,
  SAMPLE_SERVICE_URL,
  SCENARIO_STAMP,
  adminProxy,
  agentDirect,
  check,
  eventually,
  exitCode,
  http,
  prepareAdminSession,
  prepareUserSession,
  requireEnv,
} from "./agent-lib.mjs"

requireEnv([
  "REPO_ROOT",
  "AGENT_SERVICE_URL",
  "AGENT_SERVICE_TOKEN",
  "SAMPLE_SERVICE_URL",
  "SAMPLE_SERVICE_TOKEN",
  "A2A_ENDPOINT",
  "A2A_CREDENTIAL_ALIAS",
  "ADMIN_USER_ID",
  "MEMBER_EMAIL",
  "MEMBER_PASSWORD",
  "MEMBER_USER_ID",
  "SCENARIO_STAMP",
])

const stamp = SCENARIO_STAMP
const slug = `rv5-${stamp}`
const missingRunId = "00000000-0000-0000-0000-000000000000"

function sample(path, options = {}) {
  return http(SAMPLE_SERVICE_URL, `/api/business${path}`, options)
}

function direct(path, options = {}) {
  return agentDirect(`/api/v1${path}`, options)
}

async function sendViaSample(sessionId, content, idempotencyKey, extra = {}) {
  return sample(`/sessions/${sessionId}/messages`, {
    method: "POST",
    cookie: memberCookie,
    headers: { "idempotency-key": idempotencyKey },
    body: { agent_id: agent.id, content, resource_ids: [], skill_ids: [], ...extra },
  })
}

async function waitSampleRun(runId, statuses, timeout = 60000) {
  let latest = null
  const reached = await eventually(async () => {
    const response = await sample(`/runs/${runId}`, { cookie: memberCookie })
    latest = response
    return response.status === 200 && statuses.includes(response.body?.status)
  }, timeout, 250)
  return { reached, latest }
}

const adminCookie = await prepareAdminSession()
const memberCookie = await prepareUserSession(MEMBER_EMAIL, MEMBER_PASSWORD)
check("管理员与成员分别取得真实会话", adminCookie !== "" && memberCookie !== "")

// 1. 准备组织、外部 Runtime、Agent 与组织 allow 规则。
const orgResponse = await adminProxy("/admin/api/organizations", {
  method: "POST",
  cookie: adminCookie,
  body: { slug, display_name: `RV5 Business Org ${stamp}` },
})
const organization = orgResponse.body
check("组织创建成功", orgResponse.status === 201 && Boolean(organization?.id), orgResponse.text)

const scopeResponse = await adminProxy(`/admin/api/organizations/${organization.id}/admin-scopes`, {
  method: "POST",
  cookie: adminCookie,
  body: { admin_user_id: Number(process.env.ADMIN_USER_ID) },
})
check("平台管理员取得组织 scope", scopeResponse.status === 201, scopeResponse.text)

const membershipResponse = await adminProxy(`/admin/api/organizations/${organization.id}/memberships`, {
  method: "POST",
  cookie: adminCookie,
  body: { user_id: Number(MEMBER_USER_ID), display_name: `RV5 Member ${stamp}` },
})
check("成员加入组织", membershipResponse.status === 200 || membershipResponse.status === 201, membershipResponse.text)

const runtimeResponse = await adminProxy(`/admin/api/organizations/${organization.id}/runtimes`, {
  method: "POST",
  cookie: adminCookie,
  body: {
    name: `rv5-a2a-${stamp}`,
    display_name: "RV5 A2A Fixture",
    endpoint_url: A2A_ENDPOINT,
    credential_alias: A2A_CREDENTIAL_ALIAS,
  },
})
const externalRuntime = runtimeResponse.body
check(
  "外部 A2A Runtime 登记成功",
  runtimeResponse.status === 201 && Boolean(externalRuntime?.id),
  runtimeResponse.text,
)

const agentResponse = await adminProxy(`/admin/api/organizations/${organization.id}/agents`, {
  method: "POST",
  cookie: adminCookie,
  body: {
    runtime_connection_id: externalRuntime.id,
    name: `rv5-agent-${stamp}`,
    display_name: "RV5 Business Agent",
    description: "RV5 business-service sample agent",
    prompt: "You are the RV5 business-service sample agent.",
  },
})
const agent = agentResponse.body
check("Agent 创建成功", agentResponse.status === 201 && Boolean(agent?.id), agentResponse.text)

const rulesResponse = await adminProxy(
  `/admin/api/organizations/${organization.id}/agents/${agent.id}/access-rules`,
  {
    method: "PUT",
    cookie: adminCookie,
    body: { rules: [{ effect: "allow", subject: { kind: "organization", id: organization.id } }] },
  },
)
check("Agent 组织 allow 规则写入成功", rulesResponse.status === 200, rulesResponse.text)

const sessionResponse = await direct(
  `/organizations/${organization.id}/sessions`,
  { cookie: memberCookie, body: { agent_id: agent.id } },
)
const sessionId = sessionResponse.body?.id ?? ""
check("成员通过 Agent 契约创建 Session", sessionResponse.status === 201 && Boolean(sessionId), sessionResponse.text)

// 2. 业务入口创建 Run：状态码、响应封套与冻结快照必须符合契约。
const content = `business sample rv5 ${stamp}`
const idempotencyKey = `rv5-create-${stamp}`
const created = await sendViaSample(sessionId, content, idempotencyKey)
const runId = created.body?.run?.run_id ?? ""
check(
  "业务入口创建 Run 返回 201 与 created=true",
  created.status === 201 && created.body?.created === true && Boolean(runId),
  created.text,
)
check(
  "创建响应字段与手写客户端一致",
  created.body?.run?.organization_id === organization.id &&
    created.body?.run?.session_id === sessionId &&
    created.body?.run?.agent_id === agent.id &&
    created.body?.run?.runtime_connection_id === externalRuntime.id &&
    created.body?.run?.owner_user_id === Number(MEMBER_USER_ID) &&
    created.body?.run?.origin === "direct" &&
    ["queued", "running", "succeeded"].includes(created.body?.run?.status) &&
    created.body?.run?.idempotency_key === idempotencyKey &&
    typeof created.body?.run?.request_digest === "string" &&
    created.body?.run?.snapshot?.input?.content === content &&
    created.body?.run?.snapshot?.agent?.runtime?.runtime_id === externalRuntime.id &&
    created.body?.run?.snapshot?.agent?.runtime?.adapter_kind === "a2a" &&
    Object.prototype.hasOwnProperty.call(created.body?.run ?? {}, "parent_run_id"),
  JSON.stringify(created.body?.run),
)

// 3. 同一幂等键重放：返回 200、created=false 且是同一个 Run。
const replay = await sendViaSample(sessionId, content, idempotencyKey)
check(
  "业务入口幂等重放返回 200、created=false 与同一 Run",
  replay.status === 200 &&
    replay.body?.created === false &&
    replay.body?.run?.run_id === runId &&
    replay.body?.run?.request_digest === created.body?.run?.request_digest,
  replay.text,
)

// 4. 查询：业务入口与 Agent 直连读取同一持久化记录。
const terminal = await waitSampleRun(runId, ["succeeded", "failed"])
check("业务入口查询到终态 Run", terminal.reached && terminal.latest.body?.status === "succeeded", terminal.latest?.text ?? "")
const sampleRun = await sample(`/runs/${runId}`, { cookie: memberCookie })
const directRun = await direct(`/runs/${runId}`, { cookie: memberCookie })
check(
  "查询响应与 Agent 直连记录逐字段一致",
  sampleRun.status === 200 &&
    directRun.status === 200 &&
    ["run_id", "organization_id", "session_id", "owner_user_id", "agent_id", "runtime_connection_id", "origin", "status", "idempotency_key", "request_digest", "last_event_seq", "trace_delivered_through_seq", "error_code", "created_at", "started_at", "finished_at"].every(
      (field) => JSON.stringify(sampleRun.body?.[field]) === JSON.stringify(directRun.body?.[field]),
    ) &&
    sampleRun.body?.snapshot?.agent?.runtime?.adapter_kind === directRun.body?.snapshot?.agent?.runtime?.adapter_kind,
  JSON.stringify({ sample: sampleRun.body, direct: directRun.body }),
)
const detail = await direct(`/sessions/${sessionId}`, { cookie: memberCookie })
const assistant = (detail.body?.messages ?? []).find((message) => message.role === "assistant")
check(
  "同一 Run 的助手回复经真实 A2A adapter 写回 Session",
  detail.status === 200 && Boolean(assistant) && assistant.content.includes("E2E A2A reply"),
  JSON.stringify(detail.body?.messages ?? []),
)

// 5. 取消：业务入口的请求体会原样转发给协作式取消。
const slow = await sendViaSample(sessionId, `SLOW rv5 ${stamp}`, `rv5-slow-${stamp}`)
const slowRunId = slow.body?.run?.run_id ?? ""
check("业务入口创建 SLOW Run", slow.status === 201 && Boolean(slowRunId), slow.text)
const slowStarted = await waitSampleRun(slowRunId, ["running", "queued"], 30000)
const cancel = await sample(`/runs/${slowRunId}/cancel`, {
  method: "POST",
  cookie: memberCookie,
})
check(
  "取消响应包含 run 与 cancelled_immediately 且 Run 进入终态",
  cancel.status === 200 &&
    cancel.body?.run?.run_id === slowRunId &&
    typeof cancel.body?.cancelled_immediately === "boolean",
  cancel.text,
)
const slowTerminal = await waitSampleRun(slowRunId, ["cancelled", "succeeded", "failed"], 60000)
const cancelledDirect = await direct(`/runs/${slowRunId}`, { cookie: memberCookie })
check(
  "取消后状态从 Agent 直连独立读取一致",
  slowStarted.reached &&
    slowTerminal.reached &&
    slowTerminal.latest.body?.status === "cancelled" &&
    cancelledDirect.status === 200 &&
    cancelledDirect.body?.status === "cancelled",
  JSON.stringify({ started: slowStarted.latest?.body?.status, sample: slowTerminal.latest?.body?.status, direct: cancelledDirect.body?.status }),
)

// 6. 稳定错误码：业务入口必须透传 Agent 的 404/401/400，而不是自己发明。
const missingSample = await sample(`/runs/${missingRunId}`, { cookie: memberCookie })
const missingDirect = await direct(`/runs/${missingRunId}`, { cookie: memberCookie })
check(
  "不存在的 Run 透传 404 run_not_found",
  missingSample.status === 404 &&
    missingDirect.status === 404 &&
    missingSample.body?.error?.code === "run_not_found" &&
    missingSample.body?.error?.code === missingDirect.body?.error?.code,
  JSON.stringify({ sample: missingSample.body, direct: missingDirect.body }),
)

const anonymousSample = await sample(`/runs/${runId}`)
const anonymousDirect = await http(AGENT_SERVICE_URL, `/api/v1/runs/${runId}`, {
  token: AGENT_SERVICE_TOKEN,
})
check(
  "缺少用户 Cookie 透传 401 unauthenticated",
  anonymousSample.status === 401 &&
    anonymousDirect.status === 401 &&
    anonymousSample.body?.error?.code === "unauthenticated" &&
    anonymousSample.body?.error?.code === anonymousDirect.body?.error?.code,
  JSON.stringify({ sample: anonymousSample.body, direct: anonymousDirect.body }),
)

const noKeySample = await sample(`/sessions/${sessionId}/messages`, {
  method: "POST",
  cookie: memberCookie,
  body: { agent_id: agent.id, content: `missing key rv5 ${stamp}` },
})
const noKeyDirect = await direct(`/sessions/${sessionId}/messages`, {
  method: "POST",
  cookie: memberCookie,
  body: { agent_id: agent.id, content: `missing key rv5 ${stamp}` },
})
check(
  "缺少 Idempotency-Key 透传 400 invalid_request",
  noKeySample.status === 400 &&
    noKeyDirect.status === 400 &&
    noKeySample.body?.error?.code === "invalid_request" &&
    noKeySample.body?.error?.code === noKeyDirect.body?.error?.code,
  JSON.stringify({ sample: noKeySample.body, direct: noKeyDirect.body }),
)

// 7. 示例本身没有 Agent 数据库依赖或配置。
const dependencyTree = execFileSync(
  "cargo",
  ["tree", "-p", "agent-business-service", "--prefix", "none", "--edges", "normal"],
  { cwd: REPO_ROOT, encoding: "utf8" },
)
const forbiddenDependencies = ["tpl-agent", "sqlx", "postgres", "redis"].filter((name) =>
  dependencyTree.toLowerCase().includes(name),
)
check(
  "示例依赖树不含 Agent 私有 crate 或数据库客户端",
  forbiddenDependencies.length === 0,
  forbiddenDependencies.join(", "),
)
const manifestAndSources = ["Cargo.toml", "src/main.rs", "src/contract.rs", "src/upstream.rs"]
  .map((relativePath) => readFileSync(`${REPO_ROOT}/examples/agent-business-service/${relativePath}`, "utf8"))
  .join("\n")
check(
  "示例清单与源码没有 Agent 数据库配置",
  !/DATABASE_URL|AGENT_DATABASE|sqlx|postgres/i.test(manifestAndSources),
  manifestAndSources.match(/.{0,40}(DATABASE_URL|AGENT_DATABASE|sqlx|postgres).{0,40}/i)?.[0] ?? "",
)

process.exit(exitCode())

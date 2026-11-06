// rv-3：tracing 故障期间 Run 照常完成，恢复后 outbox 有序补投且重复投递不重复。
//
// 真实入口：Agent HTTP API → Agent DB/outbox → 真实停止的 tracing 进程 → 重启后
// tracing producer HTTP API / 查询 API 与 tracing DB。故障由真实进程停止制造，
// 不替换 producer 客户端，也不直接写 tracing 表。

import { openSync, writeFileSync, readFileSync } from "node:fs"
import { spawn } from "node:child_process"

import {
  A2A_CREDENTIAL_ALIAS,
  A2A_ENDPOINT,
  AGENT_LOG_FILE,
  AGENT_SERVICE_TOKEN,
  AGENT_SERVICE_URL,
  MEMBER_EMAIL,
  MEMBER_PASSWORD,
  MEMBER_USER_ID,
  SCENARIO_STAMP,
  TRACING_API_URL,
  TRACING_LOG_FILE,
  TRACING_PID_FILE,
  TRACING_START_SCRIPT,
  adminProxy,
  agentJson,
  agentScalar,
  check,
  exitCode,
  http,
  prepareAdminSession,
  prepareUserSession,
  readTextFile,
  requireEnv,
  sleep,
  tracingScalar,
  eventually,
} from "./agent-lib.mjs"

requireEnv([
  "AGENT_SERVICE_URL",
  "AGENT_SERVICE_TOKEN",
  "TRACING_API_URL",
  "TRACING_PID_FILE",
  "TRACING_START_SCRIPT",
  "A2A_ENDPOINT",
  "A2A_CREDENTIAL_ALIAS",
  "ADMIN_USER_ID",
  "MEMBER_EMAIL",
  "MEMBER_PASSWORD",
  "MEMBER_USER_ID",
  "SCENARIO_STAMP",
])

const stamp = SCENARIO_STAMP
const slug = `rv3-${stamp}`
const timeline = []
function mark(event) {
  const entry = `${new Date().toISOString()} ${event}`
  timeline.push(entry)
  console.log(`TIMELINE  ${entry}`)
}

function runDbRow(runId) {
  return agentJson(
    `SELECT row_to_json(t) FROM (
       SELECT id::text AS run_id, status, origin, agent_id::text, runtime_connection_id::text,
              last_event_seq, trace_delivered_through_seq
       FROM agent_runs WHERE id = '${runId}'
     ) t`,
  )
}

function outboxRow(runId) {
  return agentJson(
    `SELECT json_build_object(
       'pending', count(*) FILTER (WHERE status = 'pending'),
       'delivered', count(*) FILTER (WHERE status = 'delivered'),
       'max_event_seq', max(event_seq),
       'min_event_seq', min(event_seq)
     )::text FROM agent_trace_outbox WHERE run_id = '${runId}'`,
  )
}

function agentEventSeqs(runId) {
  return agentJson(
    `SELECT json_agg(json_build_object('seq', seq, 'event_type', event_type) ORDER BY seq)::text
       FROM agent_run_events WHERE run_id = '${runId}'`,
  )
}

async function tracingAlive() {
  try {
    const response = await http(TRACING_API_URL, "/live", { timeoutMs: 1500 })
    return response.status === 200
  } catch {
    return false
  }
}

async function tracingExport(runId) {
  return http(TRACING_API_URL, `/admin/agent-traces/${runId}/export`, { cookie: adminCookie })
}

const adminCookie = await prepareAdminSession()
const memberCookie = await prepareUserSession(MEMBER_EMAIL, MEMBER_PASSWORD)
check("管理员与成员分别取得真实会话", adminCookie !== "" && memberCookie !== "")

// 1. 准备组织、外部 Runtime 与 Agent。
const orgResponse = await adminProxy("/admin/api/organizations", {
  method: "POST",
  cookie: adminCookie,
  body: { slug, display_name: `RV3 Org ${stamp}` },
})
const organization = orgResponse.body
check("组织创建成功", orgResponse.status === 201 && Boolean(organization?.id), orgResponse.text)
const scopeResponse = await adminProxy(`/admin/api/organizations/${organization.id}/admin-scopes`, {
  method: "POST",
  cookie: adminCookie,
  body: { admin_user_id: Number(process.env.ADMIN_USER_ID) },
})
check("平台管理员取得组织 scope", scopeResponse.status === 201, scopeResponse.text)
await adminProxy(`/admin/api/organizations/${organization.id}/memberships`, {
  method: "POST",
  cookie: adminCookie,
  body: { user_id: Number(MEMBER_USER_ID), display_name: `RV3 Member ${stamp}` },
})
const runtimeResponse = await adminProxy(`/admin/api/organizations/${organization.id}/runtimes`, {
  method: "POST",
  cookie: adminCookie,
  body: {
    name: `rv3-a2a-${stamp}`,
    display_name: "RV3 A2A Fixture",
    endpoint_url: A2A_ENDPOINT,
    credential_alias: A2A_CREDENTIAL_ALIAS,
  },
})
const externalRuntime = runtimeResponse.body
check("外部 A2A Runtime 登记为 Ready 前提", runtimeResponse.status === 201 && Boolean(externalRuntime?.id), runtimeResponse.text)
const agentResponse = await adminProxy(`/admin/api/organizations/${organization.id}/agents`, {
  method: "POST",
  cookie: adminCookie,
  body: {
    runtime_connection_id: externalRuntime.id,
    name: `rv3-agent-${stamp}`,
    display_name: "RV3 Outage Agent",
    description: "RV3 tracing outage agent",
    prompt: "You are the RV3 outage agent.",
  },
})
const agent = agentResponse.body
check("Agent 创建成功", agentResponse.status === 201 && Boolean(agent?.id), agentResponse.text)
const rulesResponse = await adminProxy(`/admin/api/organizations/${organization.id}/agents/${agent.id}/access-rules`, {
  method: "PUT",
  cookie: adminCookie,
  body: { rules: [{ effect: "allow", subject: { kind: "organization", id: organization.id } }] },
})
check("Agent 组织 allow 规则写入成功", rulesResponse.status === 200, rulesResponse.text)

async function sendMessage(content, idempotencyKey) {
  const sessionResponse = await http(AGENT_SERVICE_URL, `/api/v1/organizations/${organization.id}/sessions`, {
    token: AGENT_SERVICE_TOKEN,
    cookie: memberCookie,
    body: { agent_id: agent.id },
  })
  const sessionId = sessionResponse.body?.id ?? ""
  if (sessionResponse.status !== 201 || !sessionId) {
    throw new Error(`创建 Session 失败：${sessionResponse.status} ${sessionResponse.text}`)
  }
  const response = await http(AGENT_SERVICE_URL, `/api/v1/sessions/${sessionId}/messages`, {
    token: AGENT_SERVICE_TOKEN,
    cookie: memberCookie,
    headers: { "idempotency-key": idempotencyKey },
    body: { agent_id: agent.id, content, resource_ids: [], skill_ids: [] },
  })
  return { sessionId, response, runId: response.body?.run?.run_id ?? "" }
}

async function waitSucceeded(runId, timeout = 60000) {
  return eventually(async () => {
    const response = await http(AGENT_SERVICE_URL, `/api/v1/runs/${runId}`, {
      token: AGENT_SERVICE_TOKEN,
      cookie: memberCookie,
    })
    return response.status === 200 && ["succeeded", "failed", "cancelled"].includes(response.body?.status)
  }, timeout)
}

// 2. 停机前基线：确认 tracing 在线且能查询到一个真实 Run。
const warm = await sendMessage(`warm rv3 ${stamp}`, `rv3-warm-${stamp}`)
check("停机前基线 Run 创建成功", warm.response.status === 201 && Boolean(warm.runId), warm.response.text)
check("停机前基线 Run 成功", await waitSucceeded(warm.runId), warm.runId)
const warmReady = await eventually(async () => {
  const response = await tracingExport(warm.runId)
  return response.status === 200 && response.body?.status === "succeeded"
}, 60000, 500)
const warmEvents = tracingExport(warm.runId)
const warmSeqs = warmReady ? (await warmEvents).body?.events?.map((event) => event.seq) ?? [] : []
check(
  "停机前 tracing fresh query 到基线 Run 的完整事件",
  warmReady && warmSeqs.length > 0 && warmSeqs.every((seq, index) => seq === index + 1),
  JSON.stringify(warmSeqs),
)
mark(`baseline run ${warm.runId} tracing events ${warmSeqs.length}`)

// 3. 真实停止 tracing 进程，并确认端口不再可用。
const tracingPid = Number(readFileSync(TRACING_PID_FILE, "utf8").trim())
check("读取 tracing 进程 PID", Number.isInteger(tracingPid) && tracingPid > 0)
process.kill(tracingPid, "SIGTERM")
const tracingDown = await eventually(async () => !(await tracingAlive()), 30000, 200)
check("tracing 进程已真实停止且 /live 不可用", tracingDown)
mark(`tracing stopped pid=${tracingPid}`)

// 4. tracing 不可用期间 Run 仍按 A2A adapter 完成；Agent 保留 pending outbox。
const outage = await sendMessage(`DELAYED rv3 ${stamp}`, `rv3-outage-${stamp}`)
const outageRunId = outage.runId
check("tracing 停机期间创建 Run", outage.response.status === 201 && Boolean(outageRunId), outage.response.text)
const outageSucceeded = await waitSucceeded(outageRunId, 90000)
const outageDb = runDbRow(outageRunId)
const outageOutbox = outboxRow(outageRunId)
const outageTracingCount = Number(tracingScalar(`SELECT count(*) FROM agent_runs WHERE run_id = '${outageRunId}'`))
const agentReadyDuringOutage = await http(AGENT_SERVICE_URL, "/ready")
check(
  "tracing 停机期间 Run 仍成功，Agent 健康且 tracing 无该 Run 记录",
  outageSucceeded &&
    outageDb?.status === "succeeded" &&
    outageDb?.trace_delivered_through_seq === 0 &&
    outageDb?.last_event_seq > 0 &&
    outageOutbox?.pending > 0 &&
    outageOutbox?.delivered === 0 &&
    outageTracingCount === 0 &&
    agentReadyDuringOutage.status === 200,
  JSON.stringify({ outageDb, outageOutbox, outageTracingCount, ready: agentReadyDuringOutage.status }),
)
mark(`outage run ${outageRunId} succeeded with ${outageOutbox?.pending} pending outbox rows`)

// 5. 重启 tracing，outbox 应有序补投。
const tracingLogFd = openSync(TRACING_LOG_FILE, "a")
const restarted = spawn(TRACING_START_SCRIPT, [], {
  detached: true,
  stdio: ["ignore", tracingLogFd, tracingLogFd],
})
restarted.unref()
writeFileSync(TRACING_PID_FILE, String(restarted.pid))
const tracingRestarted = await eventually(tracingAlive, 60000, 300)
check("tracing 进程已重启并恢复 /ready", tracingRestarted)
mark(`tracing restarted pid=${restarted.pid}`)

const delivered = await eventually(async () => {
  const row = outboxRow(outageRunId)
  const db = runDbRow(outageRunId)
  return row?.delivered === row?.pending + row?.delivered && row?.delivered > 0 && db?.trace_delivered_through_seq === db?.last_event_seq
}, 180000, 1000)
const deliveredDb = runDbRow(outageRunId)
const deliveredOutbox = outboxRow(outageRunId)
const exportReady = await eventually(async () => {
  const response = await tracingExport(outageRunId)
  return response.status === 200
}, 120000, 1000)
const exportResponse = await tracingExport(outageRunId)
const tracingEvents = exportResponse.body?.events ?? []
const tracingSeqs = tracingEvents.map((event) => event.seq)
const agentEvents = agentEventSeqs(outageRunId)
const agentSeqs = (agentEvents ?? []).map((event) => event.seq)
check(
  "恢复后 outbox 全部投递且 Agent 游标推进到最后一个事件",
  delivered && deliveredOutbox?.pending === 0 && deliveredDb?.trace_delivered_through_seq === deliveredDb?.last_event_seq,
  JSON.stringify({ deliveredOutbox, deliveredDb }),
)
check(
  "tracing 查询到完整、有序、连续且与 Agent 事件一致的事件序号",
  exportReady &&
    exportResponse.body?.run_id === outageRunId &&
    exportResponse.body?.status === "succeeded" &&
    tracingSeqs.length === deliveredDb?.last_event_seq &&
    tracingSeqs.every((seq, index) => seq === index + 1) &&
    new Set(tracingSeqs).size === tracingSeqs.length &&
    JSON.stringify(tracingSeqs) === JSON.stringify(agentSeqs) &&
    tracingEvents.at(-1)?.event_type === "run.completed",
  JSON.stringify({ tracingSeqs, agentSeqs }),
)
check(
  "tracing DB 中该 Run 的事件唯一数等于事件数",
  Number(tracingScalar(`SELECT count(*) FROM agent_run_events WHERE run_id = '${outageRunId}'`)) === tracingSeqs.length &&
    Number(tracingScalar(`SELECT count(DISTINCT seq) FROM agent_run_events WHERE run_id = '${outageRunId}'`)) === tracingSeqs.length,
)
mark(`recovery delivered ${tracingSeqs.length} events in order`)

// 6. 重复投递：强制重放 outbox，tracing 幂等键必须拒绝重复事件。
agentScalar(`UPDATE agent_runs SET trace_delivered_through_seq = 0 WHERE id = '${outageRunId}'`)
agentScalar(
  `UPDATE agent_trace_outbox SET status = 'pending', attempts = 0, next_attempt_at = now(), delivered_at = NULL
   WHERE run_id = '${outageRunId}'`,
)
const replayDelivered = await eventually(async () => {
  const db = runDbRow(outageRunId)
  const row = outboxRow(outageRunId)
  return row?.pending === 0 && row?.delivered > 0 && db?.trace_delivered_through_seq === db?.last_event_seq
}, 120000, 1000)
await sleep(1000)
const afterReplayCount = Number(tracingScalar(`SELECT count(*) FROM agent_run_events WHERE run_id = '${outageRunId}'`))
const afterReplayDistinct = Number(
  tracingScalar(`SELECT count(DISTINCT seq) FROM agent_run_events WHERE run_id = '${outageRunId}'`),
)
const afterReplayExport = await tracingExport(outageRunId)
const afterReplaySeqs = afterReplayExport.body?.events?.map((event) => event.seq) ?? []
check(
  "重复投递不产生重复事件，查询序号仍唯一且完整",
  replayDelivered &&
    afterReplayCount === tracingSeqs.length &&
    afterReplayDistinct === tracingSeqs.length &&
    afterReplayExport.body?.event_count === tracingSeqs.length &&
    afterReplaySeqs.length === tracingSeqs.length &&
    new Set(afterReplaySeqs).size === afterReplaySeqs.length,
  JSON.stringify({ afterReplayCount, afterReplayDistinct, eventCount: afterReplayExport.body?.event_count }),
)
mark(`duplicate delivery rejected; still ${afterReplayCount} unique events`)

const agentLog = readTextFile(AGENT_LOG_FILE)
check("tracing 故障期间 Agent 未崩溃且未记录敏感内容", agentLog.length > 0 && !/panic|SIGSEGV/.test(agentLog))
check("时间线覆盖停机、Run 完成、恢复与重放", timeline.length >= 4, timeline.join(" | "))

process.exit(exitCode())

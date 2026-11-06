// 本地 A2A 1.0 JSON-RPC 协议替身，只用于外部 Runtime 端点。
//
// 真实入口仍是 agent-service 的 A2A adapter 与 HTTP 客户端；本进程只模拟远端
// Agent Card、SendMessage、GetTask 与 CancelTask。它不进入 Agent 服务内部，
// 也不绕过鉴权：Card 与 RPC 都要求 Bearer token。
//
// 环境变量：
//   A2A_FIXTURE_PORT       监听端口（必填）
//   A2A_FIXTURE_TOKEN      Bearer token（必填）
//   A2A_FIXTURE_DELAY_MS   DELAYED 场景的完成延迟（默认 2500）
//
// 行为（按用户消息中的标记）：
//   FAIL     立即返回 TASK_STATE_FAILED（负控：适配器应写 Run Failed）
//   SLOW     返回 TASK_STATE_WORKING，GetTask 保持 WORKING，直到 CancelTask
//   DELAYED  返回 WORKING，GetTask 在延迟后返回 COMPLETED
//   其他     立即返回 COMPLETED 与回显文本

import { createServer } from "node:http"
import { randomUUID } from "node:crypto"

const PORT = Number(process.env.A2A_FIXTURE_PORT)
const TOKEN = process.env.A2A_FIXTURE_TOKEN
const DELAY_MS = Number(process.env.A2A_FIXTURE_DELAY_MS ?? "2500")

if (!PORT || !TOKEN) {
  console.error("请设置 A2A_FIXTURE_PORT 与 A2A_FIXTURE_TOKEN")
  process.exit(2)
}

/** @type {Map<string, {state: string, text: string, completeAt: number|null, canceled: boolean}>} */
const tasks = new Map()
const state = {
  cardRequests: 0,
  rpcCalls: 0,
  sendMessageCalls: 0,
  getTaskCalls: 0,
  cancelTaskCalls: 0,
  unauthorizedCalls: 0,
  versionHeaderViolations: 0,
  tasks,
}

function bearerToken(request) {
  const header = request.headers.authorization ?? ""
  return header.startsWith("Bearer ") ? header.slice("Bearer ".length) : ""
}

function authorized(request) {
  const authorizedRequest = bearerToken(request) === TOKEN
  if (!authorizedRequest) state.unauthorizedCalls += 1
  if (request.headers["a2a-version"] !== "1.0") {
    state.versionHeaderViolations += 1
    return false
  }
  return authorizedRequest
}

function sendJson(response, status, body) {
  const payload = JSON.stringify(body)
  response.writeHead(status, {
    "content-type": "application/json",
    "content-length": Buffer.byteLength(payload),
  })
  response.end(payload)
}

function card(origin) {
  return {
    name: "E2E A2A Fixture",
    version: "1.0.0",
    supportedInterfaces: [
      {
        url: `${origin}/rpc`,
        protocolBinding: "JSONRPC",
        protocolVersion: "1.0",
      },
    ],
    capabilities: { streaming: false },
    defaultInputModes: ["text/plain"],
    defaultOutputModes: ["text/plain"],
    securitySchemes: {
      bearer: { httpAuthSecurityScheme: { scheme: "Bearer" } },
    },
    securityRequirements: [{ schemes: { bearer: { list: [] } } }],
    skills: [],
  }
}

function messageText(message) {
  const parts = Array.isArray(message?.parts) ? message.parts : []
  return parts
    .map((part) => (typeof part?.text === "string" ? part.text : ""))
    .join("")
}

function agentMessage(text) {
  return { role: "ROLE_AGENT", parts: [{ text }] }
}

function taskPayload(taskId, taskStatus) {
  return { id: taskId, status: taskStatus }
}

function readBody(request) {
  return new Promise((resolve, reject) => {
    const chunks = []
    request.on("data", (chunk) => chunks.push(chunk))
    request.on("end", () => resolve(Buffer.concat(chunks).toString("utf8")))
    request.on("error", reject)
  })
}

function handleSendMessage(params) {
  state.sendMessageCalls += 1
  const text = messageText(params?.message)
  const taskId = `task-${randomUUID()}`
  if (text.includes("FAIL")) {
    const failed = {
      state: "TASK_STATE_FAILED",
      message: agentMessage("fixture controlled failure"),
    }
    tasks.set(taskId, { state: failed.state, text: "", completeAt: null, canceled: false })
    return { task: taskPayload(taskId, failed) }
  }
  if (text.includes("SLOW")) {
    tasks.set(taskId, {
      state: "TASK_STATE_WORKING",
      text: "",
      completeAt: null,
      canceled: false,
    })
    return { task: taskPayload(taskId, { state: "TASK_STATE_WORKING" }) }
  }
  if (text.includes("DELAYED")) {
    tasks.set(taskId, {
      state: "TASK_STATE_WORKING",
      text: "E2E A2A delayed reply",
      completeAt: Date.now() + DELAY_MS,
      canceled: false,
    })
    return { task: taskPayload(taskId, { state: "TASK_STATE_WORKING" }) }
  }
  const reply = `E2E A2A reply: ${text.replace(/\s+/g, " ").slice(0, 120)}`
  tasks.set(taskId, {
    state: "TASK_STATE_COMPLETED",
    text: reply,
    completeAt: Date.now(),
    canceled: false,
  })
  return {
    task: taskPayload(taskId, {
      state: "TASK_STATE_COMPLETED",
      message: agentMessage(reply),
    }),
  }
}

function handleGetTask(params) {
  state.getTaskCalls += 1
  const task = tasks.get(params?.id)
  if (!task) {
    return { id: params?.id ?? "unknown", status: { state: "TASK_STATE_REJECTED" } }
  }
  if (task.canceled) {
    return taskPayload(params.id, { state: "TASK_STATE_CANCELED" })
  }
  if (task.state === "TASK_STATE_WORKING" && task.completeAt !== null) {
    if (Date.now() >= task.completeAt) {
      task.state = "TASK_STATE_COMPLETED"
    }
  }
  if (task.state === "TASK_STATE_COMPLETED") {
    return taskPayload(params.id, {
      state: "TASK_STATE_COMPLETED",
      message: agentMessage(task.text),
    })
  }
  return taskPayload(params.id, { state: task.state })
}

function handleCancelTask(params) {
  state.cancelTaskCalls += 1
  const task = tasks.get(params?.id)
  if (task) {
    task.canceled = true
    task.state = "TASK_STATE_CANCELED"
  }
  return taskPayload(params?.id ?? "unknown", { state: "TASK_STATE_CANCELED" })
}

const server = createServer(async (request, response) => {
  const origin = `http://${request.headers.host}`
  const url = new URL(request.url ?? "/", origin)

  if (request.method === "GET" && url.pathname === "/") {
    state.cardRequests += 1
    if (!authorized(request)) {
      sendJson(response, 401, { error: "unauthorized" })
      return
    }
    sendJson(response, 200, card(origin))
    return
  }

  if (request.method === "GET" && url.pathname === "/_state") {
    const taskEntries = [...tasks.entries()].map(([id, task]) => ({
      id,
      state: task.state,
      canceled: task.canceled,
    }))
    sendJson(response, 200, {
      cardRequests: state.cardRequests,
      rpcCalls: state.rpcCalls,
      sendMessageCalls: state.sendMessageCalls,
      getTaskCalls: state.getTaskCalls,
      cancelTaskCalls: state.cancelTaskCalls,
      unauthorizedCalls: state.unauthorizedCalls,
      versionHeaderViolations: state.versionHeaderViolations,
      tasks: taskEntries,
    })
    return
  }

  if (request.method === "POST" && url.pathname === "/rpc") {
    state.rpcCalls += 1
    if (!authorized(request)) {
      sendJson(response, 401, { error: "unauthorized" })
      return
    }
    let envelope
    try {
      envelope = JSON.parse(await readBody(request))
    } catch {
      sendJson(response, 400, { jsonrpc: "2.0", id: null, error: { code: -32700 } })
      return
    }
    let result
    try {
      if (envelope.method === "SendMessage") {
        result = handleSendMessage(envelope.params)
      } else if (envelope.method === "GetTask") {
        result = handleGetTask(envelope.params)
      } else if (envelope.method === "CancelTask") {
        result = handleCancelTask(envelope.params)
      } else {
        sendJson(response, 200, {
          jsonrpc: "2.0",
          id: envelope.id,
          error: { code: -32601, message: "method not found" },
        })
        return
      }
    } catch {
      sendJson(response, 200, {
        jsonrpc: "2.0",
        id: envelope.id,
        error: { code: -32603, message: "internal error" },
      })
      return
    }
    sendJson(response, 200, { jsonrpc: "2.0", id: envelope.id, result })
    return
  }

  sendJson(response, 404, { error: "not_found" })
})

server.listen(PORT, "127.0.0.1", () => {
  console.log(`A2A fixture listening on 127.0.0.1:${PORT}`)
})

for (const signal of ["SIGTERM", "SIGINT"]) {
  process.on(signal, () => {
    server.close(() => process.exit(0))
  })
}

"use client"

import { useCallback, useEffect, useMemo, useRef, useState } from "react"
import { useTranslations } from "next-intl"
import {
  Bot,
  Download,
  Paperclip,
  Plus,
  Send,
  Square,
  Users,
} from "lucide-react"
import { Badge } from "@/components/ui/badge"
import { Button } from "@/components/ui/button"
import { Card, CardContent, CardHeader, CardTitle } from "@/components/ui/card"
import { Textarea } from "@/components/ui/textarea"
import {
  type AgentArtifact,
  type AgentOrganization,
  type AgentResource,
  type AgentRun,
  type AvailableAgent,
  type ChatMessage,
  type ChatSession,
  type RunEvent,
  type RunStatus,
  artifactDownloadUrl,
  cancelRun,
  createSession,
  getRun,
  getSessionDetail,
  listAvailableAgents,
  listOrganizations,
  listRunArtifacts,
  listRunEvents,
  listResources,
  listSessions,
  sendMessage,
  streamRunEvents,
  uploadResource,
} from "@/lib/api/agent"

/** 进行中的 Run 状态；只有这些状态需要订阅 SSE。 */
const ACTIVE_RUN_STATUSES = new Set<RunStatus>(["queued", "running", "cancelling"])

/** 终端事件类型 → Run 状态。 */
const TERMINAL_EVENTS: Record<string, RunStatus> = {
  "run.completed": "succeeded",
  "run.failed": "failed",
  "run.cancelled": "cancelled",
}

/** 非终端事件类型 → 进行中状态。 */
const PROGRESS_EVENTS: Record<string, RunStatus> = {
  "run.started": "running",
  "run.cancelling": "cancelling",
}

function mergeEvents(current: RunEvent[], incoming: RunEvent[]): RunEvent[] {
  const bySeq = new Map(current.map((event) => [event.seq, event]))
  for (const event of incoming) bySeq.set(event.seq, event)
  return [...bySeq.values()].sort((left, right) => left.seq - right.seq)
}

function formatTime(value: string): string {
  const parsed = new Date(value)
  return Number.isNaN(parsed.getTime()) ? "-" : parsed.toLocaleString()
}

/** Render the agent session workspace. */
export function AgentWorkspace() {
  const t = useTranslations("agents")
  const [organizations, setOrganizations] = useState<AgentOrganization[]>([])
  const [organizationId, setOrganizationId] = useState("")
  const [agents, setAgents] = useState<AvailableAgent[]>([])
  const [agentSelection, setAgentSelection] = useState("auto")
  const [sessions, setSessions] = useState<ChatSession[]>([])
  const [sessionId, setSessionId] = useState("")
  const [messages, setMessages] = useState<ChatMessage[]>([])
  const [sessionRuns, setSessionRuns] = useState<AgentRun[]>([])
  const [sessionAgentId, setSessionAgentId] = useState<string | null>(null)
  const [activeRunId, setActiveRunId] = useState("")
  const [runStatus, setRunStatus] = useState<RunStatus | null>(null)
  const [runSnapshot, setRunSnapshot] = useState<AgentRun | null>(null)
  const [events, setEvents] = useState<RunEvent[]>([])
  const [artifacts, setArtifacts] = useState<AgentArtifact[]>([])
  const [resources, setResources] = useState<AgentResource[]>([])
  const [selectedResourceIds, setSelectedResourceIds] = useState<string[]>([])
  const [draft, setDraft] = useState("")
  const [busy, setBusy] = useState(false)
  const [notice, setNotice] = useState("")
  const streamGeneration = useRef(0)

  const selectedOrganization = organizations.find((item) => item.id === organizationId)

  const showError = useCallback((error: unknown) => {
    setNotice(error instanceof Error ? error.message : String(error))
  }, [])

  useEffect(() => {
    listOrganizations()
      .then((items) => {
        setOrganizations(items)
        setOrganizationId((current) => current || items[0]?.id || "")
      })
      .catch(showError)
  }, [showError])

  useEffect(() => {
    if (!organizationId) return
    Promise.all([listAvailableAgents(organizationId), listSessions(organizationId)])
      .then(([availableAgents, availableSessions]) => {
        setAgents(availableAgents)
        setSessions(availableSessions)
        setAgentSelection("auto")
      })
      .catch(showError)
  }, [organizationId, showError])

  const refreshSession = useCallback(
    async (targetSessionId: string) => {
      const detail = await getSessionDetail(targetSessionId)
      setMessages(detail.messages)
      setSessionRuns(detail.runs)
      setSessionAgentId(detail.session.agent_id)
      return detail
    },
    []
  )

  const resetRunPanel = useCallback(() => {
    setActiveRunId("")
    setEvents([])
    setArtifacts([])
    setRunStatus(null)
    setRunSnapshot(null)
  }, [])

  useEffect(() => {
    if (!activeRunId) return
    streamGeneration.current += 1
    const generation = streamGeneration.current
    const controller = new AbortController()
    let lastSeq = 0

    const load = async () => {
      const [run, initialEvents, runArtifacts] = await Promise.all([
        getRun(activeRunId),
        listRunEvents(activeRunId),
        listRunArtifacts(activeRunId),
      ])
      if (generation !== streamGeneration.current) return
      setRunSnapshot(run)
      const latestProgress = [...initialEvents]
        .reverse()
        .map((event) => PROGRESS_EVENTS[event.event_type])
        .find((status) => status !== undefined)
      setRunStatus(latestProgress ?? run.status)
      lastSeq = initialEvents.at(-1)?.seq ?? 0
      setEvents(mergeEvents([], initialEvents))
      setArtifacts(runArtifacts)
      if (!ACTIVE_RUN_STATUSES.has(run.status) && !latestProgress) return

      await streamRunEvents(
        activeRunId,
        lastSeq,
        (event) => {
          if (generation !== streamGeneration.current) return
          lastSeq = Math.max(lastSeq, event.seq)
          setEvents((current) => mergeEvents(current, [event]))
          const progress = PROGRESS_EVENTS[event.event_type]
          if (progress) setRunStatus(progress)
          const terminal = TERMINAL_EVENTS[event.event_type]
          if (terminal) {
            setRunStatus(terminal)
            controller.abort()
          }
        },
        controller.signal
      ).catch(() => {
        // 取消或连接中断都通过下面的状态刷新收敛。
      })
      if (generation !== streamGeneration.current) return
      const refreshed = await getRun(activeRunId)
      if (generation !== streamGeneration.current) return
      setRunSnapshot(refreshed)
      setRunStatus(refreshed.status)
      setArtifacts(await listRunArtifacts(activeRunId))
      if (sessionId) await refreshSession(sessionId)
    }

    load().catch(showError)
    return () => controller.abort()
  }, [activeRunId, refreshSession, sessionId, showError])

  const activeRun = useMemo(
    () => sessionRuns.find((run) => run.run_id === activeRunId) ?? runSnapshot,
    [activeRunId, runSnapshot, sessionRuns]
  )

  async function handleNewSession() {
    if (!organizationId) return
    setBusy(true)
    try {
      const session = await createSession(organizationId, {
        agent_id: agentSelection === "auto" ? null : agentSelection,
      })
      setSessions((current) => [session, ...current])
      setSessionId(session.id)
      setSessionAgentId(session.agent_id)
      setMessages([])
      setSessionRuns([])
      resetRunPanel()
      setNotice("")
    } catch (error) {
      showError(error)
    } finally {
      setBusy(false)
    }
  }

  function handleOrganizationChange(nextOrganizationId: string) {
    setOrganizationId(nextOrganizationId)
    setSessions([])
    setSessionId("")
    setMessages([])
    setSessionRuns([])
    setSessionAgentId(null)
    setResources([])
    setSelectedResourceIds([])
    resetRunPanel()
  }

  function handleSelectSession(nextSessionId: string) {
    setSessionId(nextSessionId)
    refreshSession(nextSessionId).catch(showError)
  }

  async function handleSend() {
    if (!sessionId || !draft.trim()) return
    setBusy(true)
    try {
      const outcome = await sendMessage(
        sessionId,
        {
          agent_id: sessionAgentId ? null : agentSelection === "auto" ? null : agentSelection,
          content: draft.trim(),
          resource_ids: selectedResourceIds,
        },
        crypto.randomUUID()
      )
      setDraft("")
      setSelectedResourceIds([])
      await refreshSession(sessionId)
      setEvents([])
      setArtifacts([])
      setRunStatus(null)
      setRunSnapshot(null)
      setActiveRunId(outcome.run.run_id)
      setNotice("")
    } catch (error) {
      showError(error)
    } finally {
      setBusy(false)
    }
  }

  async function handleCancel() {
    if (!activeRunId) return
    setBusy(true)
    try {
      const cancelled = await cancelRun(activeRunId)
      setRunSnapshot(cancelled.run)
      setRunStatus(cancelled.run.status)
      if (sessionId) await refreshSession(sessionId)
    } catch (error) {
      showError(error)
    } finally {
      setBusy(false)
    }
  }

  async function handleResourceUpload(file: File | undefined) {
    if (!file || !organizationId) return
    try {
      const uploaded = await uploadResource(organizationId, file)
      setResources((current) => [uploaded, ...current])
      setSelectedResourceIds((current) => [...current, uploaded.id])
    } catch (error) {
      showError(error)
    }
  }

  useEffect(() => {
    if (!organizationId) return
    listResources(organizationId).then(setResources).catch(showError)
  }, [organizationId, showError])

  return (
    <div className="flex flex-col gap-6">
      <div className="flex flex-wrap items-end justify-between gap-3">
        <div>
          <h1 className="text-3xl font-bold">{t("title")}</h1>
          <p className="text-muted-foreground">{t("subtitle")}</p>
        </div>
        <select
          aria-label={t("organization")}
          className="h-9 rounded-lg border bg-background px-3 text-sm"
          data-testid="agent-org-select"
          value={organizationId}
          onChange={(event) => handleOrganizationChange(event.target.value)}
        >
          {organizations.map((organization) => (
            <option key={organization.id} value={organization.id}>
              {organization.display_name}
            </option>
          ))}
        </select>
      </div>

      {notice ? (
        <p className="rounded-lg border border-destructive/40 bg-destructive/10 px-3 py-2 text-sm text-destructive">
          {notice}
        </p>
      ) : null}

      <div className="grid gap-4 xl:grid-cols-[260px_minmax(0,1fr)_340px]">
        <Card>
          <CardHeader>
            <CardTitle className="flex items-center justify-between gap-2 text-base">
              <span className="flex items-center gap-2">
                <Users className="size-4" /> {t("sessions")}
              </span>
              <Button
                size="sm"
                variant="outline"
                data-testid="agent-new-session"
                disabled={busy || !organizationId}
                onClick={handleNewSession}
              >
                <Plus className="size-4" />
                {t("newSession")}
              </Button>
            </CardTitle>
          </CardHeader>
          <CardContent className="space-y-3">
            <select
              aria-label={t("agent")}
              className="h-9 w-full rounded-lg border bg-background px-3 text-sm"
              data-testid="agent-select"
              value={sessionId ? (sessionAgentId ?? "auto") : agentSelection}
              disabled={Boolean(sessionId)}
              onChange={(event) => setAgentSelection(event.target.value)}
            >
              <option value="auto">{t("autoAgent")}</option>
              {agents.map((item) => (
                <option key={item.agent.id} value={item.agent.id}>
                  {item.agent.display_name} · {item.runtime_name}
                </option>
              ))}
            </select>
            <ul className="space-y-2" data-testid="agent-session-list">
              {sessions.map((session) => (
                <li key={session.id}>
                  <button
                    type="button"
                    className={`w-full rounded-lg border px-3 py-2 text-left text-sm transition-colors ${
                      session.id === sessionId
                        ? "border-primary bg-primary/5"
                        : "hover:bg-muted"
                    }`}
                    onClick={() => handleSelectSession(session.id)}
                  >
                    <span className="block truncate font-medium">{session.title}</span>
                    <span className="text-xs text-muted-foreground">
                      {formatTime(session.last_activity_at)}
                    </span>
                  </button>
                </li>
              ))}
            </ul>
            {organizationId ? (
              <div className="space-y-2 border-t pt-3">
                <label className="flex items-center gap-2 text-xs text-muted-foreground">
                  <Paperclip className="size-3" />
                  {t("resources")}
                  <input
                    type="file"
                    className="sr-only"
                    data-testid="agent-resource-input"
                    onChange={(event) => void handleResourceUpload(event.target.files?.[0])}
                  />
                </label>
                {resources.length === 0 ? (
                  <p className="text-xs text-muted-foreground">{t("noResources")}</p>
                ) : (
                  <ul className="space-y-1">
                    {resources.slice(0, 5).map((resource) => (
                      <li key={resource.id}>
                        <label className="flex items-center gap-2 text-xs">
                          <input
                            type="checkbox"
                            checked={selectedResourceIds.includes(resource.id)}
                            onChange={(event) =>
                              setSelectedResourceIds((current) =>
                                event.target.checked
                                  ? [...current, resource.id]
                                  : current.filter((id) => id !== resource.id)
                              )
                            }
                          />
                          <span className="truncate">{resource.filename}</span>
                        </label>
                      </li>
                    ))}
                  </ul>
                )}
              </div>
            ) : null}
            {selectedOrganization ? (
              <p className="border-t pt-3 text-xs text-muted-foreground">
                {t("organizationHint", { slug: selectedOrganization.slug })}
              </p>
            ) : null}
          </CardContent>
        </Card>

        <Card className="flex min-h-[560px] flex-col">
          <CardHeader>
            <CardTitle className="text-base">
              {sessions.find((session) => session.id === sessionId)?.title ?? t("noSession")}
            </CardTitle>
          </CardHeader>
          <CardContent className="flex flex-1 flex-col gap-3">
            <div className="flex-1 space-y-3 overflow-y-auto" data-testid="agent-messages">
              {messages.length === 0 ? (
                <p className="py-10 text-center text-sm text-muted-foreground">
                  {t("emptyMessages")}
                </p>
              ) : (
                messages.map((message) => (
                  <div
                    key={message.id}
                    className={`max-w-[85%] whitespace-pre-wrap rounded-xl px-3 py-2 text-sm ${
                      message.role === "user"
                        ? "ml-auto bg-primary text-primary-foreground"
                        : "bg-muted"
                    }`}
                  >
                    {message.content}
                  </div>
                ))
              )}
            </div>
            <div className="space-y-2 border-t pt-3">
              <Textarea
                data-testid="agent-message-input"
                placeholder={t("messagePlaceholder")}
                value={draft}
                onChange={(event) => setDraft(event.target.value)}
                disabled={!sessionId || busy}
              />
              <div className="flex items-center justify-between gap-2">
                <span className="text-xs text-muted-foreground">
                  {selectedResourceIds.length > 0
                    ? t("selectedResources", { count: selectedResourceIds.length })
                    : t("runHint")}
                </span>
                <Button
                  data-testid="agent-send"
                  disabled={!sessionId || !draft.trim() || busy}
                  onClick={handleSend}
                >
                  <Send className="size-4" />
                  {t("send")}
                </Button>
              </div>
            </div>
          </CardContent>
        </Card>

        <Card>
          <CardHeader>
            <CardTitle className="flex items-center gap-2 text-base">
              <Bot className="size-4" /> {t("runProgress")}
            </CardTitle>
          </CardHeader>
          <CardContent className="space-y-3">
            {!activeRun ? (
              <p className="text-sm text-muted-foreground">{t("noRun")}</p>
            ) : (
              <>
                <div className="flex items-center justify-between gap-2">
                  <Badge variant="secondary" data-testid="agent-run-status">
                    {runStatus ?? activeRun.status}
                  </Badge>
                  <Button
                    size="sm"
                    variant="outline"
                    data-testid="agent-cancel-run"
                    disabled={busy || !runStatus || !ACTIVE_RUN_STATUSES.has(runStatus)}
                    onClick={handleCancel}
                  >
                    <Square className="size-3" />
                    {t("cancel")}
                  </Button>
                </div>
                <dl className="space-y-1 text-xs text-muted-foreground">
                  <div>
                    <dt className="inline">{t("runId")}: </dt>
                    <dd className="inline font-mono break-all" data-testid="agent-run-id">
                      {activeRun.run_id}
                    </dd>
                  </div>
                  <div>
                    <dt className="inline">{t("runtime")}: </dt>
                    <dd className="inline">{activeRun.snapshot.agent.runtime.name}</dd>
                  </div>
                  <div>
                    <dt className="inline">{t("snapshot")}: </dt>
                    <dd className="inline font-mono">
                      {activeRun.snapshot.agent.descriptor_checksum.slice(0, 12)}…
                    </dd>
                  </div>
                  <div>
                    <dt className="inline">{t("trace")}: </dt>
                    <dd className="inline font-mono">
                      #{activeRun.trace_delivered_through_seq} / #{activeRun.last_event_seq}
                    </dd>
                  </div>
                </dl>
                <ol className="max-h-72 space-y-1 overflow-y-auto" data-testid="agent-events">
                  {events.map((event) => (
                    <li
                      key={event.seq}
                      className="grid grid-cols-[34px_1fr] gap-1 border-b py-1 text-xs last:border-0"
                    >
                      <span className="font-mono text-muted-foreground">#{event.seq}</span>
                      <span>
                        <span className="font-mono text-primary">{event.event_type}</span>
                        <span className="block break-all text-muted-foreground">
                          {JSON.stringify(event.payload)}
                        </span>
                      </span>
                    </li>
                  ))}
                </ol>
                <div className="space-y-1 border-t pt-3" data-testid="agent-artifacts">
                  <p className="text-xs font-medium">{t("artifacts")}</p>
                  {artifacts.length === 0 ? (
                    <p className="text-xs text-muted-foreground">{t("noArtifacts")}</p>
                  ) : (
                    artifacts.map((artifact) => (
                      <a
                        key={artifact.id}
                        className="flex items-center justify-between gap-2 rounded border px-2 py-1 text-xs hover:bg-muted"
                        href={artifactDownloadUrl(artifact.id)}
                      >
                        <span className="truncate">{artifact.filename}</span>
                        <span className="flex items-center gap-1 text-muted-foreground">
                          {artifact.size_bytes} B <Download className="size-3" />
                        </span>
                      </a>
                    ))
                  )}
                </div>
              </>
            )}
          </CardContent>
        </Card>
      </div>
    </div>
  )
}

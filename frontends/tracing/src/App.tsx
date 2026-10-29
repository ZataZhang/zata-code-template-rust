import { useCallback, useEffect, useMemo, useState } from 'react'
import { api, ApiError, type AdminIdentity, type RunFilters, type SpanKind, type ThreadFilters, type TraceSpan, type TraceSummary, type TraceTreeNode } from './api'
import { ThreadTimeline, recordMatchesSearch, recordOverlapsRange, type TraceTimelineMode, type TraceTimeRange } from './thread-timeline'

type View = 'runs' | 'threads'
type Notice = { kind: 'error' | 'info'; text: string } | null

const emptyRunFilters: RunFilters = { search: '', status: '', created_after: '', created_before: '', page: 1 }
const emptyThreadFilters: ThreadFilters = { search: '', status: '', created_after: '', created_before: '', page: 1 }

function errorText(error: unknown): string {
  if (error instanceof ApiError) {
    if (error.code === 'admin_auth_unavailable') return '管理员认证服务暂不可用，请检查 accounts 服务。'
    if (error.code === 'export_not_ready') return 'Run 尚未结束或事件序列不完整，暂时不能导出。'
    if (error.code === 'export_too_large') return '事件超出导出限制，请使用事件查询接口分批处理。'
    return error.message
  }
  return error instanceof Error ? error.message : '发生未知错误'
}

function formatDate(value: string | null | undefined): string {
  if (!value) return '—'
  const date = new Date(value)
  if (Number.isNaN(date.valueOf())) return value
  return new Intl.DateTimeFormat('zh-CN', { month: 'short', day: '2-digit', hour: '2-digit', minute: '2-digit', second: '2-digit', hour12: false }).format(date)
}

function formatDuration(milliseconds: number | null | undefined): string {
  if (milliseconds === null || milliseconds === undefined) return '进行中'
  if (milliseconds < 1000) return `${milliseconds} ms`
  if (milliseconds < 60_000) return `${(milliseconds / 1000).toFixed(1)} s`
  return `${Math.floor(milliseconds / 60_000)}m ${Math.round((milliseconds % 60_000) / 1000)}s`
}

function boundedSearch(value: string, maxBytes = 128): string {
  let bounded = ''
  for (const character of value) {
    if (new TextEncoder().encode(bounded + character).length > maxBytes) break
    bounded += character
  }
  return bounded
}

function toApiTimestamp(localValue: string): string {
  if (!localValue) return ''
  const parsed = new Date(localValue)
  return Number.isNaN(parsed.valueOf()) ? '' : parsed.toISOString()
}

function toLocalDateTime(value: string): string {
  if (!value) return ''
  const parsed = new Date(value)
  if (Number.isNaN(parsed.valueOf())) return ''
  const pad = (part: number) => String(part).padStart(2, '0')
  return `${parsed.getFullYear()}-${pad(parsed.getMonth() + 1)}-${pad(parsed.getDate())}T${pad(parsed.getHours())}:${pad(parsed.getMinutes())}`
}

function initials(identity: AdminIdentity): string {
  return (identity.display_name || identity.username).slice(0, 1).toUpperCase()
}

function App() {
  const [admin, setAdmin] = useState<AdminIdentity | null>(null)
  const [authChecked, setAuthChecked] = useState(false)
  const [view, setView] = useState<View>('runs')
  const [runFilters, setRunFilters] = useState(emptyRunFilters)
  const [threadFilters, setThreadFilters] = useState(emptyThreadFilters)
  const [runRows, setRunRows] = useState<TraceSummary[]>([])
  const [runTotal, setRunTotal] = useState(0)
  const [threadRows, setThreadRows] = useState<Awaited<ReturnType<typeof api.listThreads>>['items']>([])
  const [threadTotal, setThreadTotal] = useState(0)
  const [activeRun, setActiveRun] = useState<Awaited<ReturnType<typeof api.getRun>> | null>(null)
  const [activeThread, setActiveThread] = useState<Awaited<ReturnType<typeof api.getThread>> | null>(null)
  const [selectedSpan, setSelectedSpan] = useState<TraceSpan | null>(null)
  const [notice, setNotice] = useState<Notice>(null)
  const [loadingList, setLoadingList] = useState(false)
  const [loadingDetail, setLoadingDetail] = useState(false)
  const [loginName, setLoginName] = useState('')
  const [loginPassword, setLoginPassword] = useState('')
  const [loggingIn, setLoggingIn] = useState(false)

  useEffect(() => {
    let mounted = true
    api.currentAdmin().then((identity) => {
      if (mounted) setAdmin(identity)
    }).catch(() => {
      if (mounted) setAdmin(null)
    }).finally(() => {
      if (mounted) setAuthChecked(true)
    })
    return () => { mounted = false }
  }, [])

  const loadRuns = useCallback(async () => {
    setLoadingList(true)
    try {
      const page = await api.listRuns(runFilters)
      setRunRows(page.items)
      setRunTotal(page.total)
      setNotice(null)
    } catch (error) {
      setNotice({ kind: 'error', text: errorText(error) })
    } finally {
      setLoadingList(false)
    }
  }, [runFilters])

  const loadThreads = useCallback(async () => {
    setLoadingList(true)
    try {
      const page = await api.listThreads(threadFilters)
      setThreadRows(page.items)
      setThreadTotal(page.total)
      setNotice(null)
    } catch (error) {
      setNotice({ kind: 'error', text: errorText(error) })
    } finally {
      setLoadingList(false)
    }
  }, [threadFilters])

  useEffect(() => {
    if (!admin || activeRun || activeThread) return
    if (view === 'runs') void loadRuns()
    else void loadThreads()
  }, [admin, activeRun, activeThread, loadRuns, loadThreads, view])

  async function handleLogin(event: React.FormEvent<HTMLFormElement>) {
    event.preventDefault()
    setLoggingIn(true)
    setNotice(null)
    try {
      const identity = await api.login(loginName, loginPassword)
      setAdmin(identity)
      setLoginPassword('')
    } catch (error) {
      setNotice({ kind: 'error', text: errorText(error) })
    } finally {
      setLoggingIn(false)
    }
  }

  async function handleLogout() {
    try {
      await api.logout()
    } finally {
      setAdmin(null)
      setActiveRun(null)
      setActiveThread(null)
      setSelectedSpan(null)
    }
  }

  async function openRun(runId: string) {
    setLoadingDetail(true)
    setNotice(null)
    try {
      const detail = await api.getRun(runId)
      setActiveRun(detail)
      setActiveThread(null)
      setSelectedSpan(detail.spans[0] ?? null)
    } catch (error) {
      setNotice({ kind: 'error', text: errorText(error) })
    } finally {
      setLoadingDetail(false)
    }
  }

  async function openThread(threadId: string) {
    setLoadingDetail(true)
    setNotice(null)
    try {
      const detail = await api.getThread(threadId)
      setActiveThread(detail)
      setActiveRun(null)
      setSelectedSpan(null)
    } catch (error) {
      setNotice({ kind: 'error', text: errorText(error) })
    } finally {
      setLoadingDetail(false)
    }
  }

  async function exportRun(run: TraceSummary) {
    try {
      await api.exportRun(run.run_id)
      setNotice({ kind: 'info', text: '原始事件导出已开始下载。' })
    } catch (error) {
      setNotice({ kind: 'error', text: errorText(error) })
    }
  }

  const completedRuns = useMemo(() => runRows.filter((run) => run.status !== 'running' && run.status !== 'queued').length, [runRows])

  if (!authChecked) return <div className="boot-screen"><span className="brand-mark">T</span><span>正在连接 Trace Studio…</span></div>
  if (!admin) {
    return (
      <main className="login-page">
        <div className="login-ambient ambient-one" />
        <div className="login-ambient ambient-two" />
        <section className="login-card">
          <div className="brand-lockup"><span className="brand-mark">T</span><span className="brand-word">Trace<span>Studio</span></span></div>
          <div className="login-eyebrow"><span className="live-dot" /> AGENT OBSERVABILITY</div>
          <h1>看见每一次<br /><em>Agent 运行。</em></h1>
          <p className="login-copy">从模型调用到工具结果，把运行过程中的每一步放回完整上下文。</p>
          {notice && <div className={`notice ${notice.kind}`}>{notice.text}</div>}
          <form onSubmit={handleLogin} className="login-form">
            <label>管理员账号<input autoComplete="username" value={loginName} onChange={(event) => setLoginName(event.target.value)} required placeholder="输入用户名" /></label>
            <label>密码<input autoComplete="current-password" type="password" value={loginPassword} onChange={(event) => setLoginPassword(event.target.value)} required placeholder="输入密码" /></label>
            <button className="button primary login-submit" disabled={loggingIn}>{loggingIn ? '正在验证…' : '进入控制台'}<span aria-hidden="true">↗</span></button>
          </form>
          <div className="login-foot"><span>管理员身份由 Accounts Service 验证</span><span className="lock-icon">●</span></div>
        </section>
      </main>
    )
  }

  return (
    <div className="app-shell">
      <aside className="sidebar">
        <div className="brand-lockup sidebar-brand"><span className="brand-mark">T</span><span className="brand-word">Trace<span>Studio</span></span></div>
        <div className="workspace-label">OBSERVABILITY</div>
        <nav className="main-nav" aria-label="主导航">
          <button className={view === 'runs' ? 'nav-item active' : 'nav-item'} onClick={() => { setView('runs'); setActiveRun(null); setActiveThread(null); setRunFilters(emptyRunFilters) }}><span className="nav-icon">⌁</span><span>Agent Runs</span><span className="nav-count">{runTotal}</span></button>
          <button className={view === 'threads' ? 'nav-item active' : 'nav-item'} onClick={() => { setView('threads'); setActiveRun(null); setActiveThread(null); setThreadFilters(emptyThreadFilters) }}><span className="nav-icon">◷</span><span>Threads</span></button>
        </nav>
        <div className="sidebar-spacer" />
        <div className="sidebar-status"><span className="live-dot" /><span>Tracing service online</span></div>
        <div className="sidebar-foot">TRACE PLATFORM <span>v0.1</span></div>
      </aside>

      <div className="main-column">
        <header className="topbar">
          <div className="breadcrumbs"><span>Workspace</span><b>/</b><strong>{activeRun ? 'Run detail' : activeThread ? 'Thread detail' : view === 'runs' ? 'Agent Runs' : 'Threads'}</strong></div>
          <div className="topbar-right"><span className="region-label"><span className="live-dot" /> LOCAL STORE</span><div className="admin-menu"><span className="avatar">{initials(admin)}</span><span className="admin-name">{admin.display_name}</span><button className="text-button" onClick={() => void handleLogout()}>退出</button></div></div>
        </header>

        <main className="content">
          {notice && <div className={`notice ${notice.kind}`} role="status">{notice.text}<button className="notice-close" onClick={() => setNotice(null)} aria-label="关闭">×</button></div>}
          {activeRun ? (
            <RunDetail detail={activeRun} selectedSpan={selectedSpan} onSelectSpan={setSelectedSpan} onBack={() => { setActiveRun(null); setSelectedSpan(null) }} onExport={() => void exportRun(activeRun.summary)} />
          ) : activeThread ? (
            <ThreadDetail detail={activeThread} onBack={() => setActiveThread(null)} onOpenRun={(runId) => void openRun(runId)} />
          ) : view === 'runs' ? (
            <>
              <section className="page-heading"><div><div className="eyebrow">RUNTIME / OVERVIEW</div><h1>Agent Runs<span className="heading-period">.</span></h1><p>查看 Agent 的执行过程、模型调用与工具链路。</p></div><div className="heading-actions"><button className="button secondary" onClick={() => void loadRuns()}><span className="refresh-symbol">↻</span> 刷新</button></div></section>
              <section className="metric-grid">
                <MetricCard label="匹配 Run" value={runTotal.toLocaleString('zh-CN')} detail="当前筛选结果" accent="blue" />
                <MetricCard label="本页已结束" value={completedRuns.toString().padStart(2, '0')} detail="当前页 · 20 条" accent="violet" />
                <MetricCard label="模型调用" value={runRows.reduce((sum, run) => sum + run.model_call_count, 0).toLocaleString('zh-CN')} detail="当前页累计" accent="mint" />
                <MetricCard label="异常步骤" value={runRows.reduce((sum, run) => sum + run.error_count, 0).toLocaleString('zh-CN')} detail="当前页累计" accent="orange" />
              </section>
              <section className="data-panel">
                <div className="panel-heading"><div><h2>运行记录</h2><p>最近创建的运行 · 按时间倒序</p></div><div className="data-meta"><span className="live-dot" /> LIVE DATA</div></div>
                <div className="filter-bar"><label className="search-box"><span>⌕</span><input value={runFilters.search} onChange={(event) => setRunFilters({ ...runFilters, search: boundedSearch(event.target.value), page: 1 })} placeholder="搜索 Run、Agent、来源或项目" /></label><select aria-label="Run 状态" value={runFilters.status} onChange={(event) => setRunFilters({ ...runFilters, status: event.target.value, page: 1 })}><option value="">所有状态</option>{statusOptions.map((status) => <option value={status.value} key={status.value}>{status.label}</option>)}</select><label className="filter-date"><span>从</span><input aria-label="开始时间" type="datetime-local" value={toLocalDateTime(runFilters.created_after)} onChange={(event) => setRunFilters({ ...runFilters, created_after: toApiTimestamp(event.target.value), page: 1 })} /></label><label className="filter-date"><span>到</span><input aria-label="结束时间" type="datetime-local" value={toLocalDateTime(runFilters.created_before)} onChange={(event) => setRunFilters({ ...runFilters, created_before: toApiTimestamp(event.target.value), page: 1 })} /></label><span className="filter-result">{runTotal.toLocaleString('zh-CN')} 条记录</span></div>
                <div className="table-scroll"><table><thead><tr><th>RUN / AGENT</th><th>来源 · 项目</th><th>状态</th><th>调用</th><th>运行时间</th><th>时长</th><th /></tr></thead><tbody>
                  {runRows.map((run) => <tr key={run.run_id} className="clickable-row" onClick={() => void openRun(run.run_id)}><td><div className="run-cell"><span className={`kind-dot ${run.visibility}`} /><div><strong>{run.agent_name || run.agent_id}</strong><small className="mono">{run.run_id}</small></div></div></td><td><span className="source-label">{run.source_id}</span><small className="sub-line">{run.project_id}</small></td><td><StatusPill status={run.status} /></td><td><span className="call-count"><span className="mini-token">M</span> {run.model_call_count}<span className="count-divider">/</span><span className="mini-tool">T</span> {run.tool_call_count}</span></td><td><span className="date-text">{formatDate(run.created_at)}</span></td><td><span className="duration-text">{formatDuration(run.duration_ms)}</span></td><td><button className="row-arrow" aria-label={`查看 ${run.run_id}`}>↗</button></td></tr>)}
                  {!runRows.length && <EmptyRow loading={loadingList} columns={7} />}
                </tbody></table></div>
                <Pagination total={runTotal} page={runFilters.page} pageSize={20} onPage={(page) => setRunFilters({ ...runFilters, page })} />
              </section>
            </>
          ) : (
            <>
              <section className="page-heading"><div><div className="eyebrow">CONVERSATION / HISTORY</div><h1>Threads<span className="heading-period">.</span></h1><p>按会话查看跨 Run 的消息、委派与执行记录。</p></div><div className="heading-actions"><button className="button secondary" onClick={() => void loadThreads()}><span className="refresh-symbol">↻</span> 刷新</button></div></section>
              <section className="metric-grid thread-metrics"><MetricCard label="匹配 Thread" value={threadTotal.toLocaleString('zh-CN')} detail="当前筛选结果" accent="blue" /><MetricCard label="本页会话" value={threadRows.length.toString().padStart(2, '0')} detail="跨 Run 的消息流" accent="violet" /><MetricCard label="关联 Runs" value={threadRows.reduce((sum, thread) => sum + thread.run_count, 0).toLocaleString('zh-CN')} detail="当前页累计" accent="mint" /><MetricCard label="近期活动" value={threadRows.filter((thread) => Date.now() - Date.parse(thread.last_activity_at) < 86_400_000).length.toString().padStart(2, '0')} detail="过去 24 小时" accent="orange" /></section>
              <section className="data-panel"><div className="panel-heading"><div><h2>会话列表</h2><p>跨轮次的 Agent 对话与任务历史</p></div><div className="data-meta"><span className="live-dot" /> LIVE DATA</div></div><div className="filter-bar"><label className="search-box"><span>⌕</span><input value={threadFilters.search} onChange={(event) => setThreadFilters({ ...threadFilters, search: boundedSearch(event.target.value), page: 1 })} placeholder="搜索 Thread、Agent、来源或项目" /></label><select aria-label="会话状态" value={threadFilters.status} onChange={(event) => setThreadFilters({ ...threadFilters, status: event.target.value, page: 1 })}><option value="">所有状态</option>{statusOptions.map((status) => <option value={status.value} key={status.value}>{status.label}</option>)}</select><label className="filter-date"><span>从</span><input aria-label="开始时间" type="datetime-local" value={toLocalDateTime(threadFilters.created_after)} onChange={(event) => setThreadFilters({ ...threadFilters, created_after: toApiTimestamp(event.target.value), page: 1 })} /></label><label className="filter-date"><span>到</span><input aria-label="结束时间" type="datetime-local" value={toLocalDateTime(threadFilters.created_before)} onChange={(event) => setThreadFilters({ ...threadFilters, created_before: toApiTimestamp(event.target.value), page: 1 })} /></label><span className="filter-result">{threadTotal.toLocaleString('zh-CN')} 条会话</span></div><div className="table-scroll"><table><thead><tr><th>THREAD</th><th>来源 · 项目</th><th>AGENT</th><th>RUNS</th><th>状态概览</th><th>最近活动</th><th /></tr></thead><tbody>{threadRows.map((thread) => <tr key={thread.thread_id} className="clickable-row" onClick={() => void openThread(thread.thread_id)}><td><div className="thread-cell"><span className="thread-icon">◷</span><div><strong>{thread.thread_id}</strong><small className="mono">创建于 {formatDate(thread.created_at)}</small></div></div></td><td><span className="source-label">{thread.source_id}</span><small className="sub-line">{thread.project_id}</small></td><td><span className="agent-chip">{thread.agent_names.slice(0, 2).join(' · ') || 'Agent'}</span></td><td><strong className="run-count">{thread.run_count}</strong><small className="sub-line">{thread.limited_run_count} 个基础轨迹</small></td><td><StatusSummary counts={thread.status_counts} /></td><td><span className="date-text">{formatDate(thread.last_activity_at)}</span></td><td><button className="row-arrow" aria-label={`查看 ${thread.thread_id}`}>↗</button></td></tr>)}{!threadRows.length && <EmptyRow loading={loadingList} columns={7} />}</tbody></table></div><Pagination total={threadTotal} page={threadFilters.page} pageSize={20} onPage={(page) => setThreadFilters({ ...threadFilters, page })} /></section>
            </>
          )}
        </main>
      </div>
      {loadingDetail && <div className="loading-overlay"><span className="spinner" />正在读取轨迹…</div>}
    </div>
  )
}

const statusOptions: { value: string; label: string }[] = [
  { value: 'queued', label: '排队中' }, { value: 'running', label: '运行中' }, { value: 'cancelling', label: '取消中' },
  { value: 'succeeded', label: '成功' }, { value: 'failed', label: '失败' }, { value: 'cancelled', label: '已取消' }, { value: 'interrupted', label: '中断' },
]

function MetricCard({ label, value, detail, accent }: { label: string; value: string; detail: string; accent: string }) {
  return <article className={`metric-card ${accent}`}><div className="metric-top"><span>{label}</span><span className="metric-spark">⌁</span></div><div className="metric-value">{value}</div><div className="metric-detail"><span className="metric-dot" />{detail}</div></article>
}

function StatusPill({ status }: { status: string }) {
  const labels: Record<string, string> = { queued: '排队中', running: '运行中', cancelling: '取消中', succeeded: '成功', failed: '失败', cancelled: '已取消', interrupted: '中断' }
  return <span className={`status-pill ${status}`}><span />{labels[status] ?? status}</span>
}

function StatusSummary({ counts }: { counts: Record<string, number> }) {
  const active = Object.entries(counts).filter(([, count]) => count > 0)
  return <div className="status-summary">{active.slice(0, 3).map(([status, count]) => <span key={status} title={`${status}: ${count}`}><i className={status} />{count}</span>)}</div>
}

function EmptyRow({ loading, columns }: { loading: boolean; columns: number }) {
  return <tr><td colSpan={columns} className="empty-state"><div className="empty-glyph">⌁</div><strong>{loading ? '读取中…' : '这里还没有运行记录'}</strong><span>{loading ? '正在从 tracing 服务获取最新数据。' : '接入 Agent producer 后，运行事件会出现在这里。'}</span></td></tr>
}

function Pagination({ total, page, pageSize, onPage }: { total: number; page: number; pageSize: number; onPage: (page: number) => void }) {
  const pages = Math.max(1, Math.ceil(total / pageSize))
  return <div className="pagination"><span>显示 <strong>{total ? (page - 1) * pageSize + 1 : 0}–{Math.min(page * pageSize, total)}</strong> / {total.toLocaleString('zh-CN')}</span><div className="page-buttons"><button disabled={page <= 1} onClick={() => onPage(page - 1)} aria-label="上一页">←</button><span>{page} <i>/</i> {pages}</span><button disabled={page >= pages} onClick={() => onPage(page + 1)} aria-label="下一页">→</button></div></div>
}

function RunDetail({ detail, selectedSpan, onSelectSpan, onBack, onExport }: { detail: NonNullable<Awaited<ReturnType<typeof api.getRun>>>; selectedSpan: TraceSpan | null; onSelectSpan: (span: TraceSpan) => void; onBack: () => void; onExport: () => void }) {
  const summary = detail.summary
  return <section className="detail-page"><button className="back-link" onClick={onBack}>← 返回运行列表</button><div className="detail-heading"><div><div className="eyebrow">TRACE / {summary.source_id.toUpperCase()}</div><h1>{summary.agent_name || summary.agent_id}<span className="heading-period">.</span></h1><div className="detail-id-row"><span className="mono">{summary.run_id}</span><StatusPill status={summary.status} /></div></div><button className="button secondary export-button" disabled={!['succeeded', 'failed', 'cancelled', 'interrupted'].includes(summary.status)} onClick={onExport}><span>↓</span> 导出事件</button></div>
    <div className="detail-meta-grid"><MetaCell label="PROJECT" value={summary.project_id} /><MetaCell label="THREAD" value={summary.thread_id} /><MetaCell label="TRACE ID" value={summary.trace_id} mono /><MetaCell label="运行时长" value={formatDuration(summary.duration_ms)} /></div>
    <div className="detail-columns"><section className="trace-panel"><div className="trace-panel-head"><div><h2>执行轨迹</h2><p>{detail.spans.length} 个 span · 内容策略 {detail.content_policy}</p></div><span className="trace-legend"><i className="model" />模型 <i className="tool" />工具 <i className="decision" />判定</span></div>
      <div className="trace-tree">{detail.tree.length ? detail.tree.map((node) => <TraceNode key={node.span.span_id} node={node} depth={0} selected={selectedSpan?.span_id} onSelect={onSelectSpan} />) : <div className="trace-empty"><span>⌁</span><strong>暂无可视化 Span</strong><small>已收到 {summary.status} 生命周期事件，但没有识别到模型或工具节点。</small></div>}</div>
      {detail.diagnostics.length > 0 && <div className="diagnostic-block"><h3>诊断提示 <span>{detail.diagnostics.length}</span></h3>{detail.diagnostics.map((item, index) => <div className="diagnostic-row" key={`${item.code}-${index}`}><span>!</span><div><strong>{item.code}</strong><small>{item.message}</small></div></div>)}</div>}
    </section><SpanInspector span={selectedSpan} /></div></section>
}

function MetaCell({ label, value, mono = false }: { label: string; value: string; mono?: boolean }) {
  return <div className="meta-cell"><span>{label}</span><strong className={mono ? 'mono' : ''} title={value}>{value}</strong></div>
}

function TraceNode({ node, depth, selected, onSelect }: { node: TraceTreeNode; depth: number; selected?: string; onSelect: (span: TraceSpan) => void }) {
  const span = node.span
  return <div className="trace-node-group"><button className={`trace-node ${selected === span.span_id ? 'selected' : ''}`} style={{ '--depth': depth } as React.CSSProperties} onClick={() => onSelect(span)}><span className={`span-kind ${span.kind}`}>{kindLetter(span.kind)}</span><span className="span-main"><strong>{span.name}</strong><small>{span.model_name || span.tool_name || `${span.event_seq} events`}</small></span><span className={`span-status ${span.status}`}><i />{span.status === 'completed' ? formatDuration(span.duration_ms) : span.status === 'running' ? '运行中' : span.status === 'failed' ? '失败' : '不完整'}</span></button>{node.children.map((child) => <TraceNode key={child.span.span_id} node={child} depth={depth + 1} selected={selected} onSelect={onSelect} />)}</div>
}

function kindLetter(kind: SpanKind): string {
  return ({ root: 'R', model: 'M', tool: 'T', artifact: 'A', decision: 'D' })[kind]
}

function SpanInspector({ span }: { span: TraceSpan | null }) {
  if (!span) return <aside className="inspector-panel"><div className="inspector-empty"><span>⌖</span><strong>选择一个轨迹节点</strong><small>查看模型、工具或策略判定的完整细节。</small></div></aside>
  return <aside className="inspector-panel"><div className="inspector-title"><div><span className={`span-kind ${span.kind}`}>{kindLetter(span.kind)}</span><div><h2>节点详情</h2><small>{span.kind.toUpperCase()} SPAN</small></div></div><span className={`inspector-state ${span.status}`}>{span.status}</span></div><div className="inspector-body"><div className="inspector-name">{span.name}</div><div className="inspector-facts"><Fact label="SPAN ID" value={span.span_id} mono /><Fact label="PARENT" value={span.parent_span_id ?? '—'} mono /><Fact label="事件序号" value={`#${span.event_seq}`} /><Fact label="开始时间" value={formatDate(span.started_at)} /><Fact label="持续时间" value={formatDuration(span.duration_ms)} />{span.model_name && <Fact label="模型" value={span.model_name} />}{span.turn_index !== null && <Fact label="轮次" value={String(span.turn_index)} />}{span.finish_reason && <Fact label="结束原因" value={span.finish_reason} />}{span.tool_call_id && <Fact label="工具调用 ID" value={span.tool_call_id} mono />}{span.result_checksum && <Fact label="结果校验" value={span.result_checksum} mono />}{span.result_length !== null && <Fact label="结果字符数" value={String(span.result_length)} />}</div>
      {span.usage && <JsonSection title="Token Usage" value={span.usage} />}{span.tool_arguments !== null && <JsonSection title="工具参数" value={span.tool_arguments} />}{span.tool_result !== null && <JsonSection title="工具结果" value={span.tool_result} />}{span.decision && <DecisionSection decision={span.decision} />}{span.error_code && <div className="error-card"><span>!</span><div><strong>{span.error_code}</strong><p>{span.error_message || '发生未说明的步骤错误。'}</p></div></div>}{span.diagnostics.length > 0 && <div className="diagnostic-tags">{span.diagnostics.map((code) => <span key={code}>{code}</span>)}</div>}</div></aside>
}

function Fact({ label, value, mono = false }: { label: string; value: string; mono?: boolean }) {
  return <div className="fact"><span>{label}</span><strong className={mono ? 'mono' : ''}>{value}</strong></div>
}

function JsonSection({ title, value }: { title: string; value: unknown }) {
  return <section className="json-section"><div className="json-title"><span>{title}</span><span>JSON</span></div><pre>{JSON.stringify(value, null, 2)}</pre></section>
}

function DecisionSection({ decision }: { decision: NonNullable<TraceSpan['decision']> }) {
  return <section className="decision-card"><div className="decision-head"><span>POLICY DECISION</span><strong>{decision.verdict}</strong></div><div className="decision-tool">{decision.gate} <b>→</b> {decision.tool_name}</div><div className="decision-answer">{decision.answer}</div><div className="decision-facts"><Fact label="判定耗时" value={`${decision.elapsed_ms} ms`} /><Fact label="Fail open" value={decision.fail_open ? '是' : '否'} /><Fact label="不可逆操作" value={decision.irreversible === null ? '未知' : decision.irreversible ? '是' : '否'} /></div><JsonSection title="判定输入摘要" value={decision.judgement_state} />{Object.keys(decision.probabilities).length > 0 && <JsonSection title="概率分布" value={decision.probabilities} />}</section>
}

function ThreadDetail({ detail, onBack, onOpenRun }: { detail: NonNullable<Awaited<ReturnType<typeof api.getThread>>>; onBack: () => void; onOpenRun: (runId: string) => void }) {
  const [timelineMode, setTimelineMode] = useState<TraceTimelineMode>('time')
  const [selectedRange, setSelectedRange] = useState<TraceTimeRange | null>(null)
  const [searchText, setSearchText] = useState('')
  const [selectedRecordId, setSelectedRecordId] = useState<string | null>(null)
  const records = useMemo(() => [...detail.records].sort((left, right) => {
    const leftTime = left.occurred_at ? Date.parse(left.occurred_at) : Number.POSITIVE_INFINITY
    const rightTime = right.occurred_at ? Date.parse(right.occurred_at) : Number.POSITIVE_INFINITY
    return leftTime - rightTime || left.seq - right.seq || left.run_id.localeCompare(right.run_id) || left.record_id.localeCompare(right.record_id)
  }), [detail.records])
  const matchingCount = records.filter((record) => recordMatchesSearch(record, searchText)).length
  return <section className="detail-page thread-detail"><button className="back-link" onClick={onBack}>← 返回会话列表</button><div className="detail-heading"><div><div className="eyebrow">THREAD / {detail.summary.source_id.toUpperCase()}</div><h1>会话记录<span className="heading-period">.</span></h1><div className="detail-id-row"><span className="mono">{detail.summary.thread_id}</span><span className="run-total-chip">{detail.summary.run_count} RUNS</span></div></div><span className="thread-range">最近活动 · {formatDate(detail.summary.last_activity_at)}</span></div>
    <div className="thread-overview"><div><span>项目</span><strong>{detail.summary.project_id}</strong></div><div><span>Agent</span><strong>{detail.summary.agent_names.join(' · ') || '—'}</strong></div><div><span>运行数量</span><strong>{detail.summary.run_count}</strong></div><div><span>基础轨迹</span><strong>{detail.summary.limited_run_count}</strong></div></div>
    <section className="thread-ledger"><div className="panel-heading"><div><h2>事件账本</h2><p>按发生时间合并消息、委派与运行步骤</p></div><span className="ledger-count">{matchingCount}/{records.length} RECORDS</span></div>
      <div className="ledger-toolbar"><div className="timeline-mode"><button className={timelineMode === 'time' ? 'active' : ''} onClick={() => setTimelineMode('time')}>时间</button><button className={timelineMode === 'sequence' ? 'active' : ''} onClick={() => setTimelineMode('sequence')}>顺序</button></div><label className="ledger-search"><span>⌕</span><input value={searchText} onChange={(event) => setSearchText(event.target.value)} placeholder="搜索消息、步骤或事件内容" /><kbd>⌘ F</kbd></label>{selectedRange && <button className="clear-range" onClick={() => setSelectedRange(null)}>清除时间区间 ×</button>}</div>
      <ThreadTimeline records={records} requestedMode={timelineMode} selectedRange={selectedRange} searchText={searchText} selectedRecordId={selectedRecordId} onRangeChange={setSelectedRange} onSelectRecord={setSelectedRecordId} />
      {selectedRange && <p className="range-status" role="status">时间区间已选中，未交叠记录会淡化显示。</p>}
      <div className="ledger-list">{records.length ? records.map((record) => { const matchesSearch = recordMatchesSearch(record, searchText); const overlapsRange = recordOverlapsRange(record, selectedRange); return <article className={`ledger-item ${!matchesSearch || !overlapsRange ? 'ledger-dimmed' : ''} ${selectedRecordId === record.record_id ? 'ledger-selected' : ''}`} key={record.record_id} onClick={() => setSelectedRecordId(record.record_id)}><div className="ledger-rail"><span className={`ledger-marker ${record.kind}`} /></div><div className="ledger-content"><div className="ledger-meta"><span className={`ledger-type ${record.kind}`}>{record.kind}</span><time>{formatDate(record.occurred_at)}</time><span>#{record.seq}</span><button onClick={(event) => { event.stopPropagation(); onOpenRun(record.run_id) }}>打开 Run ↗</button></div>{record.kind === 'message' ? <div className={`message-card ${record.role ?? 'assistant'}`}><div className="message-role">{record.role ?? 'message'}</div><div className="message-body">{renderContent(record.content)}</div></div> : record.span ? <button className="ledger-span" onClick={(event) => { event.stopPropagation(); onOpenRun(record.run_id) }}><span className={`span-kind ${record.span.kind}`}>{kindLetter(record.span.kind)}</span><span><strong>{record.span.name}</strong><small>{record.span.model_name || record.span.tool_name || record.span.status}</small></span><span className="ledger-go">查看轨迹 →</span></button> : <div className="delegation-card"><strong>{record.kind === 'delegation' ? 'Agent 委派' : record.kind === 'run' ? 'Run 开始' : record.kind}</strong><pre>{JSON.stringify(record.content, null, 2)}</pre>{record.child_run_id && <span>子 Run：{record.child_run_id}</span>}</div>}</div></article>}) : <div className="empty-state ledger-empty"><div className="empty-glyph">◷</div><strong>暂无会话账本记录</strong><span>有新消息或 Agent 步骤后会出现在这里。</span></div>}{records.length > 0 && matchingCount === 0 && <div className="ledger-no-match">没有找到匹配“{searchText}”的记录。</div>}</div>
    </section>
    <section className="thread-runs"><div className="panel-heading"><div><h2>关联 Runs</h2><p>同一会话下的所有执行</p></div></div><div className="run-card-list">{detail.runs.map((run) => <button className="thread-run-card" key={run.summary.run_id} onClick={() => onOpenRun(run.summary.run_id)}><span className="run-card-mark">⌁</span><span className="run-card-info"><strong>{run.summary.agent_name || run.summary.agent_id}</strong><small className="mono">{run.summary.run_id}</small></span><StatusPill status={run.summary.status} /><span className="thread-run-time">{formatDate(run.summary.created_at)}</span><span className="row-arrow">↗</span></button>)}</div></section>
  </section>
}

function renderContent(content: unknown): React.ReactNode {
  if (content === null || content === undefined) return <span className="muted">（空消息）</span>
  if (typeof content === 'string') return content
  if (Array.isArray(content)) return content.map((part, index) => <span key={index}>{typeof part === 'string' ? part : JSON.stringify(part)}</span>)
  return <pre>{JSON.stringify(content, null, 2)}</pre>
}

export default App

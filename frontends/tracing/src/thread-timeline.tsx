import { useRef } from 'react'
import type { ThreadTraceRecord } from './api'

export type TraceTimelineMode = 'time' | 'sequence'
export type TraceTimeRange = { start: number; end: number }
export type TraceLane = 'input' | 'model' | 'tools' | 'decisions' | 'output'
export type TraceTimelineItem = {
  record: ThreadTraceRecord
  lane: TraceLane
  startAt: number | null
  endAt: number | null
  startPercent: number
  endPercent: number
  openEnded: boolean
}

const lanes: TraceLane[] = ['input', 'model', 'tools', 'decisions', 'output']
const laneLabels: Record<TraceLane, string> = {
  input: '用户输入', model: '模型', tools: '工具 / 委派', decisions: '策略判定', output: '输出',
}

export function recordMatchesSearch(record: ThreadTraceRecord, searchText: string): boolean {
  const normalized = searchText.trim().toLocaleLowerCase()
  return !normalized || JSON.stringify(record).toLocaleLowerCase().includes(normalized)
}

export function recordOverlapsRange(record: ThreadTraceRecord, range: TraceTimeRange | null): boolean {
  if (!range) return true
  const { startAt, endAt } = timeEndpoints(record)
  if (startAt === null || endAt === null) return true
  return startAt <= range.end && endAt >= range.start
}

function timelineLane(record: ThreadTraceRecord): TraceLane | null {
  if (record.kind === 'message') return record.role === 'user' ? 'input' : 'output'
  if (record.kind === 'delegation') return 'tools'
  if (record.kind !== 'span' || !record.span) return null
  if (record.span.kind === 'model') return 'model'
  if (record.span.kind === 'tool') return 'tools'
  if (record.span.kind === 'decision') return 'decisions'
  if (record.span.kind === 'artifact') return 'output'
  return null
}

function timeEndpoints(record: ThreadTraceRecord): { startAt: number | null; endAt: number | null } {
  const startValue = record.kind === 'span' && record.span ? record.span.started_at ?? record.span.finished_at : record.occurred_at
  const endValue = record.kind === 'span' && record.span ? record.span.started_at ? record.span.finished_at ?? record.span.started_at : record.span.finished_at : record.occurred_at
  const startAt = startValue ? Date.parse(startValue) : NaN
  const endAt = endValue ? Date.parse(endValue) : NaN
  return { startAt: Number.isFinite(startAt) ? startAt : null, endAt: Number.isFinite(endAt) ? endAt : null }
}

function buildTimeline(records: ThreadTraceRecord[], requestedMode: TraceTimelineMode) {
  const laneRecords = records.flatMap((record) => {
    const lane = timelineLane(record)
    return lane ? [{ record, lane }] : []
  })
  const timestamps = laneRecords.flatMap(({ record }) => {
    const endpoints = timeEndpoints(record)
    return [endpoints.startAt, endpoints.endAt].filter((value): value is number => value !== null)
  })
  const range = timestamps.length ? { start: Math.min(...timestamps), end: Math.max(...timestamps) } : null
  const mode: TraceTimelineMode = requestedMode === 'time' && !range ? 'sequence' : requestedMode
  const items = laneRecords.map(({ record, lane }, index) => {
    const { startAt, endAt } = timeEndpoints(record)
    const openEnded = record.kind === 'span' && startAt !== null && !record.span?.finished_at
    const count = Math.max(laneRecords.length, 1)
    const startPercent = mode === 'sequence' ? (index / count) * 100 : percentage(startAt, range)
    const endPercent = mode === 'sequence' ? ((index + 1) / count) * 100 : Math.max(startPercent, percentage(endAt, range))
    return { record, lane, startAt, endAt, startPercent, endPercent, openEnded }
  })
  return { mode, fellBackToSequence: requestedMode === 'time' && mode === 'sequence', range, lanes: lanes.map((lane) => ({ lane, items: items.filter((item) => item.lane === lane) })) }
}

function percentage(value: number | null, range: TraceTimeRange | null): number {
  if (value === null || !range) return 0
  const duration = range.end - range.start
  return duration > 0 ? ((value - range.start) / duration) * 100 : 50
}

export function ThreadTimeline({ records, requestedMode, selectedRange, searchText, selectedRecordId, onRangeChange, onSelectRecord }: {
  records: ThreadTraceRecord[]
  requestedMode: TraceTimelineMode
  selectedRange: TraceTimeRange | null
  searchText: string
  selectedRecordId: string | null
  onRangeChange: (range: TraceTimeRange | null) => void
  onSelectRecord: (recordId: string) => void
}) {
  const timeline = buildTimeline(records, requestedMode)
  const activeDrag = useRef<{ startX: number; width: number; left: number; range: TraceTimeRange } | null>(null)
  function beginDrag(event: React.PointerEvent<HTMLDivElement>) {
    if (timeline.mode !== 'time' || !timeline.range) return
    const bounds = event.currentTarget.getBoundingClientRect()
    activeDrag.current = { startX: event.clientX, width: bounds.width, left: bounds.left, range: timeline.range }
    event.currentTarget.setPointerCapture(event.pointerId)
  }
  function endDrag(event: React.PointerEvent<HTMLDivElement>) {
    const drag = activeDrag.current
    activeDrag.current = null
    if (!drag || Math.abs(event.clientX - drag.startX) < 5 || drag.width <= 0) return
    const positionPercent = (position: number) => Math.min(100, Math.max(0, ((position - drag.left) / drag.width) * 100))
    const low = positionPercent(Math.min(event.clientX, drag.startX))
    const high = positionPercent(Math.max(event.clientX, drag.startX))
    const duration = drag.range.end - drag.range.start
    if (duration <= 0) return
    onRangeChange({ start: drag.range.start + duration * low / 100, end: drag.range.start + duration * high / 100 })
  }

  return <section className="trace-timeline" aria-label="会话时间轴">
    {timeline.fellBackToSequence && <p className="timeline-fallback" role="status">没有可用时间戳，已切换为事件顺序。</p>}
    {timeline.lanes.map(({ lane, items }) => <div className="timeline-lane" key={lane}><span>{laneLabels[lane]}</span><div className="timeline-track" onPointerDown={beginDrag} onPointerUp={endDrag} aria-label={laneLabels[lane]}>
      {selectedRange && timeline.range && <div className="timeline-selection" style={rangeStyle(selectedRange, timeline.range)} />}
      {items.map((item) => {
        const matched = recordMatchesSearch(item.record, searchText)
        const isPointInTime = item.startAt !== null && item.startAt === item.endAt
        const widthPercent = Math.max(item.endPercent - item.startPercent, timeline.mode === 'time' ? .8 : 2.5)
        const useFixedWidthMarker = timeline.mode === 'time' && (isPointInTime || widthPercent < 1.5)
        const markerWidthPx = item.openEnded ? 12 : 8
        const pointRightInsetPx = 8
        const left = useFixedWidthMarker
          ? `clamp(0px, calc(${item.startPercent}% - ${markerWidthPx / 2}px), calc(100% - ${markerWidthPx + pointRightInsetPx}px))`
          : timeline.mode === 'time'
            ? `min(${item.startPercent}%, calc(100% - ${widthPercent}% - ${pointRightInsetPx}px))`
            : `${item.startPercent}%`
        const width = useFixedWidthMarker
          ? `${markerWidthPx}px`
          : `${widthPercent}%`
        const kind = item.record.span?.kind ?? item.record.kind
        return <button key={item.record.record_id} aria-label={`${item.record.run_id} ${item.record.span?.name ?? item.record.role ?? ''}`} aria-pressed={selectedRecordId === item.record.record_id} title={`${kind} · ${item.record.span?.name ?? item.record.role ?? item.record.run_id}`} className={`timeline-mark ${item.lane} ${matched ? 'matches-search' : 'misses-search'} ${selectedRecordId === item.record.record_id ? 'selected' : ''}`} style={{ left, width }} onClick={() => onSelectRecord(item.record.record_id)} />
      })}
      {!items.length && <span className="timeline-empty-mark" />}
    </div></div>)}
    {timeline.lanes.every(({ items }) => items.length === 0) && <p className="timeline-empty">暂无可绘制的消息或步骤。</p>}
    {timeline.mode === 'time' && timeline.range && <div className="timeline-times"><time>{new Date(timeline.range.start).toLocaleTimeString([], { hour: '2-digit', minute: '2-digit', second: '2-digit' })}</time><time>{new Date(timeline.range.end).toLocaleTimeString([], { hour: '2-digit', minute: '2-digit', second: '2-digit' })}</time></div>}
  </section>
}

function rangeStyle(selection: TraceTimeRange, complete: TraceTimeRange): React.CSSProperties {
  const duration = complete.end - complete.start
  if (duration <= 0) return { left: '0%', width: '100%' }
  const start = Math.max(0, Math.min(100, ((selection.start - complete.start) / duration) * 100))
  const end = Math.max(0, Math.min(100, ((selection.end - complete.start) / duration) * 100))
  return { left: `${start}%`, width: `${Math.max(0, end - start)}%` }
}

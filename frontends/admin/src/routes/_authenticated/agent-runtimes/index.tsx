import { createFileRoute } from '@tanstack/react-router'
import { AgentRuntimes } from '@/features/agent-runtimes'

export const Route = createFileRoute('/_authenticated/agent-runtimes/')({
  component: AgentRuntimes,
})

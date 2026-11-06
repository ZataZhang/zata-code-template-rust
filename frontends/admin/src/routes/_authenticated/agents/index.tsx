import { createFileRoute } from '@tanstack/react-router'
import { AgentCatalog } from '@/features/agents'

export const Route = createFileRoute('/_authenticated/agents/')({
  component: AgentCatalog,
})

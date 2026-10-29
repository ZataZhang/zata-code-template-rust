import { createFileRoute, redirect } from '@tanstack/react-router'

// 目前后台只有「用户管理」一个功能，首页直接进入它。
export const Route = createFileRoute('/_authenticated/')({
  beforeLoad: () => {
    throw redirect({ to: '/users' })
  },
})

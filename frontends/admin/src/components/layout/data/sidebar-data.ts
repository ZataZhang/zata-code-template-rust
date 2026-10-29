import { type TFunction } from 'i18next'
import { Users } from 'lucide-react'
import { type NavGroup } from '../types'

/**
 * 构造随语言变化的侧边栏导航分组。
 *
 * 标题通过 i18next 的 `t` 读取，语言切换后侧边栏会随组件重渲染一起更新。
 * 调用方每次渲染都重新构造即可——这里只是组装对象字面量，没有需要缓存的昂贵计算。
 *
 * @param t - i18next 翻译函数（默认命名空间为 `translation`）。
 * @returns 已按当前语言翻译的导航分组。
 */
export function createNavGroups(t: TFunction): NavGroup[] {
  return [
    {
      title: t('nav.general'),
      items: [
        {
          title: t('nav.users'),
          url: '/users',
          icon: Users,
        },
      ],
    },
  ]
}

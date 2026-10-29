import i18n from 'i18next'
import { toast } from 'sonner'
import { apiErrorMessage } from '@/lib/api-error-message'

/**
 * 把请求失败显示成一条当前语言的 toast。
 *
 * 开发环境下会把原始错误打到控制台，方便排查。
 */
export function handleServerError(error: unknown) {
  if (import.meta.env.DEV) {
    // eslint-disable-next-line no-console
    console.log(error)
  }

  toast.error(apiErrorMessage(error, i18n.t.bind(i18n)))
}

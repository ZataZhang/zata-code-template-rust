import type { TFunction } from 'i18next'
import { ApiRequestError } from '@/api/client'

/**
 * 把请求错误转成当前语言的提示。
 *
 * 后端的 `message` 是固定的中文，不随界面语言变化，所以优先按错误码查
 * `apiErrors.<code>`；没有对应翻译时才退回后端给的原文，再退回通用提示。
 *
 * @param error - 捕获到的错误。
 * @param t - i18next 翻译函数。
 * @returns 可以直接展示给用户的文案。
 */
export function apiErrorMessage(error: unknown, t: TFunction): string {
  if (error instanceof ApiRequestError) {
    if (error.code) {
      const key = `apiErrors.${error.code}`
      const translated = t(key, { defaultValue: '' })
      if (translated) return translated
    }
    if (error.message) return error.message
  }
  return t('apiErrors.unknown')
}

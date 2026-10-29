"use client"

import { useTranslations } from "next-intl"
import { ApiRequestError } from "./client"

/** 后端会返回、且我们有翻译的错误码（对应 `messages/*.json` 的 `apiErrors` 命名空间）。 */
const TRANSLATED_ERROR_CODES = new Set([
  "invalid_email",
  "password_too_short",
  "password_too_long",
  "email_taken",
  "invalid_credentials",
  "unauthenticated",
  "too_many_attempts",
  "auth_unavailable",
  "internal_error",
])

/**
 * Return a function that turns an API error into a message in the current language.
 *
 * 优先按后端的错误码取翻译（后端的中文文案不随界面语言切换）；没有对应翻译时用调用方给的兜底文案。
 * 被限流（429）时会带上需要等待的分钟数。
 *
 * @returns A function mapping an unknown error and a fallback message to the text to show.
 */
export function useApiErrorMessage() {
  const t = useTranslations("apiErrors")

  return (error: unknown, fallback: string): string => {
    if (!(error instanceof ApiRequestError)) return fallback
    if (error.code === "too_many_attempts") {
      const minutes = Math.max(1, Math.ceil((error.retryAfterSeconds ?? 60) / 60))
      return t("too_many_attempts", { minutes })
    }
    if (error.code && TRANSLATED_ERROR_CODES.has(error.code)) {
      return t(error.code)
    }
    if (error.status === undefined) return t("network")
    return fallback
  }
}

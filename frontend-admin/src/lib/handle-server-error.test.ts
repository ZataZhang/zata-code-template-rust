import '@/i18n/init'
import i18n from 'i18next'
import { beforeEach, describe, expect, it, vi } from 'vitest'
import { ApiRequestError } from '@/api/client'
import { handleServerError } from './handle-server-error'

const toastError = vi.hoisted(() => vi.fn())

vi.mock('sonner', () => ({
  toast: {
    error: toastError,
  },
}))

beforeEach(async () => {
  vi.mocked(toastError).mockClear()
  await i18n.changeLanguage('en')
})

describe('handleServerError', () => {
  it('shows the generic message when the error is not recognised', () => {
    handleServerError(new Error('network'))

    expect(toastError).toHaveBeenCalledWith(i18n.t('apiErrors.unknown'))
  })

  it('translates a known backend error code into the current language', async () => {
    const error = new ApiRequestError(
      401,
      '邮箱或密码不正确',
      null,
      'invalid_credentials'
    )

    handleServerError(error)
    await i18n.changeLanguage('zh')
    handleServerError(error)

    const [english, chinese] = toastError.mock.calls.map(([message]) => message)
    expect(english).toBe(i18n.t('apiErrors.invalid_credentials', { lng: 'en' }))
    expect(chinese).toBe(i18n.t('apiErrors.invalid_credentials', { lng: 'zh' }))
    expect(english).not.toBe(chinese)
  })

  it('falls back to the backend message for an unknown error code', () => {
    handleServerError(
      new ApiRequestError(400, 'something specific', null, 'brand_new_code')
    )

    expect(toastError).toHaveBeenCalledWith('something specific')
  })

  it('logs the error to the console in development', () => {
    const log = vi.spyOn(console, 'log').mockImplementation(() => {})
    const err = new Error('logged')

    handleServerError(err)

    expect(log).toHaveBeenCalledWith(err)
    log.mockRestore()
  })

  it('does not log the error to the console in production', () => {
    vi.stubEnv('DEV', false)
    const log = vi.spyOn(console, 'log').mockImplementation(() => {})

    handleServerError(new Error('not logged'))

    expect(log).not.toHaveBeenCalled()
    log.mockRestore()
  })
})

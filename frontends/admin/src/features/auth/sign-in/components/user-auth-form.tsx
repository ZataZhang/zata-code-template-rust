import { useMemo, useState } from 'react'
import { z } from 'zod'
import { useForm } from 'react-hook-form'
import { zodResolver } from '@hookform/resolvers/zod'
import { useNavigate } from '@tanstack/react-router'
import { Loader2, LogIn } from 'lucide-react'
import { useTranslation } from 'react-i18next'
import { toast } from 'sonner'
import { login } from '@/api/auth'
import { useAuthStore } from '@/stores/auth-store'
import { apiErrorMessage } from '@/lib/api-error-message'
import { cn } from '@/lib/utils'
import { Button } from '@/components/ui/button'
import {
  Form,
  FormControl,
  FormField,
  FormItem,
  FormLabel,
  FormMessage,
} from '@/components/ui/form'
import { Input } from '@/components/ui/input'
import { PasswordInput } from '@/components/password-input'

/**
 * 构造登录表单校验 schema。
 *
 * 校验消息依赖当前语言，因此 schema 必须在组件内基于当前 `t` 构造；放在模块顶层
 * 会把首次渲染时的语言固化下来，切换语言后错误提示仍是旧语言。
 *
 * @param t - i18next 翻译函数。
 * @returns 与表单字段对应的 zod schema。
 */
function buildFormSchema(t: (key: string) => string) {
  return z.object({
    identifier: z.string().min(1, t('errors.identifierRequired')),
    password: z.string().min(1, t('errors.passwordRequired')),
  })
}

type UserAuthFormValues = {
  identifier: string
  password: string
}

interface UserAuthFormProps extends React.HTMLAttributes<HTMLFormElement> {
  redirectTo?: string
}

/** Render the UserAuthForm component. */
export function UserAuthForm({
  className,
  redirectTo,
  ...props
}: UserAuthFormProps) {
  const [isLoading, setIsLoading] = useState(false)
  const navigate = useNavigate()
  const { auth } = useAuthStore()
  const { t } = useTranslation()

  const formSchema = useMemo(() => buildFormSchema(t), [t])

  const form = useForm<UserAuthFormValues>({
    resolver: zodResolver(formSchema),
    defaultValues: {
      identifier: '',
      password: '',
    },
  })

  /** Submit the sign-in form. */
  async function onSubmit(data: UserAuthFormValues) {
    setIsLoading(true)
    try {
      const session = await login({
        identifier: data.identifier,
        password: data.password,
      })
      auth.setUser(session)
      if (redirectTo) {
        navigate({ href: redirectTo, replace: true })
      } else {
        navigate({ to: '/', replace: true })
      }
      toast.success(t('auth.welcomeBack', { name: session.display_name }))
    } catch (error) {
      toast.error(apiErrorMessage(error, t))
    } finally {
      setIsLoading(false)
    }
  }

  return (
    <Form {...form}>
      <form
        onSubmit={form.handleSubmit(onSubmit)}
        className={cn('grid gap-5', className)}
        {...props}
      >
        <FormField
          control={form.control}
          name='identifier'
          render={({ field }) => (
            <FormItem>
              <FormLabel>{t('auth.identifierLabel')}</FormLabel>
              <FormControl>
                <Input
                  data-testid='admin-login-identifier-input'
                  autoComplete='username'
                  {...field}
                />
              </FormControl>
              <FormMessage />
            </FormItem>
          )}
        />
        <FormField
          control={form.control}
          name='password'
          render={({ field }) => (
            <FormItem className='relative'>
              <FormLabel>{t('auth.passwordLabel')}</FormLabel>
              <FormControl>
                <PasswordInput
                  data-testid='admin-login-password-input'
                  autoComplete='current-password'
                  {...field}
                />
              </FormControl>
              <FormMessage />
            </FormItem>
          )}
        />
        <Button
          data-testid='admin-login-submit-button'
          size='lg'
          disabled={isLoading}
        >
          {isLoading ? (
            <Loader2 className='mr-2 size-4 animate-spin' />
          ) : (
            <LogIn className='mr-2 size-4' />
          )}
          {t('auth.submit')}
        </Button>
      </form>
    </Form>
  )
}

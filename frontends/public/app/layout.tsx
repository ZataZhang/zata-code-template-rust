import localFont from "next/font/local"
import { NextIntlClientProvider } from "next-intl"
import { getLocale, getMessages, getTranslations } from "next-intl/server"

import "./globals.css"
import "@tpl/accounts-ui/styles.css"
import { ThemeProvider } from "@/components/theme-provider"
import { Toaster } from "@/components/ui/sonner"
import { cn } from "@/lib/utils"

const geist = localFont({
  src: "./fonts/Geist-Variable.woff2",
  weight: "100 900",
  style: "normal",
  display: "swap",
  variable: "--font-sans",
})

const fontMono = localFont({
  src: "./fonts/GeistMono-Variable.woff2",
  weight: "100 900",
  style: "normal",
  display: "swap",
  variable: "--font-mono",
})

/**
 * 按协商出的 locale 生成页面元数据。
 *
 * 文案取自 `html` 命名空间，因此 `<title>` / `<description>` 随请求语言变化，
 * 而不是固定在中文。
 */
export async function generateMetadata() {
  const t = await getTranslations("html")
  return {
    title: t("title"),
    description: t("description"),
  }
}

/**
 * 根布局。
 *
 * locale 由 `i18n/request.ts` 协商（cookie > Accept-Language > 默认），这里把它
 * 写到 `<html lang>`，并用 `NextIntlClientProvider` 把文案注入客户端组件树，
 * 使服务端与客户端渲染共用同一份 locale 与文案。
 */
export default async function RootLayout({
  children,
}: Readonly<{
  children: React.ReactNode
}>) {
  const locale = await getLocale()
  const messages = await getMessages()

  return (
    <html
      lang={locale}
      suppressHydrationWarning
      className={cn(
        "antialiased",
        fontMono.variable,
        "font-sans",
        geist.variable
      )}
    >
      <body>
        <NextIntlClientProvider locale={locale} messages={messages}>
          <ThemeProvider>
            {children}
            <Toaster />
          </ThemeProvider>
        </NextIntlClientProvider>
      </body>
    </html>
  )
}

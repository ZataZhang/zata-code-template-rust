# Geist 字体

此目录的 Geist Sans 与 Geist Mono 可变 WOFF2 文件来自 Vercel 官方 `geist@1.7.2` npm 包：

- `Geist-Variable.woff2`：`package/dist/fonts/geist-sans/Geist-Variable.woff2`
- `GeistMono-Variable.woff2`：`package/dist/fonts/geist-mono/GeistMono-Variable.woff2`
- 上游仓库与版本：[vercel/geist-font v1.7.2](https://github.com/vercel/geist-font/tree/v1.7.2)
- 许可证：SIL Open Font License 1.1，完整文本见本目录 [`LICENSE.txt`](./LICENSE.txt)

字体文件由 `next/font/local` 加载，避免构建依赖访问 Google Fonts。升级时从官方包获取对应文件，并同步核对许可证与本说明中的版本。

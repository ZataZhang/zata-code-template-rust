import type { KnipConfig } from 'knip'

const config: KnipConfig = {
  // 浏览器端到端脚本由 scripts/web-e2e.sh 直接用 node 运行，不被任何模块引用。
  entry: ['e2e/*.mjs'],
  ignore: [
    'src/components/ui/**',
  ],
}

export default config

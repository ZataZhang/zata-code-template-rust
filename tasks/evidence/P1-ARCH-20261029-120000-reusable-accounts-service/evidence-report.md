# 可复用账号模块交付证据

## 人审导航 / Human Review Navigation

| # | 你要看什么 | 呈递物 | 想自己复核？ |
|---|---|---|---|
| 1 | 两个应用安装同一账号包且完成认证 | `just prd review P1-ARCH-20261029-120000-reusable-accounts-service.md`，打开本地审阅页 | 查看公开站点 Workspace 和第二宿主 Signed in 下的邮箱；退出及错误密码会话由自动断言证明 |

![公开站点认证后的当前用户](public-account-session.png)

![第二宿主认证后的当前用户](second-host-session.png)

两图均来自真实浏览器流程。本地图片，GitHub 上不显示。仓库根目录可执行：

```bash
open tasks/evidence/P1-ARCH-20261029-120000-reusable-accounts-service/human-review-checklist.html
```

PR/CI 链接：none，本次为本地工作树交付，没有创建提交、PR 或发布镜像。机器验证由执行器运行并交独立审查核对，人工验收保持未勾选。

## 最终验证结果

| 验证 | 结果与证据 |
|---|---|
| Rust lint / workspace tests / docs | `just ci` passed，最终轮见 `just-ci.log` |
| 真实 PostgreSQL / Redis | `just test real` passed，4 个账号端到端 + 11 个基础设施测试，见 `just-test-real.log` |
| 独立服务 native / Docker HTTP | passed：live/ready 200、注册 201、登录和 me 200、退出 204、退出后 me 401；见 `accounts-native-http.log`、`accounts-container-http.log` |
| 前端包与两个站点检查 | `just accounts-ui-check`、`just web-check`、`just web-admin-check` passed，详见 `frontend-validation.md` |
| 三端真实浏览器 | 统一 `just web-e2e` passed：公开站点 20/20、管理后台 33/33、第二宿主 7 项；宿主历史导航修订后单独再验 9 项全部通过，见 `web-e2e.log`、`accounts-host-popstate-e2e.log` |
| 三个新 Docker 镜像 | accounts、web、admin 均由最终源码构建；公开站点容器页面与本地字体请求 200、零 pageerror，见 `accounts-image-source-manifest.txt`、`stack-image-source-manifest.txt`、`web-docker-page-smoke.log` |
| 完整隔离 Compose | 使用本轮最终镜像 `up --no-build` 后执行 `just stack-smoke`，全部通过；见 `stack-smoke.log` |
| 真实故障负控 | baseline ready 200 → 停止 PostgreSQL 后 503 且无内部原因 → 恢复后 200；见 `readiness-failure-control.log` |
| 迁移完整性 | 两份迁移与原版逐字节一致，见 `migration-integrity.md` |
| 静态与审阅入口 | `just check-lines`、bash 语法、actionlint、`git diff --check` 通过；真实 Chrome 检查审阅页首卡、图片、结果生成和零 pageerror，见 `review-html-check.json` |

完整栈验证使用自建 `accounts-prd-stack`，独立服务验证使用自建 `accounts-prd-standalone`；测试容器及数据卷仅清理自建项目。未对用户既有部署做升级或数据重置。公开站点/管理后台统一浏览器脚本使用仓库测试库和独立 Redis DB6，并清理测试账号；第二宿主额外验证也清理了测试账号。

## 证据边界

普通用户注册与管理员用户管理继续由一个 accounts 业务服务拥有；管理员表、Cookie、Redis namespace 和限流保持独立。不同应用独立部署服务实例可隔离数据；共享实例会共享账号域，当前没有租户或应用级权限模型。

前端包从源码构建后打包，第二宿主通过 tarball 安装。真实浏览器通过同源代理访问 `/auth/*`，没有模拟身份或通过数据库伪造成功。注册后的新 `me` 请求观察到持久化会话；退出后及错误密码后 `me` 返回 401。

## 实施中发现的问题与处理

- 独立宿主退出后仍停留注册模式，导致错误密码流程在本地校验处停住：修正为退出后进入登录，并断言实际登录 401 与新会话查询 401。
- Corepack 在仓库根用 `pnpm --dir` 启动指定版本项目时版本不匹配：入口改为先切换项目目录，遵循其 packageManager。
- 根 Docker ignore 原本排除前端：新增前端 Dockerfile 专属 ignore，构建根上下文并排除环境密钥和本地依赖。
- Google 字体下载不稳定：采用同款真实字体的本地文件并保留来源许可证，最终前端证据须来自修复后的源码。

## 最终树与独立审查

- 基线 HEAD：`97df987992dfae98f241dec7eab1d52623be0f4b`，交付为未提交的工作树修改。
- 源码清单：`final-source-manifest.sha256`，351 个有效源码/配置/文档文件，排除任务与证据目录。
- 清单 SHA256：`2278f2bab59fea60454a550fb4852ca055a031d79581ca7a5f362d5c9aecf040`；在最终验证收尾时重新核对内容未变化。
- 独立审查：`verifier-report.md` 结论 approved、findings=[]，已核对最终源码与真实验证证据；人工验收保持未勾选。
- 镜像输入分别记录在账号/完整栈 source manifest 与 `frontend-validation.md`。最后的宿主导航修订已单独运行真实浏览器；最后的截图输出路径规范化只影响测试产物位置，已单独做路径和 shell 检查。

## 编码自检

- [x] 没有复制既有代码后保留平行微调版本；账号服务迁移原入口，公开认证逻辑集中共享包。
- [x] 复用业务规则，未把数据加载当作规则使用。
- [x] 函数参数不超过 4 个，配置与表单适配采用结构体或对象。
- [x] 新增与修改文件非空行少于 800，未向单文件无限追加。
- [x] Cargo 依赖符合更新后的账号私有层与服务边界。
- [x] 新增变量表达来源与状态，无含糊通用命名。
- [x] 已运行 `just lint`（包含于最终 `just ci`），全绿。
- [x] 已同步文档、公共注释及相关配置和示例。
- [x] 没有新增仅为包装单次调用的小 helper。

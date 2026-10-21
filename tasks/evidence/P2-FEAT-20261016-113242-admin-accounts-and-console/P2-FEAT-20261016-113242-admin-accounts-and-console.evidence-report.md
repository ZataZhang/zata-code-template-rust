## 人审导航 / Human Review Navigation

| 呈递物 | 本机路径与打开命令 | 你应看到什么 | 实际画面 |
|---|---|---|---|
| 交互验收清单 | `/Users/zata/code/rust-study/tasks/evidence/P2-FEAT-20261016-113242-admin-accounts-and-console/human-review-checklist.html`；运行 `open /Users/zata/code/rust-study/tasks/evidence/P2-FEAT-20261016-113242-admin-accounts-and-console/human-review-checklist.html`。静态底稿：[human-review-checklist.md](human-review-checklist.md) | 共 4 项决定，首屏是管理员与普通用户隔离；可逐页选择并生成可复制的 Markdown 回复 | ![交互验收清单首页](human-review-checklist-preview.png) |
| 管理员登录 | `/Users/zata/code/rust-study/tasks/evidence/P2-FEAT-20261016-113242-admin-accounts-and-console/admin-01-sign-in.png`；运行 `open /Users/zata/code/rust-study/tasks/evidence/P2-FEAT-20261016-113242-admin-accounts-and-console/admin-01-sign-in.png` | 独立管理员登录入口，有用户名与密码字段 | ![管理员登录入口](admin-01-sign-in.png) |
| 用户列表 | `/Users/zata/code/rust-study/tasks/evidence/P2-FEAT-20261016-113242-admin-accounts-and-console/admin-02-users.png`；运行 `open /Users/zata/code/rust-study/tasks/evidence/P2-FEAT-20261016-113242-admin-accounts-and-console/admin-02-users.png` | 管理后台 Users 页，能看到搜索与用户状态列表 | ![用户列表](admin-02-users.png) |
| 停用确认 | `/Users/zata/code/rust-study/tasks/evidence/P2-FEAT-20261016-113242-admin-accounts-and-console/admin-03-confirm-disable.png`；运行 `open /Users/zata/code/rust-study/tasks/evidence/P2-FEAT-20261016-113242-admin-accounts-and-console/admin-03-confirm-disable.png` | 停用前出现确认框，并说明该用户不能再登录 | ![停用确认框](admin-03-confirm-disable.png) |
| 刷新后的停用状态 | `/Users/zata/code/rust-study/tasks/evidence/P2-FEAT-20261016-113242-admin-accounts-and-console/admin-03b-after-reload.png`；运行 `open /Users/zata/code/rust-study/tasks/evidence/P2-FEAT-20261016-113242-admin-accounts-and-console/admin-03b-after-reload.png` | 刷新后，目标用户仍显示 Disabled | ![刷新后的停用状态](admin-03b-after-reload.png) |
| 启用完成 | `/Users/zata/code/rust-study/tasks/evidence/P2-FEAT-20261016-113242-admin-accounts-and-console/admin-04-after-enable.png`；运行 `open /Users/zata/code/rust-study/tasks/evidence/P2-FEAT-20261016-113242-admin-accounts-and-console/admin-04-after-enable.png` | Disabled 筛选下列表为空；右下角成功提示表明用户已启用。要看 Active 行需切换筛选条件 | ![启用完成](admin-04-after-enable.png) |
| 登录限流 | `/Users/zata/code/rust-study/tasks/evidence/P2-FEAT-20261016-113242-admin-accounts-and-console/admin-05-throttled.png`；运行 `open /Users/zata/code/rust-study/tasks/evidence/P2-FEAT-20261016-113242-admin-accounts-and-console/admin-05-throttled.png` | 正确密码也被限流，页面显示 `Too many attempts. Please try again later.` | ![登录限流提示](admin-05-throttled.png) |

PR：[GitHub PR 列表](https://github.com/ZataZhang/zata-code-template-rust/pulls)（当前未开 PR）；CI：[GitHub Actions](https://github.com/ZataZhang/zata-code-template-rust/actions)。

**执行方已交叉核对：** 最近一次 `SHOT_DIR=$PWD/target/e2e-shots just web-e2e` 的真实 Chrome 输出为公开站点 `20/20 passed`、管理后台 `33/33 项通过`；本地已逐张查看以上 6 张页面截图。限流截图在页面刷新后重新发起正确密码登录，明确等待前台提示完成动画后采集，避免旧 toast 遮挡。

## 交付与验证记录

- 对应归档 PRD：[`P2-FEAT-20261016-113242-admin-accounts-and-console.md`](../../archive/P2-FEAT-20261016-113242-admin-accounts-and-console.md)。用户已逐项确认并接受，原文见下方；归档 PRD 的 4 个 `Human-Confirmed` 勾选与横幅仍由用户本人更新，执行方未代勾。
- 重跑命令：`SHOT_DIR=$PWD/target/e2e-shots just web-e2e`。
- 截图复制到当前目录后可离线查看；原始截图与 HTML 交互页按仓库约定留在本机，不提交到 Git。
- 交互清单在隔离的 Chrome 会话中完成真实浏览器检查：新会话首屏无预选答案，4 个决定可逐页选择，结果页生成包含四项内容的 Markdown，未发生页面脚本错误。渲染预览为 `human-review-checklist-preview.png`（本地证据，不提交 Git）。
- 本地验证：`SQLX_OFFLINE=true cargo check --workspace --all-targets --locked`、`just sqlx-prepare`、`just prd check --archived`、`just test all`、`just ci-all` 均通过；真实 PostgreSQL/Redis 用例包括管理员控制台、管理员限流及用户会话流程。
- 并行测试修复：E2E 测试数据 ID 加入进程号与原子序号，防止同一时钟刻度生成重复管理员账号；模板复制另增加忽略本地状态文件和拒绝跟随目标符号链接的检查。
- GitHub Actions 首次真实 runner 尚待本地提交推送后回填。

## 人工确认回复

用户原文（审查时间：2026/10/21 18:05:33）：

> 1. 接受管理员与普通用户完全隔离：同意：接受两套完全独立的账号体系
> 2. 接受停用立即生效：同意：接受停用在下一次请求时立即生效
> 3. 接受管理员只能通过启动配置创建：同意：接受仅通过启动时环境变量创建
> 4. 亲自查看 §9.1 的交付界面：已看过并接受 §9.1 呈递区画面

上述回复已记录为验收证据。归档 PRD 中的四个 `Human-Confirmed` 勾选和横幅仍需用户本人更新；执行方未修改它们。

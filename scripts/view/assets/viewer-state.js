/*
 * 页面按 state、tree、content、viewer 顺序加载这些 classic script。
 * 它们共享本页的全局词法作用域，因此无需打包器。
 */
/**
 * 只读文件与改动查看器的前端交互。
 *
 * 数据全部来自本机只读接口（/api/info、/api/tree、/api/file、/api/changes、/api/diff、
 * /api/markdown），页面不做任何写入，也不做心跳轮询——一旦开始轮询，服务端的空闲自动
 * 回收就会静默失效。
 *
 * 改动视图在界面上分两段展示，与 VSCode 对齐：`Staged Changes` 与 `Changes`（后者由服务端
 * 的「未暂存」与「未跟踪」两段合并而来）。**同一个文件可以同时出现在两段里**（暂存了几个 hunk
 * 之后又改了几行），因此条目的身份是「路径 + 底层分区」而不是路径；选中态、请求参数与直达路径
 * 的归属都按这个二元组走，显示上的合并不改变它。折叠有三层粒度：整段（点分段标题）、段内目录、
 * 叶子——三层都用同一份「用户动过手才记」的口径，见 isChangeGroupExpanded 与
 * isDirectoryExpanded。整段的默认值两段不同：`Staged Changes` 默认收起（那是已经看过、
 * 等提交的部分），`Changes` 默认展开（那才是手头正在改的），见 CHANGE_GROUPS 的
 * defaultExpanded。
 *
 * 页面有两个写动作：`Changes` 里的加号（标题旁那一个、以及每个文件旁那一个）会把改动
 * 加进索引（`POST /api/stage`）；内容区头部的「在系统中打开」把当前文件交给系统默认应用
 * （`POST /api/open`，不写仓库）。除此之外页面不写入任何东西——没有提交、没有撤销暂存、
 * 没有丢弃改动，服务端也只有这两条写路由。
 *
 * 两个视图各自记一份「上一次看的那一个」（成功读到内容才记），切视图时还原，而不是每次
 * 都回到空态。改动视图头部的「查看文件」是显式指定，优先于记忆。
 *
 * 左树宽度由树头那个图标按钮切换（双箭头，常态指向右）：点一下把左栏放宽到放得下最长的
 * 那一行（上限视口 60%、下限 340px），箭头随之翻向左边表示「收回去」。它只改布局，不取任何
 * 数据，也不参与任何一次渲染路径。
 *
 * 预览的默认值按文件类型定：Markdown 直接进预览（源码是一次显式切换），图片直接显示（没有
 * 源码可切），其它文本文件与 HTML 默认源码。Markdown 的渲染结果由服务端给出、由「预览」开关
 * 按需取回；图片与 HTML 都指向服务端原样供出的 `/raw/`（HTML 在新标签页里打开，不内联渲染）。
 * 哪个文件支持哪种预览由服务端在 /api/file 的 `preview` / `kind` 字段里给出，本文件不复制
 * 任何后缀表。
 *
 * Markdown 里的 mermaid 围栏由服务端换成 `<pre class="mermaid">`，图本身交给浏览器里的
 * mermaid.js 画（服务端不认识 mermaid，也就无从校验语法）。那份 mermaid 是随查看器一起分发
 * 的第三方产物 `assets/mermaid.min.js`：mermaid 官方压缩包，v11.17.2，MIT，取自
 * `https://cdn.jsdelivr.net/npm/mermaid@11.17.2/dist/mermaid.min.js`
 * （sha256 `581ed7d74bd9048d0e3a91363927d72ef22942d7722546b27f7cc29e35390eb8`，字节与上游
 * 一致）。升级就是换掉这个文件与上面这行版本号，然后在真浏览器里把图看一遍。
 */
  "use strict";

  const FILES_VIEW = "files";
  const DIFF_VIEW = "diff";
  const SOURCE_MODE = "source";
  const PREVIEW_MODE = "preview";

  /** 文件视图那棵目录树的标识；改动视图每段各有一棵，见 treeKeyForGroup。 */
  const FILES_TREE_KEY = FILES_VIEW;

  /**
   * 改动视图在界面上显示的**两段**，与 VSCode 对齐：`Staged Changes` 与 `Changes`。
   *
   * 服务端仍按三段给数据（HEAD↔索引 / 索引↔工作区 / 未跟踪的 git 口径互不相同，单文件 diff
   * 与暂存动作都要知道自己面对哪一种），因此合并只发生在这里——`memberSections` 是这一段由
   * 哪几个底层分区拼出来的。`canStage` 决定这一段的标题旁摆不摆「全部暂存」的加号：已经在索引
   * 里的东西再暂存一次没有意义。`defaultExpanded` 是用户没动过手时该段是否展开：已暂存的那段
   * 默认收起（摊开只会把手头正在改的 `Changes` 挤下去），正在改的那段默认展开。
   */
  const CHANGE_GROUPS = [
    {
      key: "staged",
      label: "Staged Changes",
      memberSections: ["staged"],
      canStage: false,
      defaultExpanded: false,
    },
    {
      key: "changes",
      label: "Changes",
      memberSections: ["unstaged", "untracked"],
      canStage: true,
      defaultExpanded: true,
    },
  ];

  /**
   * 改动视图里某一段那棵目录树的标识。
   * @param {string} groupKey 显示分段的标识。
   * @returns {string} 树标识。
   */
  function treeKeyForGroup(groupKey) {
    return `${DIFF_VIEW}:${groupKey}`;
  }

  const elements = {
    repoName: document.getElementById("repo-name"),
    branchChip: document.getElementById("branch-chip"),
    tabFiles: document.getElementById("tab-files"),
    tabDiff: document.getElementById("tab-diff"),
    diffCount: document.getElementById("diff-count"),
    filterInput: document.getElementById("filter-input"),
    refreshButton: document.getElementById("refresh-view"),
    refreshTimer: document.getElementById("refresh-timer"),
    treeTitle: document.getElementById("tree-title"),
    treeNote: document.getElementById("tree-note"),
    ignoredToggle: document.getElementById("ignored-toggle"),
    treeWidthToggle: document.getElementById("tree-width-toggle"),
    treeBody: document.getElementById("tree-body"),
    bodyGrid: document.querySelector(".body-grid"),
    viewerPath: document.getElementById("viewer-path"),
    copyPathButton: document.getElementById("copy-path"),
    openInSystemButton: document.getElementById("open-in-system"),
    previewSwitch: document.getElementById("preview-switch"),
    tabSource: document.getElementById("tab-source"),
    tabPreview: document.getElementById("tab-preview"),
    openFileButton: document.getElementById("open-file"),
    openExternal: document.getElementById("open-external"),
    viewerMeta: document.getElementById("viewer-meta"),
    viewerBody: document.getElementById("viewer-body"),
    imageZoom: document.getElementById("image-zoom"),
    imageZoomImage: document.getElementById("image-zoom-img"),
    imageZoomCaption: document.getElementById("image-zoom-caption"),
    statusDot: document.getElementById("status-dot"),
    statusText: document.getElementById("status-text"),
    statusHint: document.getElementById("status-hint"),
  };

  /** 页面的全部可变状态；渲染只读它，用户操作只改它。默认落在改动视图。 */
  /** 页面的全部可变状态；渲染只读它，用户操作只改它。默认落在改动视图。 */
  const viewState = {
    view: DIFF_VIEW,
    selectedPath: "",
    selectedSection: "",
    /**
     * 各棵目录树里被手工折叠/展开过的目录：树标识 → (目录路径 → 是否展开)。
     *
     * 只记「用户动过手」的那几个，默认值因此可以是视图相关的（见 isDirectoryExpanded）。
     * 树标识区分「文件视图」与「改动视图的每段」——改动视图里同一个目录路径会出现在多段里，
     * 只按路径记会把一段的折叠连带作用到别段。
     */
    directoryExpansionByTree: new Map(),
    /**
     * 改动视图里被手工折叠/展开过的分段：显示分段的标识 → 是否展开。
     *
     * 与目录折叠同一条口径：只记「用户动过手」的那几个，没记过的按默认值（展开）走。
     * 分段是整段一起收起来的那一层——目录折叠管段内，这一层管段本身。
     */
    changeGroupExpansion: new Map(),
    /** 左树是否加宽（树头那个方向开关）。只影响布局，不参与任何取数。 */
    isTreeWide: false,
    filterText: "",
    filePaths: [],
    ignoredFilePaths: new Set(),
    includeIgnoredFiles: false,
    isLoadingIgnoredFiles: false,
    changedSections: [],
    isDisconnected: false,
    /** 一次手动刷新是否正在进行；用来防止连点发出多份请求。 */
    isRefreshing: false,
    /** 一次暂存请求是否正在进行；同样防连点——连点会发出两遍 `git add`。 */
    isStaging: false,
    /** 上次刷新的时刻（`performance.now()`，单调、不受改系统时间影响）。 */
    lastRefreshedAtMonotonic: 0,
    // --- 两个视图各自记住「上一次看的那一个」，切视图时还原（见 switchView） ---
    /** 文件视图里上一次成功读到正文的路径；空串表示还没看过任何文件。 */
    lastFileViewPath: "",
    /** 改动视图里上一次成功读到改动的「路径 + 分区」；分区一起记，否则同一文件的两段会串。 */
    lastDiffViewSelection: { path: "", section: "" },
    // --- 当前选中文件的预览状态，换文件或切视图时由 resetFilePreview 清空 ---
    /** 当前正文来自 /api/file 的应答；没有可显示正文时为 null。 */
    loadedFile: null,
    /** 服务端给出的预览能力：{mode: 'markdown'} / {mode: 'external', url} / null。 */
    previewDescriptor: null,
    /** 当前显示源码还是预览。 */
    renderMode: SOURCE_MODE,
    /** 已取回的 Markdown 预览片段；null 表示还没取过。 */
    markdownHtml: null,
  };

  /**
   * 请求一个接口。
   *
   * GET 是只读读取；`POST` 只用于两条写路由——暂存（`/api/stage`）与在系统中打开
   * （`/api/open`），且必须带 JSON 请求体：服务端用 `Content-Type: application/json`
   * 把跨站表单式 POST 挡在预检外。
   * @param {string} requestUrl 接口地址。
   * @param {{method?: string, body?: object}} [requestOptions] 方法与 JSON 请求体；缺省是 GET。
   * @returns {Promise<{status: number, body: object}>} 应答状态码与 JSON 正文。
   */
  async function requestJson(requestUrl, requestOptions = {}) {
    const requestBody = requestOptions.body;
    const response = await fetch(requestUrl, {
      method: requestOptions.method || "GET",
      headers: {
        Accept: "application/json",
        ...(requestBody ? { "Content-Type": "application/json" } : {}),
      },
      ...(requestBody ? { body: JSON.stringify(requestBody) } : {}),
    });
    const responseBody = await response.json();
    return { status: response.status, body: responseBody };
  }

  /**
   * 切换到「服务已退出」状态：左树置灰不可点，内容区给出重新连接指引。
   * @param {string} failureMessage 失败说明，用于状态条。
   */
  function enterDisconnectedState(failureMessage) {
    viewState.isDisconnected = true;
    stopRefreshTimer();
    elements.treeBody.classList.add("is-stale");
    elements.statusDot.classList.add("down");
    elements.statusText.textContent = "服务已退出 · 运行 just view 重新连接";
    renderDisconnectedNotice(failureMessage);
  }

  /**
   * 在内容区渲染服务退出提示。
   * @param {string} failureMessage 失败说明。
   */
  function renderDisconnectedNotice(failureMessage) {
    elements.viewerPath.textContent = "服务已退出";
    elements.copyPathButton.hidden = true;
    elements.copyPathButton.dataset.copied = "false";
    elements.openInSystemButton.hidden = true;
    renderPreviewControls(null);
    elements.viewerMeta.textContent = "";
    elements.viewerBody.replaceChildren(
      buildNotice({
        isFailure: true,
        title: "服务已退出",
        paragraphs: [
          "查看器空闲到时限后会自动退出（默认 30 分钟无任何请求），也可以用 just view --stop 立刻回收。",
          "在仓库里重新执行 just view 会新起一个实例并打开新标签页，本页不再自动恢复。",
        ],
        hint: failureMessage ? `本次请求失败原因：${failureMessage}` : "",
      })
    );
  }

  /**
   * 转义要拼进 innerHTML 的文本。
   *
   * 提示块的段落走 innerHTML，而界面上的路径来自 git、文件名本身可以含 `& < >`
   * （例如 `a<b.md`），不转义会让提示块结构错乱。
   * @param {string} rawText 原始文本。
   * @returns {string} 转义后的文本。
   */
  function escapeHtmlText(rawText) {
    return rawText
      .replaceAll("&", "&amp;")
      .replaceAll("<", "&lt;")
      .replaceAll(">", "&gt;");
  }

  /**
   * 构造一个内容区提示块。
   * @param {{isFailure?: boolean, title: string, paragraphs: string[], hint?: string}} noticeInput 提示内容。
   * @returns {HTMLElement} 提示块节点。
   */
  function buildNotice(noticeInput) {
    const noticeNode = document.createElement("div");
    noticeNode.className = noticeInput.isFailure ? "notice down" : "notice";
    const titleNode = document.createElement("h3");
    titleNode.textContent = noticeInput.title;
    noticeNode.append(titleNode);
    for (const paragraphText of noticeInput.paragraphs) {
      const paragraphNode = document.createElement("p");
      paragraphNode.innerHTML = paragraphText;
      noticeNode.append(paragraphNode);
    }
    if (noticeInput.hint) {
      const hintNode = document.createElement("p");
      hintNode.textContent = noticeInput.hint;
      noticeNode.append(hintNode);
    }
    return noticeNode;
  }

  /**
   * 执行一次接口调用并把网络失败统一转成「服务已退出」。
   * @param {() => Promise<unknown>} action 需要保护的调用。
   * @returns {Promise<boolean>} 调用是否成功完成。
   */
  async function guardAgainstServiceExit(action) {
    if (viewState.isDisconnected) {
      return false;
    }
    try {
      await action();
      return true;
    } catch (networkError) {
      enterDisconnectedState(networkError instanceof Error ? networkError.message : "");
      return false;
    }
  }

  /**
   * 读取 URL 上的初始视图与直达路径。
   *
   * 直达路径不带分区：一个路径可能同时出现在多个分区里，归属在改动列表加载完之后
   * 按分区顺序解析（见 `resolveSectionForPath`）。
   */
  function applyInitialQueryParameters() {
    const searchParameters = new URLSearchParams(window.location.search);
    const requestedView = searchParameters.get("view");
    if (requestedView === DIFF_VIEW || requestedView === FILES_VIEW) {
      viewState.view = requestedView;
    }
    viewState.selectedPath = searchParameters.get("path") || "";
  }

  /** 正计时定时器的句柄；页面退出「服务已退出」态后就停掉它。 */
  let refreshTimerHandle = 0;

  /**
   * 启动「距上次刷新」的正计时，并把起点定为现在。
   *
   * 这个定时器**只改本地文字，不发任何请求**：不轮询那条口径说的是请求，页面依旧是「用户不
   * 动就不产生流量」。它存在的意义是把「你看到的这份状态有多旧」摆在眼前——查看器不会自己
   * 跟上文件变更，而人很容易以为它是实时的。
   */
  function startRefreshTimer() {
    viewState.lastRefreshedAtMonotonic = performance.now();
    updateRefreshTimerText();
    if (refreshTimerHandle) {
      return;
    }
    refreshTimerHandle = window.setInterval(updateRefreshTimerText, 1000);
  }

  /**
   * 停掉正计时并把它藏起来。服务已退出时留着它只会误导：那个数字说得像是「刚刚还看过」。
   */
  function stopRefreshTimer() {
    if (refreshTimerHandle) {
      window.clearInterval(refreshTimerHandle);
      refreshTimerHandle = 0;
    }
    elements.refreshTimer.hidden = true;
  }

  /**
   * 把「距上次刷新」画成 `分:秒`（超过一小时才显示小时），并在 title 里写清整句。
   */
  function updateRefreshTimerText() {
    const elapsedSeconds = Math.floor(
      (performance.now() - viewState.lastRefreshedAtMonotonic) / 1000
    );
    const elapsedHours = Math.floor(elapsedSeconds / 3600);
    const elapsedMinutes = Math.floor((elapsedSeconds % 3600) / 60);
    const remainingSeconds = elapsedSeconds % 60;
    const paddedMinutes = String(elapsedMinutes).padStart(2, "0");
    const paddedSeconds = String(remainingSeconds).padStart(2, "0");
    elements.refreshTimer.textContent =
      elapsedHours > 0
        ? `${elapsedHours}:${paddedMinutes}:${paddedSeconds}`
        : `${elapsedMinutes}:${paddedSeconds}`;
    elements.refreshTimer.title =
      `距上次刷新已过去 ${elapsedHours > 0 ? `${elapsedHours} 小时 ` : ""}` +
      `${elapsedMinutes} 分 ${remainingSeconds} 秒（查看器不会自动刷新，点「刷新」重新计时）`;
  }

  /**
   * 手动刷新：重取仓库信息、文件树，并按当前视图重取内容。
   *
   * 页面**刻意不做任何轮询**，所以「看到
   * 最新的文件状态」这件事是一个显式动作。它比页面初始化多做的就一件：重取文件树——树只在
   * 初始化时取过一次，新建或删掉的文件不重取就永远不会出现（这也是这个按钮存在的主要理由）。
   *
   * 选中项尽量保住：文件视图重读当前那个文件（文件已被删就照实报「未找到」），改动视图重新
   * 列改动（那一条已经不在列表里就回到空态）。
   */
  async function refreshWorkspace() {
    if (viewState.isRefreshing) {
      return;
    }
    viewState.isRefreshing = true;
    elements.refreshButton.setAttribute("aria-busy", "true");
    const didRefresh = await guardAgainstServiceExit(async () => {
      const infoResponse = await requestJson("/api/info");
      elements.repoName.textContent = infoResponse.body.repo_name;
      elements.branchChip.textContent = infoResponse.body.branch;
      window.document.title = `${infoResponse.body.repo_name} · 本机查看器`;

      await loadFileTree();

      if (viewState.view === DIFF_VIEW) {
        await loadChangedFiles();
        return;
      }
      // 先展开祖先目录再渲染树，否则选中项落在折叠目录里时树上看不到它（与切视图同一条顺序）。
      expandToSelectedPath();
      renderTree();
      if (viewState.selectedPath) {
        await selectPath(viewState.selectedPath);
        return;
      }
      renderPlaceholder();
    });
    viewState.isRefreshing = false;
    elements.refreshButton.setAttribute("aria-busy", "false");
    if (didRefresh) {
      startRefreshTimer();
      showRefreshFeedback();
    }
  }

  /**
   * 给刷新按钮一个短暂的对勾反馈。
   *
   * 与「复制路径」同一套做法：成功只做图标反馈，不动状态条——状态条那行写的是当前内容的
   * 渲染方式，覆盖掉反而丢信息。
   */
  function showRefreshFeedback() {
    elements.refreshButton.dataset.refreshed = "true";
    window.setTimeout(() => {
      elements.refreshButton.dataset.refreshed = "false";
    }, 1200);
  }

  /**
   * 初始化页面：拉取仓库信息与文件树，然后按初始状态渲染。
   */
  async function initializeViewer() {
    applyInitialQueryParameters();
    renderTreeWidth();
    const didLoad = await guardAgainstServiceExit(async () => {
      const infoResponse = await requestJson("/api/info");
      elements.repoName.textContent = infoResponse.body.repo_name;
      elements.branchChip.textContent = infoResponse.body.branch;
      window.document.title = `${infoResponse.body.repo_name} · 本机查看器`;

      await loadFileTree();
    });
    if (!didLoad) {
      return;
    }

    // 页面打开这一次本身就是一次读取，「距上次刷新」从此刻起算。
    startRefreshTimer();
    expandToSelectedPath();
    if (viewState.view === DIFF_VIEW) {
      await loadChangedFiles();
    } else {
      renderTree();
      if (viewState.selectedPath) {
        await selectPath(viewState.selectedPath);
      } else {
        renderPlaceholder();
      }
    }
    renderViewTabs();
  }

  /**
   * 按当前显示偏好读取文件树。
   */
  async function loadFileTree() {
    const treeUrl = viewState.includeIgnoredFiles
      ? "/api/tree?include_ignored=1"
      : "/api/tree";
    const treeResponse = await requestJson(treeUrl);
    viewState.filePaths = treeResponse.body.paths;
    viewState.ignoredFilePaths = new Set(treeResponse.body.ignored_paths || []);
  }

  /**
   * 判断一个目录当前是否展开。
   *
   * 默认值按视图定：**文件视图默认折叠**——仓库动辄几百上千个文件，全摊开是一堵墙，找目录
   * 得先滚半天；改动视图默认展开——那里只有改动涉及的目录，本来就没几个，分段标题也已经把
   * 噪音分掉了。
   *
   * 用户点过箭头的目录以那次点击为准，但记录按**树**分开（节点的 `treeKey`）：改动视图里
   * 同一个目录路径会在每段各出现一次，只按路径记的话，折起未暂存那段里的 `previews/` 会把
   * 已暂存那段里的也一起折了——两段是两棵树，各折各的。
   *
   * 过滤生效时一律展开：命中的路径必须看得见，否则过滤会得到一个「有命中却全是折叠目录」的
   * 空树——那和「没有命中」在界面上长得一模一样，是明确错误的结论。
   * @param {{treeKey: string, path: string}} directoryNode 目录节点。
   * @param {string} filterText 小写过滤词。
   * @returns {boolean} 是否展开。
   */
  function isDirectoryExpanded(directoryNode, filterText) {
    if (filterText) {
      return true;
    }
    const explicitExpansion = readDirectoryExpansion(directoryNode.treeKey, directoryNode.path);
    if (explicitExpansion !== undefined) {
      return explicitExpansion;
    }
    return viewState.view === DIFF_VIEW;
  }

  /**
   * 读出一棵树里某个目录被手工设成的展开态。
   * @param {string} treeKey 树标识。
   * @param {string} directoryPath 目录的仓库相对路径。
   * @returns {boolean|undefined} 手工设过的展开态；没设过时为 undefined（按默认值走）。
   */
  function readDirectoryExpansion(treeKey, directoryPath) {
    const expansionByPath = viewState.directoryExpansionByTree.get(treeKey);
    return expansionByPath ? expansionByPath.get(directoryPath) : undefined;
  }

  /**
   * 记下一棵树里某个目录的展开态。
   * @param {string} treeKey 树标识。
   * @param {string} directoryPath 目录的仓库相对路径。
   * @param {boolean} isExpanded 是否展开。
   */
  function writeDirectoryExpansion(treeKey, directoryPath, isExpanded) {
    let expansionByPath = viewState.directoryExpansionByTree.get(treeKey);
    if (!expansionByPath) {
      expansionByPath = new Map();
      viewState.directoryExpansionByTree.set(treeKey, expansionByPath);
    }
    expansionByPath.set(directoryPath, isExpanded);
  }

  /**
   * 判断改动视图里某一段当前是否展开。
   *
   * 默认值两段不同（由 `CHANGE_GROUPS` 的 `defaultExpanded` 给出）：`Changes` 默认展开——
   * 那是用户手头正在改的东西；`Staged Changes` 默认收起——它列的是已经看过、等提交的部分，
   * 摊开只会把真正要盯的 `Changes` 挤出视口。段内目录的默认展开仍是「展开」，那是另一层
   * 粒度（见 `isDirectoryExpanded`）。用户点过标题的那一段以那次点击为准，其余按默认值走。
   *
   * 过滤生效时一律展开，与目录同一条口径：命中项被藏在折叠的段里，界面和「没有命中」
   * 长得一模一样，那是明确错误的结论。
   * @param {string} groupKey 显示分段的标识。
   * @param {string} filterText 小写过滤词。
   * @returns {boolean} 是否展开。
   */
  function isChangeGroupExpanded(groupKey, filterText) {
    if (filterText) {
      return true;
    }
    const explicitExpansion = viewState.changeGroupExpansion.get(groupKey);
    if (explicitExpansion !== undefined) {
      return explicitExpansion;
    }
    const changedGroup = CHANGE_GROUPS.find((group) => group.key === groupKey);
    return changedGroup ? changedGroup.defaultExpanded : true;
  }

  /**
   * 记下某一段的展开态。
   * @param {string} groupKey 显示分段的标识。
   * @param {boolean} isExpanded 是否展开。
   */
  function writeChangeGroupExpansion(groupKey, isExpanded) {
    viewState.changeGroupExpansion.set(groupKey, isExpanded);
  }

  /**
   * 展开文件视图里直达路径上的全部祖先目录。
   *
   * 只写文件视图那棵树：直达链接、切视图还原都落在文件视图上；改动视图的每段默认展开，
   * 不需要（也不该）被这里改动。只展开祖先、不含文件自身——文件那条路径不是目录，写进
   * 展开表只会多一个永远不会被查的键。
   */
  function expandToSelectedPath() {
    if (!viewState.selectedPath) {
      return;
    }
    const pathSegments = viewState.selectedPath.split("/");
    for (let segmentIndex = 0; segmentIndex < pathSegments.length - 1; segmentIndex += 1) {
      writeDirectoryExpansion(
        FILES_TREE_KEY,
        pathSegments.slice(0, segmentIndex + 1).join("/"),
        true
      );
    }
  }

  /**
   * 找出某个路径当前所属的分区，用于给不带分区的直达路径定归属。
   *
   * 同一路径可能同时以「已暂存」与「未暂存」两种身份出现，这里取底层分区顺序上的第一个
   * （已暂存优先），与列表的展示顺序一致——点开哪一段都能看到改动，但不能没有确定答案。
   * @param {string} repositoryPath 仓库相对路径。
   * @returns {string} 分区取值；该路径不在任何分区里时为空串。
   */
  function resolveSectionForPath(repositoryPath) {
    const matchedSection = viewState.changedSections.find((section) =>
      section.files.some((changedFile) => changedFile.path === repositoryPath)
    );
    return matchedSection ? matchedSection.section : "";
  }

  /**
   * 变更是否仍出现在改动列表里——路径与分区都要对得上。
   * @param {string} repositoryPath 仓库相对路径。
   * @param {string} sectionName 分区取值。
   * @returns {boolean} 该条目当前是否可见。
   */
  function isSelectableChange(repositoryPath, sectionName) {
    return viewState.changedSections.some(
      (section) =>
        section.section === sectionName &&
        section.files.some((changedFile) => changedFile.path === repositoryPath)
    );
  }

  /**
   * 读取三个分区的改动文件列表。
   */
  async function loadChangedFiles() {
    const didLoad = await guardAgainstServiceExit(async () => {
      const changesResponse = await requestJson("/api/changes");
      if (changesResponse.status !== 200) {
        viewState.changedSections = [];
        renderNotice(buildNotice({ title: "无法列出改动", paragraphs: [changesResponse.body.error] }));
        return;
      }
      viewState.changedSections = changesResponse.body.sections;
      elements.diffCount.textContent = String(countVisibleChangedFiles());
      elements.diffCount.hidden = false;
      // 增删合计只跟在能提供它的那一段后面：`Changes` 里混着未跟踪文件，它们的增删数 git 的
      // 列表命令不报，就不把一个只覆盖一部分文件的 `+N -M` 混进看起来覆盖全部的数字里。
      elements.statusHint.textContent = buildDisplayGroups()
        .map((displayGroup) =>
          displayGroup.statsAvailable
            ? `${displayGroup.label} ${displayGroup.files.length} ` +
              `(+${displayGroup.totals.add} -${displayGroup.totals.del})`
            : `${displayGroup.label} ${displayGroup.files.length}`
        )
        .join(" · ");
    });
    if (!didLoad) {
      return;
    }
    renderTree();
    if (!viewState.selectedPath) {
      renderPlaceholder();
      return;
    }
    // 直达路径（或切换视图前的选中项）不带分区时，按分区顺序补一个。
    const resolvedSection =
      viewState.selectedSection || resolveSectionForPath(viewState.selectedPath);
    if (resolvedSection && isSelectableChange(viewState.selectedPath, resolvedSection)) {
      await selectPath(viewState.selectedPath, resolvedSection);
      return;
    }
    // 还原过来的那条改动已经不在列表里（提交了、撤销了、换过 worktree）：清掉选中态
    // 再报空态。不清的话「内容区是空态、selectedPath 却还指着某个文件」，复制与跳转
    // 按钮的开关就跟内容对不上了。
    viewState.selectedPath = "";
    viewState.selectedSection = "";
    renderPlaceholder();
  }

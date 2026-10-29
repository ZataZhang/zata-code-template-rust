/** 文件树和改动树的 DOM 构造逻辑。 */
"use strict";

  /**
   * 渲染视图切换按钮。
   */
  function renderViewTabs() {
    const isDiffView = viewState.view === DIFF_VIEW;
    elements.tabFiles.setAttribute("aria-selected", String(!isDiffView));
    elements.tabDiff.setAttribute("aria-selected", String(isDiffView));
    const treeTitleNode = document.createElement("b");
    treeTitleNode.textContent = isDiffView ? "改动文件" : "文件树";
    elements.treeTitle.replaceChildren(treeTitleNode);
    elements.ignoredToggle.hidden = isDiffView;
    renderIgnoredFilesToggle();
  }

  /**
   * 同步忽略项开关的可访问名称与状态。
   */
  function renderIgnoredFilesToggle() {
    const isEnabled = viewState.includeIgnoredFiles;
    const actionLabel = isEnabled ? "隐藏" : "显示";
    const accessibleLabel = `${actionLabel}被 Git 忽略的文件`;
    elements.ignoredToggle.textContent = `${actionLabel}忽略`;
    elements.ignoredToggle.setAttribute("aria-pressed", String(isEnabled));
    elements.ignoredToggle.setAttribute("aria-label", accessibleLabel);
    elements.ignoredToggle.title = `${accessibleLabel}；依赖和缓存目录仍隐藏`;
  }

  /**
   * 渲染左树的栏宽开关。
   *
   * 加宽只改布局：不取数据、不重渲染树，所以它不走 renderTree——树的每一次重渲染都只写
   * 列表内容，栏宽是页面级的显示状态。两个箭头都在 DOM 里，由 CSS 按 `aria-pressed` 选一个
   * 显示：常态是「往右撑开」，加宽后换成「收回去」，title / aria-label 跟着一起换。
   */
  function renderTreeWidth() {
    const isWide = viewState.isTreeWide;
    elements.bodyGrid.classList.toggle("is-tree-wide", isWide);
    elements.treeWidthToggle.setAttribute("aria-pressed", String(isWide));
    const toggleLabel = isWide ? "恢复默认栏宽" : "加宽左侧栏，显示完整文件名";
    elements.treeWidthToggle.title = toggleLabel;
    elements.treeWidthToggle.setAttribute("aria-label", toggleLabel);
  }

  /**
   * 写内容区头部的路径，并同步「复制路径」与「在系统中打开」「查看文件」的可见性。
   *
   * 头部没有真实路径（占位符、服务已退出、提示块）时必须把按钮收起来：留一个复制
   * 不到东西的按钮比没有按钮更糟。「查看文件」只在**改动视图**里有意义——文件视图本来
   * 就在看正文，那里摆一个跳回自己的按钮是纯噪音。三个按钮都取
   * `viewState.selectedPath`，因此这里不另存一份路径，只负责显示与按钮的开关。
   * @param {string} repositoryPath 仓库相对路径；空串表示只显示占位符。
   */
  function setViewerPath(repositoryPath) {
    elements.viewerPath.textContent = repositoryPath || "—";
    elements.copyPathButton.hidden = !repositoryPath;
    elements.copyPathButton.dataset.copied = "false";
    elements.openInSystemButton.hidden = !repositoryPath;
    elements.openFileButton.hidden = !repositoryPath || viewState.view !== DIFF_VIEW;
  }

  /**
   * 把内容区当前文件的仓库相对路径复制到剪贴板。
   */
  async function copyCurrentViewerPath() {
    const pathToCopy = viewState.selectedPath;
    if (!pathToCopy) {
      return;
    }
    if (!navigator.clipboard) {
      elements.statusHint.textContent = "当前环境不支持剪贴板写入，请手动选中路径复制";
      return;
    }
    try {
      await navigator.clipboard.writeText(pathToCopy);
    } catch (copyError) {
      elements.statusHint.textContent = `复制路径失败：${
        copyError instanceof Error ? copyError.message : copyError
      }，请手动选中路径复制`;
      return;
    }
    // 成功后只做图标反馈，不动状态条：那行的语法高亮说明是常驻信息，覆盖掉反而丢信息。
    elements.copyPathButton.dataset.copied = "true";
    window.setTimeout(() => {
      elements.copyPathButton.dataset.copied = "false";
    }, 1200);
  }

  /**
   * 把内容区当前文件交给系统默认应用打开（`POST /api/open`；服务端调 macOS `open` /
   * Windows `startfile` / 其它 `xdg-open`）。
   *
   * 与暂存同一条写口径：POST + JSON 体。结果只写状态条，界面布局与正文都不动——查看器里
   * 看不了的二进制与超大文件正是靠它出去的，出去之后这里显示什么不该被打断。
   */
  async function openCurrentPathInSystem() {
    const pathToOpen = viewState.selectedPath;
    if (!pathToOpen || viewState.isDisconnected) {
      return;
    }
    await guardAgainstServiceExit(async () => {
      const openResponse = await requestJson("/api/open", {
        method: "POST",
        body: { path: pathToOpen },
      });
      if (openResponse.status !== 200) {
        elements.statusHint.textContent = `打开失败：${openResponse.body.error}`;
        return;
      }
      elements.statusHint.textContent = `已交给系统打开：${pathToOpen}`;
    });
  }

  /**
   * 在内容区渲染一个提示块。
   * @param {HTMLElement} noticeNode 提示块节点。
   * @param {string} [noticePath] 提示块针对的文件路径；给出的头部保留该路径与
   *   「复制 / 在系统中打开」两个按钮（「文件过大」「二进制文件」这类提示正是最需要
   *   它们的时候）。缺省按占位符处理，收起全部头部控件。
   */
  function renderNotice(noticeNode, noticePath = "") {
    setViewerPath(noticePath);
    // 提示块不是文件正文，预览控件必须跟着收起：留一个点到没有内容的开关比没有更糟。
    renderPreviewControls(null);
    elements.viewerMeta.textContent = "";
    elements.viewerBody.replaceChildren(noticeNode);
  }

  /**
   * 渲染未选中文件时的空态。
   */
  function renderPlaceholder() {
    if (viewState.view === DIFF_VIEW) {
      const hasChangedFiles = countChangedFiles() > 0;
      renderNotice(
        buildNotice({
          title: hasChangedFiles ? "选择一个改动文件" : "工作区没有改动",
          paragraphs: [
            hasChangedFiles
              ? "左侧按「Staged Changes / Changes」分段列出有改动的文件，点开任意一个查看逐行改动。"
              : "索引与工作区都与 HEAD 一致，也没有未跟踪文件。",
          ],
          hint: hasChangedFiles
            ? "提示：同一个文件可以同时出现在两段里——暂存之后又改的那几行属于未暂存。"
            : "",
        })
      );
      return;
    }
    renderNotice(
      buildNotice({
        title: "选择一个文件",
        paragraphs: ["左侧是本仓库的文件树，点开任意文件即可在右侧查看带行号与语法高亮的正文。"],
        hint: "快捷键：f 切文件视图、d 切改动视图、/ 聚焦过滤框。",
      })
    );
  }

  /**
   * 统计三个分区里的改动文件总数。
   * @returns {number} 改动文件数。
   */
  function countChangedFiles() {
    return viewState.changedSections.reduce((fileCount, section) => {
      return fileCount + section.files.length;
    }, 0);
  }

  /**
   * 渲染左侧树；两个视图共用同一套目录树，只有叶子行与空态文案不同。
   *
   * 改动视图先按分区分段、段内再按目录分组，而不是铺成一长条扁平行：改动文件动辄成百
   * 上千，扁平列表既看不出改动落在哪些目录，也分不清哪几行已经进过索引。
   */
  function renderTree() {
    const isDiffView = viewState.view === DIFF_VIEW;
    const filterText = viewState.filterText.trim().toLowerCase();
    const treeFragment = document.createDocumentFragment();

    if (isDiffView) {
      appendChangedSections(treeFragment, filterText);
      elements.treeNote.textContent = `${countChangedFiles()} 个文件`;
    } else {
      const directoryTree = buildDirectoryTree(
        viewState.filePaths.map((filePath) => ({
          path: filePath,
          ignored: viewState.ignoredFilePaths.has(filePath),
        })),
        FILES_TREE_KEY
      );
      const ordinaryFileCount = viewState.filePaths.length - viewState.ignoredFilePaths.size;
      elements.treeNote.textContent = viewState.ignoredFilePaths.size
        ? `${ordinaryFileCount} 个文件 · ${viewState.ignoredFilePaths.size} 个忽略项`
        : `${ordinaryFileCount} 个文件`;
      appendDirectoryChildren(treeFragment, directoryTree, 0, filterText);
    }

    if (!treeFragment.childNodes.length) {
      elements.treeBody.replaceChildren(buildTreeEmpty(emptyTreeMessage(isDiffView)));
      return;
    }
    elements.treeBody.replaceChildren(treeFragment);
  }

  /**
   * 去掉已被重命名消费掉的删除行。
   *
   * 未暂存区里的「删除」与未跟踪区里的「新文件」可以是同一处工作区重命名（普通 `mv`）：服务端
   * 把目的地认成 `R` 并给出旧路径之后，那条删除行再单列一次就会让人以为文件被删了。只在同一
   * 显示段内折叠——跨段折叠会把别处的同名删除也顺带藏起来。
   * @param {Array<object>} changedFiles 合并后的改动条目。
   * @returns {Array<object>} 去掉重命名来源后的条目。
   */
  function dropRowsConsumedByRenames(changedFiles) {
    const renameSourcePaths = new Set(
      changedFiles.map((changedFile) => changedFile.rename_from).filter(Boolean)
    );
    return changedFiles.filter(
      (changedFile) => !(changedFile.status === "D" && renameSourcePaths.has(changedFile.path))
    );
  }

  /**
   * 数出改动列表里**能看到的**条目数。
   *
   * 与服务端的分区合计不同：一处工作区重命名在服务端是「未暂存的删除 + 未跟踪的新文件」两条，
   * 显示上折叠成一条 `R`，照搬服务端那个数就会和树对不上。已暂存的重命名本来就是一条 `R`，
   * 这个口径与它一致。
   * @returns {number} 可见改动条目数。
   */
  function countVisibleChangedFiles() {
    return buildDisplayGroups().reduce(
      (fileCount, displayGroup) => fileCount + displayGroup.files.length,
      0
    );
  }

  /**
   * 把服务端给的三段整理成界面上显示的两段。
   *
   * 每个文件条目都带上它**底层**的分区取值（`section`）：单文件 diff 与暂存动作都要靠它，
   * 而显示上的分段只是把后两段并到了一起。合并之后段内仍是一棵树，因此未暂存的与未跟踪的
   * 文件按同一套目录顺序排在一起，靠状态徽标区分（`M` 与 `A`）。
   * @returns {Array<{key: string, label: string, files: Array<object>, canStage: boolean, statsAvailable: boolean, totals: {add: number, del: number}}>} 两段。
   */
  function buildDisplayGroups() {
    return CHANGE_GROUPS.map((changedGroup) => {
      const memberSections = viewState.changedSections.filter((changedSection) =>
        changedGroup.memberSections.includes(changedSection.section)
      );
      return {
        key: changedGroup.key,
        label: changedGroup.label,
        canStage: changedGroup.canStage,
        files: dropRowsConsumedByRenames(
          memberSections.flatMap((changedSection) =>
            changedSection.files.map((changedFile) => ({
              ...changedFile,
              section: changedSection.section,
              statsAvailable: changedSection.stats_available,
            }))
          )
        ),
        // 「这一段能不能给出完整的增删合计」= 成员段都提供统计。`Changes` 里混着未跟踪文件，
        // git 的列表命令不为它们报增删（逐文件去取在 800 个文件时会让页面卡住），只把其中
        // 一部分加起来当成整段合计就是编数字。
        statsAvailable:
          memberSections.length > 0 &&
          memberSections.every((changedSection) => changedSection.stats_available),
        totals: {
          add: memberSections.reduce((addSum, section) => addSum + section.totals.add, 0),
          del: memberSections.reduce((delSum, section) => delSum + section.totals.del, 0),
        },
      };
    });
  }

  /**
   * 按显示分段追加改动列表：每段一条标题行，后面跟该段自己的目录树。
   *
   * 段内那棵树先建进临时片段再判断有没有内容——空段（或被过滤掉全部内容的段）不该
   * 留下一条孤零零的标题行。折叠的段同样要把这棵树建出来：判空看的是「这一段有没有
   * 可见内容」，不是「眼下展开了几行」，否则折起来再过滤就会得到一条骗人的空段标题。
   * @param {DocumentFragment} treeFragment 目标片段。
   * @param {string} filterText 小写过滤词。
   */
  function appendChangedSections(treeFragment, filterText) {
    for (const displayGroup of buildDisplayGroups()) {
      const sectionFragment = document.createDocumentFragment();
      // 底层分区取值与「本段是否提供 +N -M」随条目一起进树：叶子行需要它们才能把「路径 +
      // 分区」这个身份还原出来、并按段决定统计列的显示，而递归渲染函数自己不知道当前
      // 在哪一段。
      appendDirectoryChildren(
        sectionFragment,
        buildDirectoryTree(displayGroup.files, treeKeyForGroup(displayGroup.key)),
        0,
        filterText
      );
      if (!sectionFragment.childNodes.length) {
        continue;
      }
      treeFragment.append(buildChangedSectionHeader(displayGroup, filterText));
      if (isChangeGroupExpanded(displayGroup.key, filterText)) {
        treeFragment.append(sectionFragment);
      }
    }
  }

  /**
   * 构造一条分段标题行：折叠箭头 + 标签 + 文件数 +（`Changes` 段才有的）全部暂存加号。
   *
   * 标题行整行可点，点一下收起或展开整段（折叠粒度：整段 → 段内目录 → 叶子）。可点的那部分
   * 必须是一个真按钮，且**不能**把暂存加号套在里面：按钮套按钮不是合法 HTML，键盘用户也分不出
   * 两者——与 `buildChangedFileRow` 里那条约束同源，所以加号是它的兄弟节点。
   * @param {{key: string, label: string, files: Array<object>, canStage: boolean}} displayGroup 显示分段。
   * @param {string} filterText 小写过滤词。
   * @returns {HTMLElement} 标题行节点。
   */
  function buildChangedSectionHeader(displayGroup, filterText) {
    const isExpanded = isChangeGroupExpanded(displayGroup.key, filterText);

    const headerNode = document.createElement("div");
    headerNode.className = "section-head";
    headerNode.dataset.section = displayGroup.key;

    const toggleNode = document.createElement("button");
    toggleNode.type = "button";
    toggleNode.className = "section-toggle";
    toggleNode.setAttribute("aria-expanded", String(isExpanded));

    const caretNode = document.createElement("span");
    caretNode.className = "caret";
    caretNode.textContent = isExpanded ? "▾" : "▸";
    toggleNode.append(caretNode);

    const labelNode = document.createElement("span");
    labelNode.className = "section-label";
    labelNode.textContent = displayGroup.label;
    toggleNode.append(labelNode);

    const countNode = document.createElement("span");
    countNode.className = "chip";
    countNode.textContent = String(displayGroup.files.length);
    toggleNode.append(countNode);

    toggleNode.addEventListener("click", () => {
      // 取反按「不过滤时的状态」算，与目录折叠同一条理由：过滤生效时一律被强制展开、箭头恒为
      // ▾，这时点击本就没有可见效果，该记下的是「清掉过滤词之后希望它是什么样」。
      writeChangeGroupExpansion(displayGroup.key, !isChangeGroupExpanded(displayGroup.key, ""));
      renderTree();
    });
    headerNode.append(toggleNode);

    if (displayGroup.canStage) {
      const stageAllNode = buildStageButton({
        title: "把这一段的全部改动加入索引（git add -A）",
        label: "+",
        isSectionLevel: true,
      });
      stageAllNode.addEventListener("click", () => {
        void stageRequest({ scope: "all" });
      });
      headerNode.append(stageAllNode);
    }
    return headerNode;
  }

  /**
   * 构造一个「加入索引」按钮。
   * @param {{title: string, label: string, isSectionLevel: boolean}} stageButtonInput 按钮内容。
   * @returns {HTMLElement} 按钮节点。
   */
  function buildStageButton(stageButtonInput) {
    const stageButtonNode = document.createElement("button");
    stageButtonNode.type = "button";
    stageButtonNode.className = stageButtonInput.isSectionLevel
      ? "tree-icon-button stage-button section-level"
      : "tree-icon-button stage-button";
    stageButtonNode.title = stageButtonInput.title;
    stageButtonNode.setAttribute("aria-label", stageButtonInput.title);
    stageButtonNode.textContent = stageButtonInput.label;
    return stageButtonNode;
  }

  /**
   * 左树的空态文案。
   * @param {boolean} isDiffView 是否处于改动视图。
   * @returns {string} 空态说明。
   */
  function emptyTreeMessage(isDiffView) {
    if (!isDiffView) {
      return "没有匹配过滤条件的文件。";
    }
    return countChangedFiles()
      ? "没有匹配过滤条件的改动文件。"
      : "工作区没有改动文件。";
  }

  /**
   * 给树行画缩进参考线：第 i 层祖先在第 i 层缩进的中缝留一条 1px 的淡竖线，贯穿整行。
   *
   * 行是拍平的按钮，层级只靠 `paddingLeft` 的 14px 步长表达时，深一层的目录与浅一层的文件
   * 很难一眼对上归属；参考线把「这些行同属一个父目录」钉在背景上。画在行自己的背景层
   * （inline style）而不是给行套嵌套容器：渲染保持扁平结构，hover 与选中改的底色走
   * background-color，不会抹掉这层背景图。
   * @param {HTMLElement} rowButton 树行按钮。
   * @param {number} depth 缩进层级。
   */
  function applyIndentGuides(rowButton, depth) {
    if (depth <= 0) {
      return;
    }
    rowButton.style.backgroundImage = Array.from(
      { length: depth },
      () => "linear-gradient(var(--indent-guide), var(--indent-guide))"
    ).join(", ");
    rowButton.style.backgroundRepeat = "no-repeat";
    rowButton.style.backgroundSize = "1px 100%";
    rowButton.style.backgroundPosition = Array.from(
      { length: depth },
      (_, levelIndex) => `${8 + levelIndex * 14 + 5}px 0`
    ).join(", ");
  }

  /**
   * 构造一个改动文件叶子行：状态徽标 + 文件名 + 增删统计（+ 还在「Changes」里时的加号）。
   * 目录上下文由所在层级表达，行内不再重复完整路径。
   *
   * 统计列有三种状态：给出 `+N -M`、给一个 `01` 徽标（git 判定为二进制，不逐行给数）、以及
   * 不提供（未跟踪文件，git 的列表命令不报这个数）。后两者不能混：`01` 是「有改动但数不出
   * 行」，不提供是「本轮没取」。
   *
   * 行本身是一个 `<button>`，因此那个加号**不能**嵌在它里面（按钮套按钮不是合法 HTML，键盘
   * 用户也分不出两者）。它作为兄弟节点放在外层容器里，各点各的。
   * @param {{path: string, status: string, rename_from?: string|null, add: number|null, del: number|null, section: string, statsAvailable: boolean}} changedFile 改动文件（含底层分区与统计可用性）。
   * @param {number} depth 缩进层级。
   * @returns {HTMLElement} 条目节点。
   */
  function buildChangedFileRow(changedFile, depth) {
    const rowButton = document.createElement("button");
    rowButton.type = "button";
    rowButton.className = "node";
    rowButton.style.paddingLeft = `${8 + depth * 14}px`;
    applyIndentGuides(rowButton, depth);
    rowButton.title = changedFile.rename_from
      ? `${changedFile.path}\n重命名自 ${changedFile.rename_from}`
      : changedFile.path;
    rowButton.setAttribute(
      "aria-selected",
      String(
        changedFile.path === viewState.selectedPath &&
          changedFile.section === viewState.selectedSection
      )
    );
    rowButton.append(buildCaretPlaceholder());

    const badgeNode = document.createElement("span");
    badgeNode.className = `badge ${changedFile.status}`;
    badgeNode.textContent = changedFile.status;
    rowButton.append(badgeNode);

    const nameNode = document.createElement("span");
    nameNode.className = "name";
    // `Changes` 段（未暂存 + 未跟踪）的文件名给一层淡绿：同一个文件同时出现在两段里时，
    // 一眼分得清哪条还没进索引；`Staged Changes` 段保持默认色不动。
    if (changedFile.section !== "staged") {
      nameNode.classList.add("name-unstaged");
    }
    const lastSlashIndex = changedFile.path.lastIndexOf("/");
    nameNode.textContent =
      lastSlashIndex >= 0 ? changedFile.path.slice(lastSlashIndex + 1) : changedFile.path;
    rowButton.append(nameNode);

    if (changedFile.rename_from) {
      const renameSourceNode = document.createElement("span");
      renameSourceNode.className = "rename-source";
      renameSourceNode.textContent = `← ${changedFile.rename_from}`;
      renameSourceNode.setAttribute("aria-label", `重命名自 ${changedFile.rename_from}`);
      rowButton.append(renameSourceNode);
    }

    // 统计列的显示口径：这一条自己有可信的增删数就显示。段级的 `statsAvailable` 不够用——未跟踪
    // 那一段普通文件没有统计（`add`/`del` 为 null 表示「本轮不提供」），但其中被认作重命名目的地
    // 的那一条，两个数是服务端在一次性索引上单独算出来的。`rename_from` 非空即为后者：它的数不是
    // 「不提供」，`null` 在这里只可能是二进制。
    if (changedFile.statsAvailable || changedFile.rename_from) {
      const statNode = document.createElement("span");
      statNode.className = "stat";
      if (changedFile.add === null || changedFile.del === null) {
        const binaryNode = document.createElement("span");
        binaryNode.className = "badge";
        // 用 `01` 而不是中文「二进制」：这一列只有十几像素宽，三个汉字会折成三行把行高顶开；
        // 而 `01` 与同排的 A / D / R 一样是语言无关的短记号，含义靠 title 兜住。
        binaryNode.textContent = "01";
        binaryNode.title = "二进制文件：内容有变化，但 git 不提供逐行改动";
        statNode.append(binaryNode);
      } else {
        statNode.append(buildStatNode("add", `+${changedFile.add}`));
        statNode.append(buildStatNode("del", `-${changedFile.del}`));
      }
      rowButton.append(statNode);
    }

    rowButton.addEventListener("click", () => {
      void selectPath(changedFile.path, changedFile.section);
    });

    const rowNode = document.createElement("div");
    rowNode.className = "node-row";
    rowNode.append(rowButton);
    // 只在「Changes」里的条目上给加号：已经在索引里的东西再暂存一次没有意义（这也是为什么
    // 「Staged Changes」那一段没有反向的减号——取消暂存是刻意不做的，不是漏了）。
    if (changedFile.section !== "staged") {
      const stageButtonNode = buildStageButton({
        title: `把 ${changedFile.path} 的当前内容加入索引（git add）`,
        label: "+",
        isSectionLevel: false,
      });
      stageButtonNode.addEventListener("click", () => {
        void stageRequest({ scope: "path", path: changedFile.path });
      });
      rowNode.append(stageButtonNode);
    }
    return rowNode;
  }

  /**
   * 发起一次暂存请求，并把界面拉到暂存之后的状态。
   *
   * 这是页面唯一的写操作（`POST /api/stage`，服务端只做 `git add`）。暂存之后那个文件就不在
   * 「Changes」里了：把选中项的分区清空，交给重新取回的列表按分区顺序解析——它自然落到
   * 「Staged Changes」那一侧，而不是留下一句「该文件在此区段下没有改动」，看着像把手上的东西
   * 弄丢了。
   * @param {{scope: string, path?: string}} stageBody 暂存范围：`{scope:"path", path}` 或 `{scope:"all"}`。
   */
  async function stageRequest(stageBody) {
    if (viewState.isStaging) {
      return;
    }
    viewState.isStaging = true;
    const didStage = await guardAgainstServiceExit(async () => {
      const stageResponse = await requestJson("/api/stage", { method: "POST", body: stageBody });
      if (stageResponse.status !== 200) {
        // 失败照实说，且不改动列表——观众看到的那份状态和仓库仍然一致。
        elements.statusHint.textContent = `暂存失败：${stageResponse.body.error}`;
        return false;
      }
      return true;
    });
    viewState.isStaging = false;
    if (!didStage) {
      return;
    }
    viewState.selectedSection = "";
    await loadChangedFiles();
  }

  /**
   * 构造一个增删统计节点。
   * @param {"add"|"del"} statKind 统计类型。
   * @param {string} statText 统计文本。
   * @returns {HTMLElement} 统计节点。
   */
  function buildStatNode(statKind, statText) {
    const statNode = document.createElement("span");
    statNode.className = statKind;
    statNode.textContent = statText;
    return statNode;
  }

  /**
   * 构造占位用的折叠箭头，让改动视图的条目与文件树条目左缘对齐。
   * @returns {HTMLElement} 占位节点。
   */
  function buildCaretPlaceholder() {
    const caretNode = document.createElement("span");
    caretNode.className = "caret";
    return caretNode;
  }

  /**
   * 构造文件树的空态节点。
   * @param {string} emptyMessage 空态说明。
   * @returns {HTMLElement} 空态节点。
   */
  function buildTreeEmpty(emptyMessage) {
    const emptyNode = document.createElement("div");
    emptyNode.className = "tree-empty";
    emptyNode.textContent = emptyMessage;
    return emptyNode;
  }

  /**
   * 把扁平的条目列表组装成目录树。
   *
   * 文件视图传 `{path}`，改动视图传改动文件对象；叶子节点把原条目留在 `payload` 里，
   * 于是两种叶子行都能从同一棵树渲染。`treeKey` 标出这棵树是谁的，用来把折叠状态按树分开：
   * 改动视图里同一个目录路径会在每段各出现一次，两段是两棵树。
   * @param {Array<{path: string, ignored?: boolean}>} treeEntries 每条含路径，可标记忽略文件。
   * @param {string} treeKey 这棵树的标识（文件视图或改动视图的某一段）。
   * @returns {{name: string, path: string, treeKey: string, isDirectory: boolean, children: Map, payload: object|null}} 根节点。
   */
  function buildDirectoryTree(treeEntries, treeKey) {
    const rootNode = {
      name: "",
      path: "",
      treeKey,
      isDirectory: true,
      children: new Map(),
      payload: null,
    };
    for (const treeEntry of treeEntries) {
      const pathSegments = treeEntry.path.split("/");
      let currentNode = rootNode;
      for (let segmentIndex = 0; segmentIndex < pathSegments.length; segmentIndex += 1) {
        const pathSegment = pathSegments[segmentIndex];
        if (!currentNode.children.has(pathSegment)) {
          currentNode.children.set(pathSegment, {
            name: pathSegment,
            path: pathSegments.slice(0, segmentIndex + 1).join("/"),
            treeKey,
            isDirectory: segmentIndex < pathSegments.length - 1,
            children: new Map(),
            payload: null,
          });
        }
        currentNode = currentNode.children.get(pathSegment);
      }
      currentNode.payload = treeEntry;
    }
    return rootNode;
  }

  /**
   * 递归追加目录下的可见节点。
   * @param {DocumentFragment} treeFragment 目标片段。
   * @param {{name: string, path: string, isDirectory: boolean, children: Map}} directoryNode 当前目录。
   * @param {number} depth 缩进层级。
   * @param {string} filterText 小写过滤词；非空时只保留命中路径及其祖先。
   */
  function appendDirectoryChildren(treeFragment, directoryNode, depth, filterText) {
    const sortedChildren = [...directoryNode.children.values()].sort(compareTreeNode);
    for (const childNode of sortedChildren) {
      if (!matchesFilter(childNode, filterText)) {
        continue;
      }
      if (childNode.isDirectory) {
        treeFragment.append(buildDirectoryRow(childNode, depth, filterText));
        if (isDirectoryExpanded(childNode, filterText)) {
          appendDirectoryChildren(treeFragment, childNode, depth + 1, filterText);
        }
      } else if (viewState.view === DIFF_VIEW) {
        treeFragment.append(buildChangedFileRow(childNode.payload, depth));
      } else {
        treeFragment.append(buildFileRow(childNode, depth));
      }
    }
  }

  /**
   * 判断节点是否命中过滤条件；目录在本目录自身或任一后代命中时保留。
   * @param {{name: string, path: string, isDirectory: boolean, children: Map, payload: object|null}} treeNode 待判定节点。
   * @param {string} filterText 小写过滤词。
   * @returns {boolean} 是否应当显示。
   */
  function matchesFilter(treeNode, filterText) {
    if (!filterText) {
      return true;
    }
    if (treeNode.path.toLowerCase().includes(filterText)) {
      return true;
    }
    if (!treeNode.isDirectory) {
      // 重命名那一行还要能按**旧路径**搜到：消费掉的那条删除行已经不在树里，只按新路径匹配的话，
      // 用户照旧路径搜索会得到一片空白。
      const renameSource = treeNode.payload ? treeNode.payload.rename_from : null;
      return Boolean(renameSource && renameSource.toLowerCase().includes(filterText));
    }
    return [...treeNode.children.values()].some((childNode) =>
      matchesFilter(childNode, filterText)
    );
  }

  /**
   * 排序：目录在前，同类按名称排序。
   * @param {{name: string, isDirectory: boolean}} leftNode 左节点。
   * @param {{name: string, isDirectory: boolean}} rightNode 右节点。
   * @returns {number} 排序比较值。
   */
  function compareTreeNode(leftNode, rightNode) {
    if (leftNode.isDirectory !== rightNode.isDirectory) {
      return leftNode.isDirectory ? -1 : 1;
    }
    return leftNode.name.localeCompare(rightNode.name);
  }

  /**
   * 构造目录行。
   * @param {{name: string, path: string, children: Map}} directoryNode 目录节点。
   * @param {number} depth 缩进层级。
   * @param {string} filterText 小写过滤词；命中目录强制展开，便于看到命中项。
   * @returns {HTMLElement} 目录行。
   */
  function buildDirectoryRow(directoryNode, depth, filterText) {
    const isCollapsed = !isDirectoryExpanded(directoryNode, filterText);
    const rowButton = document.createElement("button");
    rowButton.type = "button";
    // is-dir 给目录名加一档字重：段标题 → 目录 → 文件三层的中间层看得出来（见 viewer.css）。
    rowButton.className = "node is-dir";
    rowButton.style.paddingLeft = `${8 + depth * 14}px`;
    applyIndentGuides(rowButton, depth);
    rowButton.setAttribute("aria-expanded", String(!isCollapsed));

    const caretNode = document.createElement("span");
    caretNode.className = "caret";
    caretNode.textContent = isCollapsed ? "▸" : "▾";
    rowButton.append(caretNode);

    const nameNode = document.createElement("span");
    nameNode.className = "name";
    nameNode.textContent = `${directoryNode.name}/`;
    rowButton.append(nameNode);

    if (!filterText) {
      const countNode = document.createElement("span");
      countNode.className = "chip";
      countNode.textContent = String(countFileLeaves(directoryNode));
      rowButton.append(countNode);
    }

    rowButton.addEventListener("click", () => {
      // 取反按「不过滤时的状态」算，不看当场那个值：过滤生效时目录一律被强制展开、箭头恒为
      // ▾，这时点击本就没有可见效果，该记下的是「清掉过滤词之后希望它是什么样」。按当场值
      // 取反会把这个被强制出来的 true 当成用户的意愿，反过来记成折叠。
      const isExpandedWithoutFilter = isDirectoryExpanded(directoryNode, "");
      writeDirectoryExpansion(directoryNode.treeKey, directoryNode.path, !isExpandedWithoutFilter);
      renderTree();
    });
    return rowButton;
  }

  /**
   * 统计目录下的文件数。
   * @param {{isDirectory: boolean, children: Map}} treeNode 目录节点。
   * @returns {number} 文件数。
   */
  function countFileLeaves(treeNode) {
    if (!treeNode.isDirectory) {
      return 1;
    }
    let fileCount = 0;
    for (const childNode of treeNode.children.values()) {
      fileCount += countFileLeaves(childNode);
    }
    return fileCount;
  }

  /**
   * 构造文件行。
   * @param {{name: string, path: string, payload: {ignored?: boolean}|null}} fileNode 文件节点。
   * @param {number} depth 缩进层级。
   * @returns {HTMLElement} 文件行。
   */
  function buildFileRow(fileNode, depth) {
    const rowButton = document.createElement("button");
    rowButton.type = "button";
    rowButton.className = "node";
    rowButton.style.paddingLeft = `${8 + depth * 14}px`;
    applyIndentGuides(rowButton, depth);
    rowButton.setAttribute("aria-selected", String(fileNode.path === viewState.selectedPath));
    rowButton.append(buildCaretPlaceholder());

    const nameNode = document.createElement("span");
    nameNode.className = "name";
    nameNode.textContent = fileNode.name;
    rowButton.append(nameNode);

    if (fileNode.payload && fileNode.payload.ignored) {
      const ignoredBadge = document.createElement("span");
      ignoredBadge.className = "ignored-badge";
      ignoredBadge.textContent = "忽略";
      ignoredBadge.title = "被 Git 忽略；只在文件树中只读查看，不进入改动或暂存";
      rowButton.append(ignoredBadge);
    }

    rowButton.addEventListener("click", () => {
      void selectPath(fileNode.path);
    });
    return rowButton;
  }

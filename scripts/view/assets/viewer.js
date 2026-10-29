/** 视图切换、用户事件绑定与页面初始化。 */
"use strict";

  /**
   * 切换视图。
   *
   * 两个视图各记一份「上一次看的那一个」，切过去时还原，而不是每次都回到空态：在改动里
   * 看到一半切去翻文件、再切回来，本该还在原来那条改动上。`pathToSelect` 是显式指定，
   * 优先级高于记忆——改动视图头部的「查看文件」就是靠它跳到当前这条改动对应的文件（那
   * 是「看你点的这一条」，不是「回到上次看的那个文件」）。还原是尽力而为：文件可能已经
   * 被删、改动可能已经提交，两种情况都由下游给出明确结果（未找到文件 / 空态）。
   * @param {string} nextView 目标视图。
   * @param {string} pathToSelect 显式要选中的仓库相对路径；空串表示用目标视图的记忆。
   */
  async function switchView(nextView, pathToSelect = "") {
    if (viewState.isDisconnected || viewState.view === nextView) {
      return;
    }
    viewState.view = nextView;
    resetFilePreview();
    renderViewTabs();
    if (nextView === DIFF_VIEW) {
      // 分区也要一起还原：同一个文件可以同时出现在已暂存与未暂存两段，只带路径回去会
      // 落到按分区顺序解析出来的另一段。
      viewState.selectedPath = pathToSelect || viewState.lastDiffViewSelection.path;
      viewState.selectedSection = pathToSelect ? "" : viewState.lastDiffViewSelection.section;
      await loadChangedFiles();
      return;
    }
    viewState.selectedPath = pathToSelect || viewState.lastFileViewPath;
    viewState.selectedSection = "";
    elements.statusHint.textContent = "";
    // 先展开祖先目录再渲染树，否则文件落在折叠的目录里时树上看不到选中态。
    expandToSelectedPath();
    renderTree();
    if (viewState.selectedPath) {
      await selectPath(viewState.selectedPath);
    } else {
      renderPlaceholder();
    }
  }

  /**
   * 切换是否在文件树中包含被忽略的文件。
   */
  async function toggleIgnoredFiles() {
    if (
      viewState.isDisconnected ||
      viewState.view !== FILES_VIEW ||
      viewState.isLoadingIgnoredFiles
    ) {
      return;
    }
    viewState.isLoadingIgnoredFiles = true;
    viewState.includeIgnoredFiles = !viewState.includeIgnoredFiles;
    elements.ignoredToggle.disabled = true;
    renderIgnoredFilesToggle();
    await guardAgainstServiceExit(async () => {
      await loadFileTree();
      if (
        !viewState.includeIgnoredFiles &&
        viewState.view === FILES_VIEW &&
        viewState.selectedPath &&
        !viewState.filePaths.includes(viewState.selectedPath)
      ) {
        viewState.selectedPath = "";
        viewState.lastFileViewPath = "";
        resetFilePreview();
        renderPlaceholder();
      }
      if (viewState.view !== FILES_VIEW) {
        return;
      }
      expandToSelectedPath();
      renderTree();
    });
    viewState.isLoadingIgnoredFiles = false;
    elements.ignoredToggle.disabled = false;
  }

  elements.tabFiles.addEventListener("click", () => {
    void switchView(FILES_VIEW);
  });
  elements.treeWidthToggle.addEventListener("click", () => {
    viewState.isTreeWide = !viewState.isTreeWide;
    renderTreeWidth();
  });
  elements.ignoredToggle.addEventListener("click", () => {
    void toggleIgnoredFiles();
  });
  elements.copyPathButton.addEventListener("click", () => {
    void copyCurrentViewerPath();
  });
  // 交给系统打开：与「复制路径」一样现取 selectedPath，不在渲染时捕获进闭包——
  // 每点一个条目都会换一份路径，捕获的那份会开错文件。
  elements.openInSystemButton.addEventListener("click", () => {
    void openCurrentPathInSystem();
  });
  elements.tabSource.addEventListener("click", () => {
    void setRenderMode(SOURCE_MODE);
  });
  elements.tabPreview.addEventListener("click", () => {
    void setRenderMode(PREVIEW_MODE);
  });
  // 从改动跳到文件：这里现取 selectedPath，而不是在渲染时就把它捕获进闭包——改动视图
  // 里每点一个条目都会换一份路径，捕获的那份会跳错文件。
  elements.openFileButton.addEventListener("click", () => {
    void switchView(FILES_VIEW, viewState.selectedPath);
  });
  elements.refreshButton.addEventListener("click", () => {
    void refreshWorkspace();
  });
  // 内容区里点图片即放大。用事件委托而不是逐个挂监听：图片是每次渲染才创建的，逐个挂必然
  // 会在某条重渲染路径上漏掉。
  elements.viewerBody.addEventListener("click", (clickEvent) => {
    const clickedImage = clickEvent.target.closest("img");
    if (clickedImage) {
      openImageZoom(clickedImage);
    }
  });
  elements.imageZoom.addEventListener("click", () => {
    closeImageZoom();
  });
  // 点图片本身不关，而是切「适应屏幕 / 原始像素」——放大一层只为了看清楚，切两档比反复开关
  // 顺手。所以这里要拦住冒泡，否则外层那个「点空白处关闭」会立刻把它关掉。
  elements.imageZoomImage.addEventListener("click", (clickEvent) => {
    clickEvent.stopPropagation();
    setImageZoomToActualSize(!elements.imageZoom.classList.contains("is-actual-size"));
  });
  elements.tabDiff.addEventListener("click", () => {
    void switchView(DIFF_VIEW);
  });
  elements.filterInput.addEventListener("input", () => {
    viewState.filterText = elements.filterInput.value;
    renderTree();
  });
  window.document.addEventListener("keydown", (keyboardEvent) => {
    // 放大层开着时只认 Esc：底下的 f / d / 过滤快捷键不该在看一张大图时把视图换掉。
    if (!elements.imageZoom.hidden) {
      if (keyboardEvent.key === "Escape") {
        closeImageZoom();
      }
      return;
    }
    if (keyboardEvent.target === elements.filterInput) {
      if (keyboardEvent.key === "Escape") {
        elements.filterInput.blur();
      }
      return;
    }
    if (keyboardEvent.key === "f") {
      void switchView(FILES_VIEW);
    } else if (keyboardEvent.key === "d") {
      void switchView(DIFF_VIEW);
    } else if (keyboardEvent.key === "/") {
      keyboardEvent.preventDefault();
      elements.filterInput.focus();
    }
  });

  void initializeViewer();

/** 文件正文、预览、图片和逐行 diff 的呈现逻辑。 */
"use strict";

  /**
   * 选中一个条目并按当前视图加载内容。
   *
   * 改动视图下条目的身份是「路径 + 分区」：同一个文件可以同时在已暂存与未暂存两段里，
   * 只给路径会拿到错误的 diff。分区缺省时按分区顺序解析。
   * @param {string} repositoryPath 仓库相对路径。
   * @param {string} sectionName 分区取值；文件视图不用。
   */
  async function selectPath(repositoryPath, sectionName = "") {
    viewState.selectedPath = repositoryPath;
    viewState.selectedSection =
      viewState.view === DIFF_VIEW
        ? sectionName || resolveSectionForPath(repositoryPath)
        : "";
    renderTree();
    await guardAgainstServiceExit(async () => {
      if (viewState.view === DIFF_VIEW) {
        await loadDiffForPath(repositoryPath, viewState.selectedSection);
      } else {
        await loadFileContent(repositoryPath);
      }
    });
  }

  /**
   * 加载并渲染单个文件正文。默认渲染源码，预览要由用户显式切换。
   * @param {string} repositoryPath 仓库相对路径。
   */
  async function loadFileContent(repositoryPath) {
    resetFilePreview();
    const fileResponse = await requestJson(`/api/file?path=${encodeURIComponent(repositoryPath)}`);
    setViewerPath(repositoryPath);
    if (fileResponse.status !== 200) {
      renderNotice(buildNotice({ isFailure: true, title: "无法读取", paragraphs: [fileResponse.body.error] }), repositoryPath);
      return;
    }
    // 读到了才算「在文件视图里看过这个文件」：读失败的提示块不该被记下来，否则每次切回
    // 文件视图都会重新弹同一个错误。
    viewState.lastFileViewPath = repositoryPath;
    const fileBody = fileResponse.body;
    if (fileBody.kind === "image") {
      // 图片没有逐行正文，也就没有「源码 / 预览」可切：内容区就是这张图，头部不摆预览控件。
      elements.viewerMeta.textContent = fileBody.size_label;
      elements.statusHint.textContent = "只读视图 · 图片预览";
      renderImagePreview(fileBody);
      return;
    }
    if (fileBody.kind !== "text") {
      elements.viewerMeta.textContent = fileBody.size_label;
      renderNotice(
        buildNotice({
          title: fileBody.kind === "binary" ? "二进制文件" : "文件过大",
          paragraphs: [fileBody.note],
          hint:
            fileBody.kind === "binary"
              ? "二进制文件没有可读正文，查看器不猜测它的内容。"
              : "大文件在浏览器里渲染既慢又难读，可用本地编辑器打开，或点头部的「在系统中打开」。",
        }),
        repositoryPath
      );
      return;
    }
    viewState.loadedFile = fileBody;
    viewState.previewDescriptor = fileBody.preview || null;
    // Markdown 默认进预览：文档先看渲染结果，源码是一次显式切换。其它文本文件默认源码。
    if (viewState.previewDescriptor && viewState.previewDescriptor.mode === "markdown") {
      viewState.renderMode = PREVIEW_MODE;
    }
    renderPreviewControls(viewState.previewDescriptor);
    await renderLoadedFile();
  }

  /**
   * 加载并渲染单个文件在某个分区下的逐行改动。
   * @param {string} repositoryPath 仓库相对路径。
   * @param {string} sectionName 分区取值。
   */
  async function loadDiffForPath(repositoryPath, sectionName) {
    const diffResponse = await requestJson(
      `/api/diff?path=${encodeURIComponent(repositoryPath)}&section=${encodeURIComponent(sectionName)}`
    );
    setViewerPath(repositoryPath);
    if (diffResponse.status !== 200) {
      elements.viewerMeta.textContent = "";
      renderNotice(buildNotice({ isFailure: true, title: "无法读取改动", paragraphs: [diffResponse.body.error] }), repositoryPath);
      return;
    }
    // 与文件视图同一条口径：读到了才算「在改动视图里看过这条」，读失败的应答不记。
    viewState.lastDiffViewSelection = { path: repositoryPath, section: sectionName };
    const diffBody = diffResponse.body;
    // 图片改动放在最前面：它没有逐行内容，但两版画面是可比的，比下面那句「无逐行改动」
    // 有用得多。服务端只在能给出至少一版时才带这个字段。
    if (diffBody.image_comparison) {
      elements.viewerMeta.textContent = diffBody.section_label;
      elements.statusHint.textContent = "只读视图 · 图片改动，旧新两版对比";
      renderImageComparison(diffBody.image_comparison, diffBody.path);
      return;
    }
    // 头部在空态下由 renderNotice 保留路径（「复制 / 在系统中打开」还挂着），所以来源只在
    // 有逐行改动时才写头部，纯重命名与二进制那两支交给提示块正文自己说。
    if (diffBody.empty) {
      renderNotice(
        buildNotice(
          // 纯重命名没有逐行改动可看，但它并不是「与当前内容一致」——路径确实变了，
          // 说成「没有改动」会把人引向错误结论。二进制同理：内容确实变了，只是
          // git 不逐行给。
          diffBody.rename_from
            ? {
                title: "重命名，内容未变",
                paragraphs: [
                  `该文件在「${diffBody.section_label}」里由 <code>${escapeHtmlText(diffBody.rename_from)}</code> 重命名而来，正文没有变化，因此没有逐行改动可看。`,
                ],
              }
            : diffBody.binary
              ? {
                  title: "二进制文件，无逐行改动",
                  paragraphs: [
                    `该文件在「${diffBody.section_label}」里的内容有变化，但 git 判定它是二进制，不提供逐行 diff。`,
                  ],
                  hint: "要对比二进制内容，请在本机用专门工具打开这两个版本，或点头部的「在系统中打开」。",
                }
              : {
                  title: "该文件在此区段下没有改动",
                  paragraphs: [`「${diffBody.section_label}」中没有这个文件的改动。`],
                }
        ),
        repositoryPath
      );
      return;
    }
    elements.viewerMeta.textContent = diffBody.rename_from
      ? `${diffBody.section_label} · 重命名自 ${diffBody.rename_from}`
      : diffBody.section_label;
    if (diffBody.truncated) {
      elements.statusHint.textContent = "只读视图 · 改动过大，仅显示前若干行";
    }
    renderDiffRows(diffBody.rows);
  }

  /**
   * 渲染带行号的正文；行内容来自服务端已转义的 HTML 片段。
   * @param {string[]} highlightedLines 逐行 HTML。
   */
  function renderCodeLines(highlightedLines) {
    const codeBlock = document.createElement("div");
    codeBlock.className = "code";
    const codeFragment = document.createDocumentFragment();
    highlightedLines.forEach((highlightedLine, lineIndex) => {
      const rowNode = document.createElement("div");
      rowNode.className = "row";
      const lineNumberNode = document.createElement("span");
      lineNumberNode.className = "lineno";
      lineNumberNode.textContent = String(lineIndex + 1);
      const sourceNode = document.createElement("span");
      sourceNode.className = "src";
      // 服务端产出的是 Pygments 转义后的行级 HTML 或转义后的纯文本，不含未转义的源码字符。
      sourceNode.innerHTML = highlightedLine;
      rowNode.append(lineNumberNode, sourceNode);
      codeFragment.append(rowNode);
    });
    codeBlock.append(codeFragment);
    elements.viewerBody.replaceChildren(codeBlock);
  }

  /**
   * 渲染图片改动的旧新两版对比。
   *
   * 每一版一栏，栏内是标签 + 图；两栏在宽屏（横屏）左右并排，竖屏或窗口放不下时改上下
   * ——这一条由 CSS 管（见 `.image-compare`），这里只负责把结构摆出来。只有一版时也照
   * 样渲染，缺的那一版由服务端给的 `note` 说明，而不是在这里猜。
   * @param {{panes: Array<{label: string, url: string, size_label: string}>, note: string}} imageComparison 服务端给出的两版信息。
   * @param {string} repositoryPath 仓库相对路径，用于图片的 alt 文本。
   */
  function renderImageComparison(imageComparison, repositoryPath) {
    const compareNode = document.createElement("div");
    compareNode.className =
      imageComparison.panes.length > 1 ? "preview image-compare" : "preview image-compare single";
    for (const pane of imageComparison.panes) {
      const paneNode = document.createElement("figure");
      paneNode.className = "image-pane";
      const captionNode = document.createElement("figcaption");
      captionNode.textContent = `${pane.label} · ${pane.size_label}`;
      const imageNode = document.createElement("img");
      imageNode.src = pane.url;
      imageNode.alt = `${repositoryPath}（${pane.label}）`;
      paneNode.append(captionNode, imageNode);
      compareNode.append(paneNode);
    }
    if (imageComparison.note) {
      const noteNode = document.createElement("p");
      noteNode.className = "image-compare-note";
      noteNode.textContent = imageComparison.note;
      compareNode.append(noteNode);
    }
    elements.viewerBody.replaceChildren(compareNode);
  }

  /**
   * 渲染图片预览：内容区放一张指向 `/raw/` 的图。
   *
   * 地址由服务端拼好（``fileBody.url``）——URL 编码只在服务端一处负责，这里不自己拼路径。
   * 图片原始字节是浏览器直接向 `/raw/` 取，不经过 ``/api/file``，因此这里不读、也没有
   * 「正文过大不渲染」这回事。
   * @param {{url: string, path: string, size_label: string}} imageFile 图片文件应答。
   */
  function renderImagePreview(imageFile) {
    const containerNode = document.createElement("div");
    containerNode.className = "preview image";
    const imageNode = document.createElement("img");
    imageNode.src = imageFile.url;
    // 路径进 alt：图裂或加载中时能看出这里本该是哪张图。
    imageNode.alt = imageFile.path;
    containerNode.append(imageNode);
    elements.viewerBody.replaceChildren(containerNode);
  }

  /**
   * 打开图片放大层：默认「适应屏幕」，点图片可在它和「原始像素」之间来回切。
   *
   * 内容区里任何图片都能点开——文件视图的图片预览、改动视图的旧新两栏、以及 Markdown 里
   * 引到的图，走的是同一个入口（见 viewerBody 上的点击委托）。它只是把同一份 `/raw/` 字节
   * 放大显示，不改任何状态，也不重新取图。
   * @param {HTMLImageElement} imageNode 被点击的图片。
   */
  function openImageZoom(imageNode) {
    elements.imageZoomImage.src = imageNode.src;
    elements.imageZoomImage.alt = imageNode.alt;
    setImageZoomToActualSize(false);
    elements.imageZoom.hidden = false;
  }

  /**
   * 关闭图片放大层。
   *
   * 顺手把 src 清掉：留着会让关掉的放大层继续持有一份解码后的图片，而这一层的图动辄几 MB，
   * 用户看过的每一张都会攒在内存里。
   */
  function closeImageZoom() {
    elements.imageZoom.hidden = true;
    elements.imageZoomImage.removeAttribute("src");
    elements.imageZoomCaption.textContent = "";
  }

  /**
   * 在「适应屏幕」与「原始像素」之间切换放大层。
   *
   * 适应屏幕是打开时的默认值——先看全貌，知道自己看的是什么；要看细节再切到原始像素，那一档
   * 不缩放、由整层滚动承载，截图里的字才真的看得清。
   * @param {boolean} isActualSize 是否切到原始像素。
   */
  function setImageZoomToActualSize(isActualSize) {
    elements.imageZoom.classList.toggle("is-actual-size", isActualSize);
    // 路径从放大层自己的 alt 读：它就在同一份 DOM 上，再存一个变量只会多一处可能不同步的状态。
    const imageLabel = elements.imageZoomImage.alt;
    elements.imageZoomCaption.textContent = isActualSize
      ? `${imageLabel} · 原始像素（可滚动）· 点图片回到适应屏幕 · 点空白处或按 Esc 关闭`
      : `${imageLabel} · 适应屏幕 · 点图片看原始像素 · 点空白处或按 Esc 关闭`;
  }

  /**
   * 清空当前文件的预览状态，并把界面上的预览控件一起收掉。
   *
   * 换文件、切视图都要走一遍：不清的话上一个文件的预览能力（以及已取回的 HTML）会跟着
   * 新文件一起留着，点出来的预览与内容区的正文不是同一个文件。
   *
   * **状态和控件必须一起清。** 两个预览控件的可见性只由 `renderPreviewControls` 决定，
   * 只把 `previewDescriptor` 置空而不同步界面，上一个 `.md` 留下的「源码 / 预览」开关
   * （以及 HTML 留下的「在新标签页打开」）就会滞留在头部——切到改动视图时最明显，那里
   * 根本没有正文可预览。所以这里自己收尾，而不是指望每个调用方都记得再调一次。
   */
  function resetFilePreview() {
    viewState.loadedFile = null;
    viewState.previewDescriptor = null;
    viewState.renderMode = SOURCE_MODE;
    viewState.markdownHtml = null;
    renderPreviewControls(null);
  }

  /**
   * 同步预览控件的可见性与选中态。
   *
   * 两种能力各自对应一个控件：Markdown 给「源码 / 预览」开关，HTML 给「在新标签页
   * 打开」。非 Markdown 文件不显示开关，非 HTML 文件不显示链接——一个点了没反应或
   * 给出错误内容的控件比没有更糟。
   * @param {{mode: string, url?: string}|null} previewDescriptor 服务端给出的预览能力。
   */
  function renderPreviewControls(previewDescriptor) {
    const previewMode = previewDescriptor ? previewDescriptor.mode : "";
    elements.previewSwitch.hidden = previewMode !== "markdown";
    elements.openExternal.hidden = previewMode !== "external";
    if (previewMode === "external") {
      elements.openExternal.href = previewDescriptor.url;
    } else {
      elements.openExternal.removeAttribute("href");
    }
    const isPreview = viewState.renderMode === PREVIEW_MODE;
    elements.tabSource.setAttribute("aria-selected", String(!isPreview));
    elements.tabPreview.setAttribute("aria-selected", String(isPreview));
  }

  /**
   * 按当前模式渲染已加载的文件正文。
   */
  async function renderLoadedFile() {
    const fileBody = viewState.loadedFile;
    elements.viewerMeta.textContent = `${fileBody.language} · ${fileBody.line_count} 行 · ${fileBody.size_label}`;
    if (viewState.renderMode === PREVIEW_MODE) {
      await renderMarkdownPreview();
      return;
    }
    elements.statusHint.textContent = fileBody.highlighted
      ? "只读视图 · 服务端语法高亮"
      : "只读视图 · 未安装语法高亮依赖，已降级为纯文本";
    renderCodeLines(fileBody.lines);
  }

  /** 那份 mermaid 资源的加载过程（含已加载完成的），null 表示还没取过。 */
  let mermaidLoadPromise = null;

  /**
   * 取回 mermaid 的资源。
   *
   * 按需取：页面里没有图时一个字节都不请求——把 3.5 MB 的 mermaid 写进 index.html 会让每个
   * 页面都为它付一次解析成本。取过一次就记着这份 Promise，失败时清回 null，让下一次预览还能
   * 再试（服务在跑就基本不会失败；真取不到时图块退回代码块，见 renderMermaidDiagrams）。
   * @returns {Promise<object>} mermaid 的全局对象。
   */
  function loadMermaidLibrary() {
    if (mermaidLoadPromise === null) {
      mermaidLoadPromise = new Promise((resolve, reject) => {
        const mermaidScriptNode = document.createElement("script");
        mermaidScriptNode.src = "/assets/mermaid.min.js";
        mermaidScriptNode.addEventListener("load", () => {
          if (window.mermaid) {
            resolve(window.mermaid);
            return;
          }
          mermaidLoadPromise = null;
          reject(new Error("mermaid 资源没有挂到全局对象上"));
        });
        mermaidScriptNode.addEventListener("error", () => {
          mermaidLoadPromise = null;
          reject(new Error("mermaid 资源取不到"));
        });
        document.head.append(mermaidScriptNode);
      });
    }
    return mermaidLoadPromise;
  }

  /**
   * 把预览里的 mermaid 块画成 SVG。
   *
   * 服务端把围栏换成 `<pre class="mermaid">` 之后就不管了，图由这里画：mermaid 读的是块里的
   * 文本（服务端已转义，`&gt;` 在浏览器里解回来还是 `>`），画完把内容换成 `<svg>` 并打上
   * `data-processed`——所以同一块重复调用是安全的。来回切「源码 / 预览」会重新插入一份 DOM，
   * 那是新节点，会再画一次。
   *
   * 两种失败都不往上抛：资源取不到时退回代码块并由状态条说明，图本身语法有错时 mermaid 自己
   * 在块里画出错误说明，这里只把状态条补上。
   * @param {HTMLElement} previewNode 刚插进内容区的预览节点。
   */
  async function renderMermaidDiagrams(previewNode) {
    const diagramNodes = Array.from(previewNode.querySelectorAll("pre.mermaid"));
    if (diagramNodes.length === 0) {
      return;
    }
    let mermaidLibrary;
    try {
      mermaidLibrary = await loadMermaidLibrary();
    } catch (loadError) {
      elements.statusHint.textContent = `只读视图 · ${loadError.message}，Mermaid 图按源码显示`;
      return;
    }
    mermaidLibrary.initialize({
      startOnLoad: false,
      fontFamily:
        '-apple-system, BlinkMacSystemFont, "Segoe UI", "PingFang SC", "Microsoft YaHei", sans-serif',
    });
    try {
      await mermaidLibrary.run({ nodes: diagramNodes });
    } catch (renderError) {
      elements.statusHint.textContent = `只读视图 · 有 Mermaid 图没画出来：${renderError.message}`;
    }
  }

  /**
   * 渲染 Markdown 预览。
   *
   * 预览片段向服务端要一次并缓存：来回切换是查看 Markdown 的常规动作，每次都重新请求
   * 既慢又要多跑一遍服务端渲染。取不到时退回源码并说明原因，不把空白预览留在那里。
   */
  async function renderMarkdownPreview() {
    if (viewState.markdownHtml === null) {
      const markdownResponse = await requestJson(
        `/api/markdown?path=${encodeURIComponent(viewState.loadedFile.path)}`
      );
      if (markdownResponse.status !== 200) {
        await setRenderMode(SOURCE_MODE);
        elements.statusHint.textContent = `只读视图 · 无法生成预览：${markdownResponse.body.error}`;
        return;
      }
      viewState.markdownHtml = markdownResponse.body.html;
    }
    const previewNode = document.createElement("div");
    previewNode.className = "preview markdown";
    // 服务端渲染出的 HTML 片段。Markdown 正文里的 raw HTML 会在这里执行——预览是用户
    // 主动点开的一次，且查看器是绑在回环上的本机工具。
    previewNode.innerHTML = viewState.markdownHtml;
    elements.viewerBody.replaceChildren(previewNode);
    elements.statusHint.textContent = "只读视图 · Markdown 预览（服务端渲染）";
    await renderMermaidDiagrams(previewNode);
  }

  /**
   * 切换源码与预览。
   * @param {"source"|"preview"} nextMode 目标模式。
   */
  async function setRenderMode(nextMode) {
    if (!viewState.loadedFile || viewState.renderMode === nextMode) {
      return;
    }
    viewState.renderMode = nextMode;
    renderPreviewControls(viewState.previewDescriptor);
    await guardAgainstServiceExit(async () => {
      await renderLoadedFile();
    });
  }

  /**
   * 渲染逐行 diff。
   * @param {Array<{kind: string, old_no: number|null, new_no: number|null, text: string}>} diffRows 逐行改动。
   */
  function renderDiffRows(diffRows) {
    const codeBlock = document.createElement("div");
    codeBlock.className = "code diff";
    const codeFragment = document.createDocumentFragment();
    for (const diffRow of diffRows) {
      const rowNode = document.createElement("div");
      rowNode.className = `row ${diffRow.kind}`;
      if (diffRow.kind === "hunk") {
        rowNode.textContent = diffRow.text;
        codeFragment.append(rowNode);
        continue;
      }
      const lineNumberNode = document.createElement("span");
      lineNumberNode.className = "lineno";
      const oldLineNumberNode = document.createElement("span");
      oldLineNumberNode.className = "old-line-no";
      oldLineNumberNode.textContent = diffRow.old_no === null ? "" : String(diffRow.old_no);
      const lineNumberSeparatorNode = document.createElement("em");
      lineNumberSeparatorNode.className = "line-number-separator";
      lineNumberSeparatorNode.textContent = "│";
      const newLineNumberNode = document.createElement("span");
      newLineNumberNode.className = "new-line-no";
      newLineNumberNode.textContent = diffRow.new_no === null ? "" : String(diffRow.new_no);
      lineNumberNode.append(oldLineNumberNode, lineNumberSeparatorNode, newLineNumberNode);
      const sourceNode = document.createElement("span");
      sourceNode.className = "src";
      sourceNode.textContent = diffRow.text;
      rowNode.append(lineNumberNode, sourceNode);
      codeFragment.append(rowNode);
    }
    codeBlock.append(codeFragment);
    elements.viewerBody.replaceChildren(codeBlock);
  }

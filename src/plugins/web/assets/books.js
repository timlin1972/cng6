// cng6 電子書: 書庫與閱讀器(PC 視窗與手機共用)
//
// - EPUB / MOBI / TXT: 章節在 iframe(禁止 script)裡顯示, 由這裡注入閱讀設定
//   翻頁: 橫排用 CSS 分欄、直排(vertical-rl)直接左右捲動; 捲動: 原生捲動
// - 漫畫: 一頁一張圖; PDF: pdf.js(見 PdfView)
// - 閱讀進度與書籤存在 Pi 上(/api/books/{file}/progress), 各裝置同步
"use strict";

const BooksApp = (() => {
  const enc = encodeURIComponent;
  const encPath = (p) => p.split("/").map(enc).join("/");
  function h(tag, cls, text) {
    const el = document.createElement(tag);
    if (cls) el.className = cls;
    if (text != null) el.textContent = text;
    return el;
  }
  const quoteArg = (a) => (a !== "" && !/[\s"'\\]/.test(a) ? a : `"${a.replace(/\\/g, "\\\\").replace(/"/g, '\\"')}"`);
  const ICON = {
    back: `<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.2" stroke-linecap="round" stroke-linejoin="round"><path d="M15 5l-7 7 7 7"/></svg>`,
    toc: `<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round"><path d="M8 6h13M8 12h13M8 18h13M3 6h.01M3 12h.01M3 18h.01"/></svg>`,
    bookmark: `<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linejoin="round"><path d="M6 3h12v18l-6-4-6 4z"/></svg>`,
    font: `<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><path d="M4 20l5-14 5 14M6 15h6M15 20l3-8 3 8M16 17.5h4"/></svg>`,
    more: `<svg viewBox="0 0 24 24"><circle cx="5" cy="12" r="1.8" fill="currentColor"/><circle cx="12" cy="12" r="1.8" fill="currentColor"/><circle cx="19" cy="12" r="1.8" fill="currentColor"/></svg>`,
  };
  const KIND_NAME = { epub: "EPUB", mobi: "MOBI", txt: "TXT", pdf: "PDF", comic: "漫畫" };

  // ---- 閱讀設定(每台裝置各自記住) ----
  const PREF_KEY = "cng6.books.prefs";
  function loadPrefs(mobile) {
    const def = { size: mobile ? 19 : 20, line: 1.8, font: "serif", theme: "auto", mode: "paged" };
    try {
      return { ...def, ...JSON.parse(localStorage.getItem(PREF_KEY)) };
    } catch {
      return def;
    }
  }
  function savePrefs(p) {
    try {
      localStorage.setItem(PREF_KEY, JSON.stringify(p));
    } catch {
      /* 略過 */
    }
  }
  const THEMES = {
    white: { bg: "#ffffff", fg: "#1d1d1f", link: "#0a62c9" },
    sepia: { bg: "#f6efe0", fg: "#3b2f20", link: "#8a5a1c" },
    dark: { bg: "#17171a", fg: "#d6d6d8", link: "#7ab7ff" },
  };
  const FONTS = {
    serif: `"Noto Serif TC", "Noto Serif CJK TC", "Source Han Serif TC", "PMingLiU", "MingLiU", "Songti TC", serif`,
    sans: `"Noto Sans TC", "Noto Sans CJK TC", "PingFang TC", "Microsoft JhengHei", sans-serif`,
    kai: `"LXGW WenKai TC", "BiauKai", "DFKai-SB", "Kaiti TC", "STKaiti", serif`,
  };
  // 手機通常沒有中文明體、楷體: 由 Pi 提供網路字型(第一次用到時 Pi 會先下載)
  const WEB_FONTS = { serif: "/books/fonts/serif.css", kai: "/books/fonts/kai.css" };
  const isDark = () =>
    document.documentElement.dataset.theme === "dark" ||
    (document.documentElement.dataset.theme !== "light" && matchMedia("(prefers-color-scheme: dark)").matches);
  const themeOf = (p) => THEMES[p.theme === "auto" ? (isDark() ? "dark" : "white") : p.theme];

  // 自動產生的封面(書裡沒有圖片時)
  function coverEl(b, cls = "bk-cover") {
    const c = h("span", cls);
    if (b.has_cover) {
      const img = h("img");
      img.loading = "lazy";
      img.alt = "";
      img.src = `/books/cover/${enc(b.file)}`;
      img.onerror = () => img.remove();
      c.appendChild(img);
    }
    let hash = 0;
    for (const ch of b.title) hash = (hash * 31 + ch.codePointAt(0)) >>> 0;
    c.style.setProperty("--hue", String(hash % 360));
    const t = h("span", "bk-cover-text");
    t.append(h("b", null, b.title), h("small", null, b.author || ""));
    c.prepend(t);
    return c;
  }

  // ======================================================================
  function mount(el, { device = "pc" } = {}) {
    const mobile = device === "mobile";
    el.classList.add("bk-root", mobile ? "bk-mobile" : "bk-pc");
    let closed = false;
    let stopReader = null;

    function toast(text, ok = true) {
      let t = el.querySelector(".bk-toast");
      if (!t) {
        t = h("div", "bk-toast");
        el.appendChild(t);
      }
      t.textContent = text;
      t.dataset.ok = ok;
      t.classList.add("show");
      clearTimeout(t._timer);
      t._timer = setTimeout(() => t.classList.remove("show"), 2600);
    }

    // ================================================================ 書庫
    async function showLibrary() {
      if (stopReader) stopReader();
      stopReader = null;
      el.innerHTML = `
        <div class="bk-lib">
          <header class="bk-lib-head">
            <h2>書庫</h2>
            <input type="search" class="bk-search" placeholder="搜尋書名、作者" spellcheck="false">
            <label class="bk-btn">上傳…<input type="file" hidden multiple accept=".epub,.mobi,.azw,.azw3,.txt,.pdf,.cbz,.zip"></label>
          </header>
          <div class="bk-lib-body"></div>
        </div>`;
      const body = el.querySelector(".bk-lib-body");
      const search = el.querySelector(".bk-search");
      const file = el.querySelector('input[type="file"]');
      let books = [];

      file.addEventListener("change", async () => {
        const files = [...file.files];
        file.value = "";
        for (const f of files) {
          toast(`上傳中… ${f.name}`);
          try {
            const r = await fetch(`/api/books/upload/${enc(f.name)}`, { method: "PUT", body: f });
            if (!r.ok) throw new Error(await r.text());
          } catch (e) {
            toast(`${f.name}: ${e.message}`, false);
            continue;
          }
        }
        toast(`已上傳 ${files.length} 本`);
        load();
      });
      search.addEventListener("input", render);

      function card(b) {
        const c = h("div", "bk-book");
        c.tabIndex = 0;
        c.title = `${b.title}${b.author ? ` — ${b.author}` : ""}`;
        const cover = coverEl(b);
        if (b.percent > 0) {
          const bar = h("span", "bk-prog");
          const fill = h("span");
          fill.style.width = `${(b.percent * 100).toFixed(1)}%`;
          bar.appendChild(fill);
          cover.appendChild(bar);
        }
        cover.appendChild(h("span", "bk-kind", KIND_NAME[b.kind] || b.kind));
        const info = h("div", "bk-book-info");
        info.append(h("div", "bk-book-title", b.title), h("div", "bk-book-sub", [b.author, b.percent > 0 ? `${Math.round(b.percent * 100)}%` : ""].filter(Boolean).join(" · ")));
        const more = h("button", "bk-more");
        more.innerHTML = ICON.more;
        more.title = "更多";
        more.addEventListener("click", async (e) => {
          e.stopPropagation();
          if (!confirm(`刪除「${b.title}」?\n\n檔案 ${b.file} 會從書庫刪除, 閱讀進度與書籤也會一起刪除。`)) return;
          const r = await Apps.exec(`/books delete ${quoteArg(b.file)}`);
          toast(r.text || "已刪除", r.ok);
          load();
        });
        c.append(cover, info, more);
        const open = () => openBook(b);
        c.addEventListener("click", open);
        c.addEventListener("keydown", (e) => e.key === "Enter" && open());
        return c;
      }

      function render() {
        const q = search.value.trim().toLowerCase();
        const shown = books.filter((b) => !q || b.title.toLowerCase().includes(q) || b.author.toLowerCase().includes(q) || b.file.toLowerCase().includes(q));
        body.replaceChildren();
        const recent = shown.filter((b) => b.last_read > 0).sort((a, b) => b.last_read - a.last_read).slice(0, mobile ? 3 : 6);
        if (recent.length && !q) {
          body.append(h("h3", null, "繼續閱讀"));
          const g = h("div", "bk-grid");
          recent.forEach((b) => g.append(card(b)));
          body.append(g);
          body.append(h("h3", null, `所有書 (${shown.length})`));
        }
        const g = h("div", "bk-grid");
        shown.forEach((b) => g.append(card(b)));
        body.append(g);
        if (!books.length) body.append(h("div", "bk-empty", "書庫是空的: 按「上傳…」, 或把書放進 books 資料夾(EPUB / MOBI / AZW3 / TXT / PDF / CBZ)"));
        else if (!shown.length) body.append(h("div", "bk-empty", `找不到「${search.value}」`));
      }

      async function load() {
        try {
          books = (await (await fetch("/api/books", { cache: "no-store" })).json()).books;
        } catch {
          body.replaceChildren(h("div", "bk-empty", "無法讀取書庫"));
          return;
        }
        render();
      }
      load();
    }

    // ================================================================ 閱讀器
    async function openBook(b) {
      let d;
      try {
        d = await (await fetch(`/api/books/${enc(b.file)}`, { cache: "no-store" })).json();
      } catch {
        return toast("無法開啟這本書", false);
      }
      if (d.kind === "converting") {
        return toast(d.error ? `無法開啟: ${d.error}` : "MOBI 轉檔中, 請稍後再開", false);
      }
      stopReader = startReader(d);
    }

    function startReader(d) {
      const prefs = loadPrefs(mobile);
      el.innerHTML = `
        <div class="bk-reader">
          <header class="bk-bar top">
            <button class="bk-ib" data-act="back" title="回書庫">${ICON.back}</button>
            <div class="bk-title"><b></b><span></span></div>
            <button class="bk-ib" data-act="toc" title="目錄">${ICON.toc}</button>
            <button class="bk-ib" data-act="mark" title="書籤">${ICON.bookmark}</button>
            <button class="bk-ib" data-act="prefs" title="閱讀設定">${ICON.font}</button>
          </header>
          <div class="bk-stage"></div>
          <footer class="bk-bar bottom">
            <span class="bk-chapter"></span>
            <input type="range" class="bk-slider" min="0" max="1000" value="0" aria-label="閱讀進度">
            <span class="bk-percent">0%</span>
          </footer>
          <aside class="bk-panel" hidden></aside>
        </div>`;
      const reader = el.querySelector(".bk-reader");
      const stage = el.querySelector(".bk-stage");
      const panel = el.querySelector(".bk-panel");
      const slider = el.querySelector(".bk-slider");
      const percentEl = el.querySelector(".bk-percent");
      const chapterEl = el.querySelector(".bk-chapter");
      el.querySelector(".bk-title b").textContent = d.title;
      el.querySelector(".bk-title span").textContent = d.author || "";

      const progress = { ...d.progress, bookmarks: d.progress.bookmarks || [] };
      // 各格式的檢視: { go(chapter, pos), next(), prev(), position(): {chapter,pos,percent,label}, total, apply(prefs), stop() }
      let view;
      const chapterTitles = chapterTitlesOf(d);

      // ---- 進度: 稍等再存, 避免每翻一頁都寫入 ----
      let saveTimer = null;
      function onMove() {
        const p = view.position();
        progress.chapter = p.chapter;
        progress.pos = p.pos;
        progress.percent = p.percent;
        if (!slider.matches(":active")) slider.value = Math.round(p.percent * 1000);
        percentEl.textContent = `${Math.round(p.percent * 100)}%`;
        chapterEl.textContent = p.label || "";
        clearTimeout(saveTimer);
        saveTimer = setTimeout(saveProgress, 1200);
      }
      function saveProgress() {
        clearTimeout(saveTimer);
        fetch(`/api/books/${enc(d.file)}/progress`, {
          method: "PUT",
          headers: { "content-type": "application/json" },
          body: JSON.stringify(progress),
          keepalive: true,
        }).catch(() => {});
      }

      slider.addEventListener("change", () => view.seekPercent(slider.value / 1000));

      // ---- 面板: 目錄 / 書籤 / 設定 ----
      function openPanel(kind) {
        if (!panel.hidden && panel.dataset.kind === kind) {
          panel.hidden = true;
          return;
        }
        panel.dataset.kind = kind;
        panel.replaceChildren();
        panel.hidden = false;
        if (kind === "toc") renderToc();
        else if (kind === "mark") renderMarks();
        else renderPrefs();
      }
      function closePanel() {
        panel.hidden = true;
      }
      function renderToc() {
        panel.append(h("h3", null, "目錄"));
        const list = h("div", "bk-list");
        const items = tocOf(d);
        const cur = view.position().chapter;
        items.forEach((t) => {
          const b = h("button", `bk-toc-item ${t.chapter === cur ? "on" : ""}`, t.title || "(無標題)");
          b.style.paddingLeft = `${12 + t.level * 16}px`;
          b.addEventListener("click", () => {
            closePanel();
            view.go(t.chapter, 0, t.anchor);
          });
          list.append(b);
        });
        if (!items.length) list.append(h("div", "bk-empty", "這本書沒有目錄"));
        panel.append(list);
        list.querySelector(".on")?.scrollIntoView({ block: "center" });
      }
      function renderMarks() {
        panel.append(h("h3", null, "書籤"));
        const add = h("button", "bk-btn primary", "加入書籤(目前位置)");
        add.addEventListener("click", () => {
          const p = view.position();
          progress.bookmarks.push({ chapter: p.chapter, pos: p.pos, percent: p.percent, label: p.label || `${Math.round(p.percent * 100)}%`, created: Math.floor(Date.now() / 1000) });
          saveProgress();
          toast("已加入書籤");
          panel.replaceChildren();
          renderMarks();
        });
        panel.append(add);
        const list = h("div", "bk-list");
        [...progress.bookmarks]
          .map((m, i) => ({ m, i }))
          .sort((a, b) => a.m.percent - b.m.percent)
          .forEach(({ m, i }) => {
            const row = h("div", "bk-mark");
            const go = h("button", "bk-mark-go");
            go.append(h("b", null, m.label), h("span", null, `${Math.round(m.percent * 100)}% · ${new Date(m.created * 1000).toLocaleDateString("zh-TW")}`));
            go.addEventListener("click", () => {
              closePanel();
              view.go(m.chapter, m.pos);
            });
            const del = h("button", "bk-mark-del", "×");
            del.title = "刪除書籤";
            del.addEventListener("click", () => {
              progress.bookmarks.splice(i, 1);
              saveProgress();
              panel.replaceChildren();
              renderMarks();
            });
            row.append(go, del);
            list.append(row);
          });
        if (!progress.bookmarks.length) list.append(h("div", "bk-empty", "還沒有書籤"));
        panel.append(list);
      }
      function renderPrefs() {
        panel.append(h("h3", null, "閱讀設定"));
        const row = (label, ...ctl) => {
          const r = h("div", "bk-pref");
          r.append(h("span", null, label), ...ctl);
          panel.append(r);
        };
        const seg = (options, value, onPick) => {
          const s = h("div", "bk-seg");
          for (const [v, label] of options) {
            const b = h("button", v === value ? "on" : "", label);
            b.addEventListener("click", () => {
              onPick(v);
              savePrefs(prefs);
              view.apply(prefs);
              panel.replaceChildren();
              renderPrefs();
            });
            s.append(b);
          }
          return s;
        };
        const textual = d.kind === "epub" || d.kind === "txt";
        if (textual) {
          const minus = h("button", "bk-btn", "A−");
          const plus = h("button", "bk-btn", "A+");
          const val = h("b", "bk-pref-val", `${prefs.size}`);
          minus.addEventListener("click", () => {
            prefs.size = Math.max(12, prefs.size - 1);
            savePrefs(prefs);
            view.apply(prefs);
            val.textContent = prefs.size;
          });
          plus.addEventListener("click", () => {
            prefs.size = Math.min(40, prefs.size + 1);
            savePrefs(prefs);
            view.apply(prefs);
            val.textContent = prefs.size;
          });
          row("字體大小", minus, val, plus);
          row("行距", seg([[1.5, "緊"], [1.8, "中"], [2.1, "鬆"]], prefs.line, (v) => (prefs.line = v)));
          row("字型", seg([["serif", "明體"], ["sans", "黑體"], ["kai", "楷體"], ["book", "原書"]], prefs.font, (v) => (prefs.font = v)));
          // 排版是每本書各自記住
          row(
            "排版",
            seg([["auto", "自動"], ["h", "橫排"], ["v", "直排"]], prefs.flows?.[d.file] || "auto", (v) => {
              prefs.flows = { ...(prefs.flows || {}) };
              if (v === "auto") delete prefs.flows[d.file];
              else prefs.flows[d.file] = v;
            })
          );
        }
        row("主題", seg([["auto", "跟系統"], ["white", "白"], ["sepia", "米黃"], ["dark", "深色"]], prefs.theme, (v) => (prefs.theme = v)));
        row("閱讀方式", seg([["paged", "翻頁"], ["scroll", "捲動"]], prefs.mode, (v) => (prefs.mode = v)));
      }

      reader.querySelector('[data-act="back"]').addEventListener("click", () => {
        saveProgress();
        showLibrary();
      });
      reader.querySelector('[data-act="toc"]').addEventListener("click", () => openPanel("toc"));
      reader.querySelector('[data-act="mark"]').addEventListener("click", () => openPanel("mark"));
      reader.querySelector('[data-act="prefs"]').addEventListener("click", () => openPanel("prefs"));

      // 鍵盤: ← → 翻頁, Esc 關面板
      const onKey = (e) => {
        if (closed || e.target.closest?.("input, textarea")) return;
        if (e.key === "Escape" && !panel.hidden) return closePanel();
        // 直排的書由右往左翻: ← 是下一頁
        const rtl = view.vertical?.();
        const fwd = ["PageDown", " ", rtl ? "ArrowLeft" : "ArrowRight"];
        const back = ["PageUp", rtl ? "ArrowRight" : "ArrowLeft"];
        if (fwd.includes(e.key)) {
          e.preventDefault();
          view.next();
        } else if (back.includes(e.key)) {
          e.preventDefault();
          view.prev();
        }
      };
      el.tabIndex = -1;
      el.addEventListener("keydown", onKey);
      setTimeout(() => el.focus(), 50);

      const ctx = { d, stage, prefs, onMove, chapterTitles, closePanel, toast, mobile, keydown: onKey };
      if (d.kind === "epub" || d.kind === "txt") view = TextView(ctx);
      else if (d.kind === "comic") view = ComicView(ctx);
      else view = PdfView(ctx);
      reader.dataset.theme = (prefs.theme === "auto" ? (isDark() ? "dark" : "white") : prefs.theme);
      const applyTheme = view.apply;
      view.apply = (p) => {
        reader.dataset.theme = p.theme === "auto" ? (isDark() ? "dark" : "white") : p.theme;
        applyTheme(p);
      };
      view.go(progress.chapter || 0, progress.pos || 0);

      const onTheme = () => view.apply(prefs);
      window.addEventListener("cng6-theme", onTheme);
      const onHide = () => document.hidden && saveProgress();
      document.addEventListener("visibilitychange", onHide);
      return () => {
        saveProgress();
        view.stop();
        window.removeEventListener("cng6-theme", onTheme);
        document.removeEventListener("visibilitychange", onHide);
        el.removeEventListener("keydown", onKey);
      };
    }

    showLibrary();
    return () => {
      closed = true;
      if (stopReader) stopReader();
    };
  }

  // ---- 目錄與章節標題 ----
  function chapterTitlesOf(d) {
    if (d.kind === "txt") return d.chapters;
    if (d.kind !== "epub") return [];
    // 每章用目錄裡最後一個指向它(或它之前)的標題
    const titles = new Array(d.chapters.length).fill("");
    const idx = (href) => d.chapters.indexOf(href.split("#")[0]);
    let last = "";
    const firstOf = {};
    for (const t of d.toc) {
      const i = idx(t.href);
      if (i >= 0 && firstOf[i] == null) firstOf[i] = t.title;
    }
    for (let i = 0; i < titles.length; i++) {
      if (firstOf[i] != null) last = firstOf[i];
      titles[i] = last;
    }
    return titles;
  }
  function tocOf(d) {
    if (d.kind === "txt") return d.chapters.map((t, i) => ({ title: t, chapter: i, level: 0 }));
    if (d.kind === "epub") {
      return d.toc
        .map((t) => {
          const [path, anchor] = t.href.split("#");
          return { title: t.title, chapter: d.chapters.indexOf(path), anchor, level: t.level };
        })
        .filter((t) => t.chapter >= 0);
    }
    if (d.kind === "comic") return Array.from({ length: d.pages }, (_, i) => ({ title: `第 ${i + 1} 頁`, chapter: i, level: 0 }));
    if (d.kind === "pdf") {
      // PDF 有書籤(outline)就用書籤, 沒有就列出每一頁
      if (d.pdfToc?.length) return d.pdfToc;
      return Array.from({ length: d.pages || 0 }, (_, i) => ({ title: `第 ${i + 1} 頁`, chapter: i, level: 0 }));
    }
    return [];
  }

  // ======================================================================
  // EPUB / TXT: 章節在 iframe 裡
  // ======================================================================
  function TextView(ctx) {
    const { d, stage, onMove, chapterTitles, closePanel, mobile } = ctx;
    let prefs = ctx.prefs;
    const n = d.chapters.length;
    let chapter = 0;
    let frame = null;
    let vertical = false;
    let bookVertical = false; // 書本身的 CSS 是直排
    let page = 0; // 翻頁模式: 目前第幾頁
    let pages = 1;
    let starts = [0]; // 直排翻頁: 每一頁從哪裡開始(離最右邊的距離)
    let loading = false;

    const urlOf = (i, asHtml) =>
      d.kind === "txt"
        ? `/books/txt/${encodeURIComponent(d.file)}/${i}`
        : `/books/res/${encodeURIComponent(d.file)}/${encPath(d.chapters[i])}${asHtml ? "?as=html" : ""}`;

    // 點左右兩側翻頁在章節裡處理(見 hook), 不另外蓋一層, 才不會擋到捲動與選取文字
    stage.classList.add("bk-text");

    function docOf() {
      return frame?.contentDocument;
    }
    function scroller() {
      const doc = docOf();
      return doc && (doc.scrollingElement || doc.documentElement);
    }

    /** 注入閱讀設定 */
    function inject() {
      const doc = docOf();
      if (!doc) return;
      const th = THEMES[prefs.theme === "auto" ? (isDark() ? "dark" : "white") : prefs.theme];
      const W = stage.clientWidth;
      const H = stage.clientHeight;
      const padX = mobile ? 20 : Math.max(40, Math.round(W * 0.07));
      const padY = mobile ? 24 : 36;
      const paged = prefs.mode === "paged";
      const font = prefs.font === "book" ? "" : `font-family: ${FONTS[prefs.font]} !important;`;
      // 直排: 使用者指定 > 書的 CSS > 由右往左翻的書(很多書只寫 -epub-writing-mode, 瀏覽器不認得)
      const flow = prefs.flows?.[d.file] || "auto";
      vertical = flow === "v" || (flow === "auto" && (bookVertical || d.direction === "rtl"));
      let css = `
        html { background: ${th.bg} !important; -webkit-text-size-adjust: 100%; }
        body { color: ${th.fg} !important; background: transparent !important; font-size: ${prefs.size}px !important;
               line-height: ${prefs.line} !important; ${font} margin: 0 !important; padding: ${padY}px ${padX}px !important; box-sizing: border-box; }
        body * { color: inherit !important; background-color: transparent !important; line-height: inherit !important; ${font} }
        a, a * { color: ${th.link} !important; }
        img, svg, video { max-width: 100% !important; height: auto !important; }
        p { text-align: justify; }`;
      // 字體大小: 書裡寫死的 px 字級改用相對大小
      css += `p, div, span, li, td { font-size: inherit !important; }`;
      css += vertical
        ? `html, body { writing-mode: vertical-rl !important; } img, svg { max-width: none !important; }`
        : flow === "h"
          ? `html, body, body * { writing-mode: horizontal-tb !important; }`
          : "";
      if (paged) {
        if (!vertical) {
          // 橫排: 分欄, 一欄 = 一頁
          css += `
            html { height: ${H}px !important; overflow: hidden !important; }
            body { height: ${H}px !important; column-width: ${W - padX * 2}px !important; column-gap: ${padX * 2}px !important;
                   column-fill: auto !important; overflow: visible !important; }
            img, svg { max-height: ${H - padY * 2}px !important; object-fit: contain; break-inside: avoid; }`;
        } else {
          // 直排: 左右留白放在 iframe 外面, 分頁由 measure() 依每一行的位置計算
          css += `
            html { height: ${H}px !important; overflow: hidden !important; }
            body { height: ${H}px !important; padding-left: 0 !important; padding-right: 0 !important; }
            p { margin-left: 0 !important; margin-right: 0 !important; }
            img, svg { max-height: ${H - padY * 2}px !important; }`;
        }
      } else {
        css += `html { overflow: auto !important; }`;
      }
      let style = doc.getElementById("cng6-reader");
      if (!style) {
        style = doc.createElement("style");
        style.id = "cng6-reader";
        (doc.head || doc.documentElement).appendChild(style);
      }
      style.textContent = css;
      const webFont = WEB_FONTS[prefs.font];
      if (webFont && !doc.querySelector(`link[data-cng6-font="${prefs.font}"]`)) {
        const link = doc.createElement("link");
        link.rel = "stylesheet";
        link.href = webFont;
        link.dataset.cng6Font = prefs.font;
        (doc.head || doc.documentElement).appendChild(link);
      }
      if (paged && vertical) {
        frame.style.width = `${W - padX * 2}px`;
        frame.style.left = `${padX}px`;
      } else {
        frame.style.width = "";
        frame.style.left = "";
        frame.style.clipPath = "";
      }
      stage.classList.toggle("paged", paged);
      stage.classList.toggle("vertical", vertical);
    }

    function measure() {
      const s = scroller();
      if (!s) return;
      if (prefs.mode !== "paged") return;
      if (!vertical) {
        pages = Math.max(1, Math.ceil(s.scrollWidth / pageStep() - 0.02));
        return;
      }
      // 直排: 找出每一行的位置, 一頁放得下幾行就放幾行, 不會切到半行
      const doc = docOf();
      const fw = pageStep();
      const scrolled = -s.scrollLeft;
      const cols = [];
      const add = (r) => r.width > 0 && r.height > 0 && cols.push([fw - r.right + scrolled, fw - r.left + scrolled]);
      const walker = doc.createTreeWalker(doc.body || doc.documentElement, NodeFilter.SHOW_TEXT);
      const range = doc.createRange();
      for (let t = walker.nextNode(); t; t = walker.nextNode()) {
        if (!t.data.trim()) continue;
        range.selectNodeContents(t);
        for (const r of range.getClientRects()) add(r);
      }
      for (const img of doc.querySelectorAll("img, svg, video")) add(img.getBoundingClientRect());
      cols.sort((a, b) => a[0] - b[0]);
      starts = [0];
      let st = 0;
      for (const [a, b] of cols) {
        if (b - st > fw + 0.5 && a > st + 0.5) {
          st = a;
          starts.push(a);
        }
      }
      pages = starts.length;
    }

    /** 一頁的寬度(直排時 iframe 比畫面窄, 見 inject) */
    function pageStep() {
      return frame?.clientWidth || stage.clientWidth;
    }

    function showPage(p) {
      const s = scroller();
      if (!s) return;
      page = Math.max(0, Math.min(p, pages - 1));
      if (vertical) {
        // 直排(vertical-rl)的捲動座標往左是負的; 下一頁的第一行如果露出一部分就遮掉
        const st = starts[page] ?? 0;
        const end = starts[page + 1];
        s.scrollLeft = -st;
        const hide = end === undefined ? 0 : Math.max(0, pageStep() - (end - st));
        frame.style.clipPath = hide > 0 ? `inset(0 0 0 ${hide}px)` : "";
      } else {
        s.scrollLeft = page * pageStep();
      }
      onMove();
    }

    function load(i, pos = 0, anchor = null) {
      if (i < 0 || i >= n) return;
      chapter = i;
      loading = true;
      const old = frame;
      frame = h("iframe", "bk-frame");
      // 書裡的 script 由伺服器的 CSP(script-src 'none')擋掉; 這裡要 allow-scripts,
      // 我們掛在書頁上的點擊、滑動事件才會觸發。sandbox 另外擋掉表單與跳轉整個頁面。
      frame.setAttribute("sandbox", "allow-same-origin allow-scripts");
      frame.title = d.title;
      let triedHtml = false;
      frame.addEventListener("load", () => {
        const doc = docOf();
        if (!doc) return;
        // XHTML 格式錯誤: 改用寬鬆的 HTML 解析再載一次
        if (!triedHtml && d.kind !== "txt" && doc.querySelector("parsererror")) {
          triedHtml = true;
          frame.src = urlOf(i, true);
          return;
        }
        const wm = getComputedStyle(doc.body || doc.documentElement).writingMode || "";
        bookVertical = wm.startsWith("vertical");
        inject();
        hook(doc);
        // 等圖片與字型排版完成再定位
        const ready = () => {
          measure();
          loading = false;
          if (anchor) {
            const target = doc.getElementById(anchor) || doc.querySelector(`[name="${CSS.escape(anchor)}"]`);
            if (target) return gotoElement(target);
          }
          setPos(pos);
        };
        (doc.fonts?.ready || Promise.resolve()).then(() => setTimeout(ready, 60));
      });
      frame.src = urlOf(i);
      stage.prepend(frame);
      if (old) old.remove();
    }

    function setPos(pos) {
      const s = scroller();
      if (!s) return;
      if (prefs.mode === "paged") {
        showPage(Math.round(pos * (pages - 1)));
      } else if (vertical) {
        const max = s.scrollWidth - s.clientWidth;
        s.scrollLeft = -pos * max;
        onMove();
      } else {
        s.scrollTop = pos * (s.scrollHeight - s.clientHeight);
        onMove();
      }
    }

    function gotoElement(el) {
      const s = scroller();
      if (prefs.mode === "paged") {
        const r = el.getBoundingClientRect();
        if (vertical) {
          const x = -s.scrollLeft + (pageStep() - r.right);
          let p = 0;
          while (p + 1 < starts.length && starts[p + 1] <= x + 1) p++;
          showPage(p);
        } else {
          showPage(Math.floor((s.scrollLeft + r.left) / pageStep()));
        }
      } else {
        el.scrollIntoView({ block: "start", inline: "start" });
        onMove();
      }
    }

    function fraction() {
      const s = scroller();
      if (!s) return 0;
      if (prefs.mode === "paged") return pages > 1 ? page / (pages - 1) : 0;
      if (vertical) {
        const max = s.scrollWidth - s.clientWidth;
        return max > 0 ? Math.min(1, -s.scrollLeft / max) : 0;
      }
      const max = s.scrollHeight - s.clientHeight;
      return max > 0 ? Math.min(1, s.scrollTop / max) : 0;
    }

    /** 章節內的事件: 連結、捲動、手勢、鍵盤 */
    /** 排版改變(字型載入完成)後重新分頁, 保持目前位置 */
    let relayoutTimer = null;
    function relayout() {
      clearTimeout(relayoutTimer);
      relayoutTimer = setTimeout(() => {
        if (!docOf() || loading) return;
        const pos = fraction();
        measure();
        setPos(pos);
      }, 150);
    }

    function hook(doc) {
      doc.fonts?.addEventListener("loadingdone", relayout);
      doc.addEventListener("click", (e) => {
        const a = e.target.closest?.("a[href]");
        if (a) {
          e.preventDefault();
          const href = a.getAttribute("href");
          if (/^[a-z]+:/i.test(href) && !href.startsWith(location.origin)) {
            window.open(href, "_blank", "noopener");
            return;
          }
          // 書裡的連結: 換成章節 + 錨點
          const url = new URL(href, doc.baseURI);
          const prefix = `/books/res/${encodeURIComponent(d.file)}/`;
          if (url.pathname.startsWith(prefix)) {
            const path = decodeURIComponent(url.pathname.slice(prefix.length));
            const idx = d.chapters.indexOf(path);
            const anchor = url.hash ? decodeURIComponent(url.hash.slice(1)) : null;
            if (idx === chapter && anchor) {
              const t = doc.getElementById(anchor);
              if (t) gotoElement(t);
            } else if (idx >= 0) {
              load(idx, 0, anchor);
            }
          }
          return;
        }
        closePanel();
        // 翻頁模式: 點左右兩側翻頁
        if (prefs.mode === "paged") {
          const x = e.clientX / pageStep();
          if (x < 0.3) vertical ? next() : prev();
          else if (x > 0.7) vertical ? prev() : next();
        }
      });
      doc.addEventListener("keydown", ctx.keydown);
      doc.addEventListener("scroll", () => !loading && prefs.mode === "scroll" && onMove(), { passive: true });
      // 捲動模式: 到底再往下 → 下一章
      doc.addEventListener(
        "wheel",
        (e) => {
          if (prefs.mode !== "scroll" || loading) return;
          const s = scroller();
          const atEnd = vertical ? -s.scrollLeft >= s.scrollWidth - s.clientWidth - 2 : s.scrollTop >= s.scrollHeight - s.clientHeight - 2;
          const atStart = vertical ? s.scrollLeft >= -2 : s.scrollTop <= 2;
          if (e.deltaY > 30 && atEnd) next();
          else if (e.deltaY < -30 && atStart) prev();
        },
        { passive: true },
      );
      // 手機: 左右滑動翻頁
      let sx = 0;
      let sy = 0;
      doc.addEventListener("touchstart", (e) => ((sx = e.touches[0].clientX), (sy = e.touches[0].clientY)), { passive: true });
      doc.addEventListener(
        "touchend",
        (e) => {
          if (prefs.mode !== "paged") return;
          const dx = e.changedTouches[0].clientX - sx;
          const dy = e.changedTouches[0].clientY - sy;
          if (Math.abs(dx) > 50 && Math.abs(dx) > Math.abs(dy)) {
            // 往左滑 = 下一頁(直排相反)
            if (dx < 0) vertical ? prev() : next();
            else vertical ? next() : prev();
          }
        },
        { passive: true },
      );
    }

    function next() {
      if (loading) return;
      if (prefs.mode === "paged" && page < pages - 1) return showPage(page + 1);
      if (prefs.mode === "scroll") {
        const s = scroller();
        const atEnd = vertical ? -s.scrollLeft >= s.scrollWidth - s.clientWidth - 2 : s.scrollTop >= s.scrollHeight - s.clientHeight - 2;
        if (!atEnd) {
          if (vertical) s.scrollBy({ left: -s.clientWidth * 0.9, behavior: "smooth" });
          else s.scrollBy({ top: s.clientHeight * 0.9, behavior: "smooth" });
          return;
        }
      }
      if (chapter < n - 1) load(chapter + 1, 0);
    }
    function prev() {
      if (loading) return;
      if (prefs.mode === "paged" && page > 0) return showPage(page - 1);
      if (prefs.mode === "scroll") {
        const s = scroller();
        const atStart = vertical ? s.scrollLeft >= -2 : s.scrollTop <= 2;
        if (!atStart) {
          if (vertical) s.scrollBy({ left: s.clientWidth * 0.9, behavior: "smooth" });
          else s.scrollBy({ top: -s.clientHeight * 0.9, behavior: "smooth" });
          return;
        }
      }
      if (chapter > 0) load(chapter - 1, 1);
    }

    // 視窗大小改變: 重新分頁, 保持位置
    const ro = new ResizeObserver(() => {
      if (!docOf() || loading) return;
      const pos = fraction();
      inject();
      measure();
      setPos(pos);
    });
    ro.observe(stage);

    return {
      go: (i, pos = 0, anchor = null) => load(Math.max(0, Math.min(i, n - 1)), pos, anchor),
      vertical: () => vertical,
      next,
      prev,
      seekPercent(p) {
        const x = p * n;
        const i = Math.min(n - 1, Math.floor(x));
        load(i, x - i);
      },
      position() {
        const pos = fraction();
        return { chapter, pos, percent: Math.min(1, (chapter + pos) / n), label: chapterTitles[chapter] || `第 ${chapter + 1} 章` };
      },
      apply(p) {
        const pos = fraction();
        prefs = p;
        inject();
        measure();
        setPos(pos);
      },
      stop() {
        ro.disconnect();
      },
    };
  }

  // ======================================================================
  // 漫畫: 一頁一張圖
  // ======================================================================
  function ComicView(ctx) {
    const { d, stage, onMove } = ctx;
    let prefs = ctx.prefs;
    const n = d.pages;
    let page = 0;
    stage.classList.add("bk-comic");
    const url = (i) => `/books/page/${encodeURIComponent(d.file)}/${i}`;
    const img = h("img", "bk-comic-img");
    img.alt = "";
    const strip = h("div", "bk-comic-strip");
    stage.append(img, strip);

    function show(i) {
      page = Math.max(0, Math.min(i, n - 1));
      if (prefs.mode === "paged") {
        img.src = url(page);
        // 預先載入下一頁
        if (page + 1 < n) new Image().src = url(page + 1);
      } else {
        strip.children[page]?.scrollIntoView({ block: "start" });
      }
      onMove();
    }
    function layout() {
      const paged = prefs.mode === "paged";
      img.hidden = !paged;
      strip.hidden = paged;
      if (!paged && !strip.children.length) {
        for (let i = 0; i < n; i++) {
          const im = h("img");
          im.loading = "lazy";
          im.alt = "";
          im.src = url(i);
          im.dataset.i = i;
          strip.append(im);
        }
        strip.addEventListener("scroll", () => {
          const top = strip.scrollTop;
          let cur = 0;
          for (const im of strip.children) if (im.offsetTop - 10 <= top) cur = +im.dataset.i;
          if (cur !== page) {
            page = cur;
            onMove();
          }
        });
      }
    }
    stage.addEventListener("click", (e) => {
      if (prefs.mode !== "paged") return;
      const x = e.offsetX / stage.clientWidth;
      if (x < 0.35) show(page - 1);
      else if (x > 0.65) show(page + 1);
    });
    let sx = 0;
    stage.addEventListener("touchstart", (e) => (sx = e.touches[0].clientX), { passive: true });
    stage.addEventListener("touchend", (e) => {
      if (prefs.mode !== "paged") return;
      const dx = e.changedTouches[0].clientX - sx;
      if (Math.abs(dx) > 50) show(page + (dx < 0 ? 1 : -1));
    });
    layout();
    return {
      go: (i) => show(i),
      next: () => show(page + 1),
      prev: () => show(page - 1),
      seekPercent: (p) => show(Math.round(p * (n - 1))),
      position: () => ({ chapter: page, pos: 0, percent: n > 1 ? page / (n - 1) : 1, label: `第 ${page + 1} / ${n} 頁` }),
      apply(p) {
        prefs = p;
        layout();
        show(page);
      },
      stop() {},
    };
  }

  // ======================================================================
  // PDF: pdf.js 畫在 canvas 上
  // ======================================================================
  let pdfjsLib = null;
  function loadPdfjs() {
    if (!pdfjsLib) {
      pdfjsLib = import("/pdfjs/pdf.min.mjs").then((m) => {
        m.GlobalWorkerOptions.workerSrc = "/pdfjs/pdf.worker.min.mjs";
        return m;
      });
      pdfjsLib.catch(() => (pdfjsLib = null));
    }
    return pdfjsLib;
  }

  /** PDF 書籤 → 目錄 [{title, chapter(頁), level}] */
  async function pdfOutline(pdf) {
    const out = [];
    const walk = async (items, level) => {
      for (const it of items || []) {
        if (out.length >= 3000) return;
        let page = -1;
        try {
          let dest = it.dest;
          if (typeof dest === "string") dest = await pdf.getDestination(dest);
          if (Array.isArray(dest) && dest[0] != null) {
            page = typeof dest[0] === "number" ? dest[0] : await pdf.getPageIndex(dest[0]);
          }
        } catch {
          /* 壞掉的書籤略過 */
        }
        if (page >= 0) out.push({ title: (it.title || "").trim(), chapter: page, level });
        await walk(it.items, level + 1);
      }
    };
    await walk(await pdf.getOutline(), 0);
    return out;
  }

  function PdfView(ctx) {
    const { d, stage, onMove } = ctx;
    let prefs = ctx.prefs;
    let pdf = null;
    let n = 0;
    let page = 0;
    let pos = 0; // 捲動模式: 在這一頁的哪裡
    let target = null; // 還沒載入完成前要去的位置
    let stopped = false;
    stage.classList.add("bk-pdf");
    const paper = h("div", "bk-pdf-paper");
    const canvas = h("canvas");
    paper.append(canvas);
    const strip = h("div", "bk-pdf-strip");
    const status = h("div", "bk-empty", "PDF 載入中…");
    stage.append(paper, strip, status);
    const dpr = () => Math.min(window.devicePixelRatio || 1, 3);

    // ---- 翻頁: 一次一頁, 整頁放進畫面 ----
    let task = null;
    let drawSeq = 0;
    async function drawPaged() {
      if (!pdf) return;
      const seq = ++drawSeq;
      const p = await pdf.getPage(page + 1);
      if (seq !== drawSeq || stopped) return;
      const pad = ctx.mobile ? 6 : 16;
      const base = p.getViewport({ scale: 1 });
      const scale = Math.min((stage.clientWidth - pad * 2) / base.width, (stage.clientHeight - pad * 2) / base.height);
      const vp = p.getViewport({ scale: scale * dpr() });
      task?.cancel();
      // 先畫在暫存的 canvas, 畫完再換上, 翻頁時不會閃白
      const off = document.createElement("canvas");
      off.width = Math.floor(vp.width);
      off.height = Math.floor(vp.height);
      task = p.render({ canvas: off, viewport: vp });
      try {
        await task.promise;
      } catch {
        return; // 被下一次翻頁取消
      }
      if (seq !== drawSeq || stopped) return;
      canvas.width = off.width;
      canvas.height = off.height;
      canvas.getContext("2d").drawImage(off, 0, 0);
      canvas.style.width = `${Math.floor(base.width * scale)}px`;
      canvas.style.height = `${Math.floor(base.height * scale)}px`;
      // 預先載入下一頁的資料
      if (page + 1 < n) pdf.getPage(page + 2).catch(() => {});
    }

    // ---- 捲動: 每頁一個格子, 看得到的才畫 ----
    let io = null;
    let ratio = 1.414; // 第一頁的高寬比, 還沒量到的頁先用這個
    const drawn = new Map(); // 頁 → render task
    function buildStrip() {
      strip.replaceChildren();
      io?.disconnect();
      drawn.forEach((t) => t?.cancel());
      drawn.clear();
      const w = stripWidth();
      for (let i = 0; i < n; i++) {
        const box = h("div", "bk-pdf-box");
        box.dataset.i = i;
        box.style.width = `${w}px`;
        box.style.height = `${Math.round(w * ratio)}px`;
        strip.append(box);
      }
      io = new IntersectionObserver(
        (entries) => {
          for (const e of entries) {
            const i = +e.target.dataset.i;
            if (e.isIntersecting) drawBox(i, e.target);
            else if (drawn.has(i)) {
              // 離開畫面很遠的頁面釋放記憶體
              drawn.get(i)?.cancel();
              drawn.delete(i);
              e.target.replaceChildren();
            }
          }
        },
        { root: strip, rootMargin: "150% 0px" }
      );
      for (const b of strip.children) io.observe(b);
    }
    function stripWidth() {
      const pad = ctx.mobile ? 0 : 32;
      return Math.min(stage.clientWidth - pad, ctx.mobile ? 4000 : 1100);
    }
    async function drawBox(i, box) {
      if (drawn.has(i)) return;
      drawn.set(i, null);
      const p = await pdf.getPage(i + 1);
      if (!drawn.has(i) || stopped) return;
      const base = p.getViewport({ scale: 1 });
      const w = stripWidth();
      const scale = w / base.width;
      const hgt = Math.round(base.height * scale);
      if (Math.abs(box.offsetHeight - hgt) > 1) {
        // 這一頁的大小和預設不同: 調整格子, 並保持目前看到的位置
        const above = box.offsetTop < strip.scrollTop;
        const diff = hgt - box.offsetHeight;
        box.style.height = `${hgt}px`;
        if (above) strip.scrollTop += diff;
      }
      const vp = p.getViewport({ scale: scale * dpr() });
      const c = h("canvas");
      c.width = Math.floor(vp.width);
      c.height = Math.floor(vp.height);
      const t = p.render({ canvas: c, viewport: vp });
      drawn.set(i, t);
      try {
        await t.promise;
      } catch {
        return;
      }
      if (drawn.get(i) === t) box.replaceChildren(c);
    }
    function scrollTo(i, f = 0) {
      const box = strip.children[i];
      if (!box) return;
      strip.scrollTop = box.offsetTop + f * box.offsetHeight - 8;
    }
    strip.addEventListener(
      "scroll",
      () => {
        if (prefs.mode !== "scroll" || !n) return;
        const top = strip.scrollTop + 8;
        let cur = page;
        // 找出畫面頂端是哪一頁(格子高度差不多, 先猜再修正)
        const boxes = strip.children;
        while (cur > 0 && boxes[cur].offsetTop > top) cur--;
        while (cur + 1 < n && boxes[cur + 1].offsetTop <= top) cur++;
        page = cur;
        pos = Math.max(0, Math.min(1, (top - boxes[cur].offsetTop) / boxes[cur].offsetHeight));
        onMove();
      },
      { passive: true }
    );

    function layout() {
      const paged = prefs.mode === "paged";
      paper.hidden = !paged;
      strip.hidden = paged;
      if (!pdf) return;
      if (paged) drawPaged();
      else {
        buildStrip();
        scrollTo(page, pos);
      }
    }

    function show(i, f = 0) {
      if (!pdf) {
        target = [i, f];
        return;
      }
      page = Math.max(0, Math.min(i, n - 1));
      pos = f;
      if (prefs.mode === "paged") drawPaged();
      else scrollTo(page, f);
      onMove();
    }

    // ---- 翻頁操作: 點左右兩側 / 滑動 ----
    paper.addEventListener("click", (e) => {
      const r = stage.getBoundingClientRect();
      const x = (e.clientX - r.left) / r.width;
      ctx.closePanel();
      if (x < 0.35) show(page - 1);
      else if (x > 0.65) show(page + 1);
    });
    let sx = 0;
    paper.addEventListener("touchstart", (e) => (sx = e.touches[0].clientX), { passive: true });
    paper.addEventListener("touchend", (e) => {
      const dx = e.changedTouches[0].clientX - sx;
      if (Math.abs(dx) > 50) show(page + (dx < 0 ? 1 : -1));
    });
    paper.addEventListener(
      "wheel",
      (e) => {
        if (Math.abs(e.deltaY) < 30) return;
        e.preventDefault();
        show(page + (e.deltaY > 0 ? 1 : -1));
      },
      { passive: false }
    );

    let resizeTimer = null;
    const ro = new ResizeObserver(() => {
      clearTimeout(resizeTimer);
      resizeTimer = setTimeout(() => {
        if (!pdf) return;
        if (prefs.mode === "paged") drawPaged();
        else {
          const [p0, f0] = [page, pos];
          buildStrip();
          scrollTo(p0, f0);
        }
      }, 150);
    });
    ro.observe(stage);

    layout();
    loadPdfjs()
      .then((lib) =>
        lib.getDocument({
          url: `/books/file/${encodeURIComponent(d.file)}`,
          cMapUrl: "/pdfjs/cmaps/",
          cMapPacked: true,
          standardFontDataUrl: "/pdfjs/standard_fonts/",
          wasmUrl: "/pdfjs/wasm/",
          isEvalSupported: false,
        }).promise
      )
      .then(async (doc) => {
        if (stopped) return doc.destroy();
        pdf = doc;
        n = doc.numPages;
        d.pages = n;
        const first = await doc.getPage(1);
        const vp = first.getViewport({ scale: 1 });
        ratio = vp.height / vp.width;
        status.remove();
        layout();
        const [i, f] = target || [0, 0];
        target = null;
        show(i, f);
        pdfOutline(doc)
          .then((t) => (d.pdfToc = t))
          .catch(() => {});
      })
      .catch((e) => {
        status.textContent = `無法開啟 PDF: ${e?.message || e}`;
      });

    return {
      go: (i, f = 0) => show(i, f),
      next: () => (prefs.mode === "paged" ? show(page + 1) : strip.scrollBy({ top: strip.clientHeight * 0.9, behavior: "smooth" })),
      prev: () => (prefs.mode === "paged" ? show(page - 1) : strip.scrollBy({ top: -strip.clientHeight * 0.9, behavior: "smooth" })),
      seekPercent: (p) => {
        const x = p * n;
        const i = Math.min(n - 1, Math.floor(x));
        show(i, prefs.mode === "paged" ? 0 : x - i);
      },
      position() {
        if (!n) return { chapter: page, pos, percent: 0, label: "" };
        const percent = prefs.mode === "paged" ? (n > 1 ? page / (n - 1) : 1) : Math.min(1, (page + pos) / n);
        return { chapter: page, pos: prefs.mode === "paged" ? 0 : pos, percent, label: `第 ${page + 1} / ${n} 頁` };
      },
      apply(p) {
        prefs = p;
        layout();
      },
      stop() {
        stopped = true;
        ro.disconnect();
        io?.disconnect();
        task?.cancel();
        drawn.forEach((t) => t?.cancel());
        pdf?.destroy();
      },
    };
  }

  return { mount };
})();

if (typeof Apps !== "undefined") Apps.registerView("books", BooksApp);

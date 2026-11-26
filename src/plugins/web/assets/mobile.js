// cng6 手機版: 底部三個分頁(總覽 / 指令 / 設定), 實用優先
//
// - 總覽: 各 plugin 的狀態卡片(apps.js), 有自訂畫面的可以點進去
// - 指令: 點選式組出指令(不用打字), 也可以直接輸入
// - 設定: iOS 設定風格的列表, 外觀與桌布也在這裡
"use strict";

(() => {
  const $ = (sel, root = document) => root.querySelector(sel);
  function h(tag, cls, text) {
    const el = document.createElement(tag);
    if (cls) el.className = cls;
    if (text != null) el.textContent = text;
    return el;
  }
  // 和 src/parse.rs 的 quote_arg 相同
  function quoteArg(a) {
    if (a !== "" && !/[\s"'\\]/.test(a)) return a;
    return `"${a.replace(/\\/g, "\\\\").replace(/"/g, '\\"')}"`;
  }
  const chevron = () => {
    const s = h("span", "m-chev");
    s.innerHTML = `<svg viewBox="0 0 24 24" aria-hidden="true"><path d="M9 5l7 7-7 7"/></svg>`;
    return s;
  };

  // ---------------------------------------------------------- 提示訊息
  const toastEl = $(".m-toast");
  let toastTimer = null;
  function toast(text, ok = true) {
    toastEl.textContent = text;
    toastEl.dataset.ok = ok;
    toastEl.classList.add("show");
    clearTimeout(toastTimer);
    toastTimer = setTimeout(() => toastEl.classList.remove("show"), 2600);
  }

  // ---------------------------------------------------------- 連線狀態
  const live = $(".m-live");
  function setLive(ok) {
    live.dataset.state = ok ? "on" : "off";
    $(".txt", live).textContent = ok ? "即時" : "離線";
  }

  // ---------------------------------------------------------- 分頁與頁面堆疊
  // 每個分頁有自己的堆疊, 切換分頁時保留狀態(例如輸入到一半的指令)
  const main = $(".m-main");
  // 每個底部分頁一個堆疊(分頁可以設定, 見下面的 setTabs)
  const stacks = { overview: [] };
  let tab = "overview";
  const backBtn = $(".m-back");
  const titleEl = $(".m-title");

  function top(t = tab) {
    return stacks[t][stacks[t].length - 1];
  }

  function show() {
    for (const [t, stack] of Object.entries(stacks)) {
      stack.forEach((p, i) => (p.el.hidden = !(t === tab && i === stack.length - 1)));
    }
    const p = top();
    titleEl.textContent = p.title;
    const prev = stacks[tab][stacks[tab].length - 2];
    backBtn.hidden = !prev;
    // 感應區: 除了總覽的第一頁, 都可以往回(App 裡的上一層 → 上一頁 → 總覽)
    edge.hidden = !prev && tab === "overview";
    if (prev) $("span", backBtn).textContent = prev.title;
    document.querySelectorAll(".m-tabs button").forEach((b) => b.setAttribute("aria-selected", String(b.dataset.tab === tab)));
    p.onShow && p.onShow();
  }

  /** page: { title, el, onShow?, destroy? } */
  function push(t, page) {
    page.el.classList.add("m-page");
    main.appendChild(page.el);
    stacks[t].push(page);
    if (t === tab) {
      show();
      main.scrollTop = 0;
    }
  }

  function pop() {
    if (stacks[tab].length < 2) return;
    const p = stacks[tab].pop();
    p.destroy && p.destroy();
    p.el.remove();
    show();
  }

  backBtn.addEventListener("click", pop);

  // ---- 從左邊緣往右滑: 回上一層 ----
  // App 裡面有「可以邊緣返回」的按鈕(data-edge-back, 正在顯示)就按它(例如郵件從信回到列表),
  // 沒有就回上一頁。data-edge-el 指定拖動時要移動的元素(預設整頁)。
  // 左邊一條透明的感應區接手勢: 內容可能是 iframe(信件), 觸控事件不會傳出來。
  const edge = h("div", "m-edge");
  edge.setAttribute("aria-hidden", "true");
  edge.hidden = true;
  $("#app").append(edge);

  function innerBack() {
    const p = top();
    if (!p) return null;
    const list = [...p.el.querySelectorAll("[data-edge-back]")].filter((b) => !b.disabled && b.offsetParent !== null);
    return list.pop() || null;
  }

  let g = null;
  const resetEl = (el) => {
    el.style.transition = "";
    el.style.transform = "";
    el.style.opacity = "";
  };
  edge.addEventListener("pointerdown", (e) => {
    g = { id: e.pointerId, sx: e.clientX, sy: e.clientY, dx: 0, active: false };
    try {
      edge.setPointerCapture(e.pointerId);
    } catch {}
  });
  edge.addEventListener("pointermove", (e) => {
    if (!g || e.pointerId !== g.id) return;
    const x = e.clientX - g.sx;
    const y = e.clientY - g.sy;
    if (!g.active) {
      if (x > 8 && x > Math.abs(y)) {
        g.btn = innerBack();
        // 沒有上一層也沒有上一頁: 其他分頁的第一頁回到總覽, 總覽的第一頁不動
        if (!g.btn && stacks[tab].length < 2 && tab === "overview") {
          g = null;
          return;
        }
        g.active = true;
        const p = top();
        g.el = (g.btn && g.btn.dataset.edgeEl && p.el.querySelector(g.btn.dataset.edgeEl)) || p.el;
        g.el.dispatchEvent(new CustomEvent("edgeback-start", { bubbles: true }));
      } else if (Math.abs(y) > 12 || x < -8) {
        g = null;
        return;
      } else return;
    }
    g.dx = Math.max(0, x);
    g.el.style.transform = `translateX(${g.dx}px)`;
    // 整頁拖動時漸漸淡出(App 自己的層會露出底下的內容, 不用淡出)
    if (g.el === top().el) g.el.style.opacity = String(1 - (g.dx / g.el.offsetWidth) * 0.6);
  });
  edge.addEventListener("pointerup", (e) => {
    if (!g || e.pointerId !== g.id) return;
    const cur = g;
    g = null;
    if (!cur.active) return;
    const el = cur.el;
    const done = () => el.dispatchEvent(new CustomEvent("edgeback-end", { bubbles: true }));
    if (cur.dx > el.offsetWidth * 0.3) {
      el.style.transition = "transform .18s, opacity .18s";
      el.style.transform = "translateX(100%)";
      setTimeout(() => {
        resetEl(el);
        done();
        if (cur.btn) cur.btn.click();
        else if (stacks[tab].length >= 2) pop();
        else selectTab("overview");
      }, 180);
    } else {
      el.style.transition = "transform .2s, opacity .2s";
      el.style.transform = "";
      el.style.opacity = "";
      setTimeout(() => {
        resetEl(el);
        done();
      }, 200);
    }
  });
  edge.addEventListener("pointercancel", (e) => {
    if (!g || e.pointerId !== g.id) return;
    if (g.active) {
      resetEl(g.el);
      g.el.dispatchEvent(new CustomEvent("edgeback-end", { bubbles: true }));
    }
    g = null;
  });
  // ---- 底部分頁: web.mobile_tabs 設定(最多 5 個, 一定有總覽) ----
  const TAB_SVG = {
    overview: `<rect x="3" y="3" width="8" height="8" rx="2"/><rect x="13" y="3" width="8" height="8" rx="2"/><rect x="3" y="13" width="8" height="8" rx="2"/><rect x="13" y="13" width="8" height="8" rx="2"/>`,
    cli: `<path d="M5 7l5 5-5 5"/><path d="M12 18h7"/>`,
    settings: `<circle cx="12" cy="12" r="3"/><path d="M12 2v3M12 19v3M4.9 4.9l2.1 2.1M17 17l2.1 2.1M2 12h3M19 12h3M4.9 19.1L7 17M17 7l2.1-2.1"/>`,
  };
  const CORE = { overview: "總覽", cli: "指令", settings: "設定" };
  const ALIAS = { 總覽: "overview", 指令: "cli", 設定: "settings" };
  // Apps 還沒載入時用的名稱與圖示
  const FALLBACK = { music: ["音樂", "music"], weather: ["天氣", "weather"], books: ["書", "book"], mail: ["郵件", "mail"], files: ["檔案", "folder"] };
  const DEFAULT_TABS = "overview,music,weather,books,mail";
  const tabsEl = $(".m-tabs");
  let tabIds = [];
  let tabsResolved = false;
  let tabValue = DEFAULT_TABS;

  const hasPage = (a) => a && (a.card || (a.view && Apps.views[a.view]));

  function resolveTab(tok) {
    tok = tok.trim();
    if (!tok) return null;
    if (CORE[tok]) return tok;
    if (ALIAS[tok]) return ALIAS[tok];
    if (!Apps.list.length) return tok; // 還不知道有哪些 App: 先相信設定
    const a = Apps.list.find((x) => x.name === tok || x.title === tok);
    return hasPage(a) ? a.name : null;
  }

  function tabInfo(id) {
    if (CORE[id]) return { title: CORE[id], svg: TAB_SVG[id] };
    const a = Apps.find(id);
    const fb = FALLBACK[id];
    return { title: a?.title || fb?.[0] || id, svg: Apps.ICONS[a?.icon || fb?.[1]] || Apps.ICONS.other };
  }

  function setTabs(value, force = false) {
    tabValue = value || DEFAULT_TABS;
    let ids = [...new Set(String(tabValue).split(/[,，、]/).map(resolveTab).filter(Boolean))];
    if (!ids.includes("overview")) ids.unshift("overview");
    ids = ids.slice(0, 5);
    if (!force && JSON.stringify(ids) === JSON.stringify(tabIds)) return;
    // 拿掉的分頁: 收掉它的頁面
    for (const id of Object.keys(stacks)) {
      if (id === "overview" || ids.includes(id)) continue;
      for (const p of stacks[id]) {
        p.destroy && p.destroy();
        p.el.remove();
      }
      delete stacks[id];
    }
    tabIds = ids;
    tabsEl.replaceChildren(
      ...ids.map((id) => {
        const info = tabInfo(id);
        const b = document.createElement("button");
        b.setAttribute("role", "tab");
        b.dataset.tab = id;
        b.innerHTML = `<svg viewBox="0 0 24 24">${info.svg}</svg><span></span><i class="m-tab-badge" hidden></i>`;
        b.querySelector("span").textContent = info.title;
        b.addEventListener("click", () => selectTab(id));
        return b;
      }),
    );
    if (!stacks[tab]) tab = "overview";
    show();
    updateBadges();
  }

  /** 分頁的第一頁 */
  function rootPage(id) {
    if (id === "overview") return overviewPage();
    if (id === "cli") return cliPage();
    if (id === "settings") return settingsPage();
    return detailPage(Apps.find(id));
  }

  function selectTab(id) {
    // 再點一次目前的分頁: 回到第一頁
    if (id === tab && stacks[tab]) {
      while (stacks[tab].length > 1) pop();
      return show();
    }
    // 第一次打開才建立(音樂、電子書這些畫面比較重)
    if (!stacks[id]) {
      if (!CORE[id] && !hasPage(Apps.find(id))) return;
      stacks[id] = [];
      tab = id;
      push(id, rootPage(id));
      return;
    }
    tab = id;
    show();
  }

  // 郵件分頁的未讀數
  async function updateBadges() {
    const b = tabsEl.querySelector('[data-tab="mail"] .m-tab-badge');
    if (!b) return;
    try {
      const r = await fetch("/api/mail", { cache: "no-store" });
      const d = await r.json();
      const n = (d.accounts || []).reduce((sum, a) => sum + (a.unread || 0), 0);
      b.hidden = !n;
      b.textContent = n > 99 ? "99+" : String(n);
    } catch {}
  }
  setInterval(() => !document.hidden && updateBadges(), 30000);

  /** 一個 plugin 的畫面: 自訂畫面(view), 或通用的狀態卡片 */
  function detailPage(app) {
    const view = app.view && Apps.views[app.view];
    if (view) {
      const el = h("div", "m-view");
      const content = h("div", "m-view-content");
      el.appendChild(content);
      const stop = view.mount(content, { device: "mobile" });
      return { title: app.title, el, destroy: () => typeof stop === "function" && stop() };
    }
    return cardPage(app.name, app.title);
  }

  // 讓其他模組(例如音樂的迷你播放列、新郵件通知)打開某個 plugin 的畫面:
  // 有自己的分頁就切過去, 沒有就從總覽打開
  window.MobileNav = {
    openApp(name) {
      if (tabIds.includes(name)) {
        if (stacks[name]) {
          tab = name;
          while (stacks[tab].length > 1) pop();
          show();
        } else selectTab(name);
        return;
      }
      const app = Apps.find(name);
      if (!hasPage(app)) return;
      tab = "overview";
      while (stacks.overview.length > 1) pop();
      show();
      push("overview", detailPage(app));
    },
  };

  // ====================================================================
  // 總覽
  // ====================================================================
  const STATUS_ICON = { warning: "▲", critical: "●", off: "○", good: "●" };
  const RANK = { critical: 0, warning: 1 };

  function overviewPage() {
    const el = h("div", "m-overview");
    const list = h("div", "m-list");
    const foot = h("div", "m-foot");
    el.append(list, foot);
    let timer = null;

    async function refresh() {
      try {
        await Apps.load();
        setLive(true);
      } catch {
        setLive(false);
        return;
      }
      render();
    }

    // 清單: 每個項目一列(名稱 + 摘要 + 狀態), 點了才進到細節
    function render() {
      const apps = Apps.list.filter((a) => a.card || (a.view && Apps.views[a.view]));
      const sorted = apps
        .map((a, i) => ({ a, i }))
        .sort((x, y) => (RANK[x.a.card?.status] ?? 9) - (RANK[y.a.card?.status] ?? 9) || x.i - y.i)
        .map((x) => x.a);
      list.replaceChildren(...sorted.map(row));
      if (!sorted.length) list.appendChild(h("div", "m-empty", "還沒有任何 plugin 提供狀態"));
      // 指令、設定: 不在底部分頁時放在這裡
      const core = [
        ["cli", "cli", "dark", "選單與指令輸入"],
        ["settings", "settings", "gray", "系統設定與外觀"],
      ].filter(([id]) => !tabIds.includes(id));
      for (const [id, icon, color, desc] of core) {
        const r = h("button", "m-row nav m-app-row");
        const t = h("div", "m-row-text");
        t.append(h("div", "m-row-name ui", CORE[id]), h("div", "m-row-desc", desc));
        r.append(Apps.icon(icon, color, "m-icon"), t, chevron());
        r.addEventListener("click", () => push("overview", rootPage(id)));
        list.appendChild(r);
      }
      // App 載入後名稱與圖示才確定
      if (!tabsResolved) {
        tabsResolved = true;
        setTabs(tabValue, true);
      }
      foot.textContent = `更新於 ${new Date().toLocaleTimeString("zh-TW", { hour12: false })}`;
    }

    function row(app) {
      const r = h("button", "m-row nav m-app-row");
      const st = app.card?.status;
      if (st) r.dataset.status = st;
      const t = h("div", "m-row-text");
      t.append(h("div", "m-row-name ui", app.title), h("div", "m-row-desc", app.card?.summary || app.description));
      r.append(Apps.icon(app.icon, app.color, "m-icon"), t);
      if (st === "warning" || st === "critical") r.append(h("span", "m-pill", `${STATUS_ICON[st]} ${Apps.STATUS_TEXT[st]}`));
      r.append(chevron());
      r.addEventListener("click", () => MobileNav.openApp(app.name));
      return r;
    }

    return {
      title: "總覽",
      el,
      onShow() {
        clearInterval(timer);
        refresh();
        timer = setInterval(() => {
          if (!document.hidden && tab === "overview" && stacks.overview.length === 1) refresh();
        }, 3000);
      },
    };
  }

  // 一個 plugin 的細節頁: 狀態卡片的內容(按鈕、開關…), 定時更新
  function cardPage(name, title) {
    const el = h("div", "m-detail");
    const head = h("div", "m-detail-head");
    const body = h("div", "cw-root m-cw m-detail-body");
    el.append(head, body);
    let busy = false;
    let timer = null;

    async function refresh() {
      if (busy) return;
      try {
        await Apps.load();
        setLive(true);
      } catch {
        setLive(false);
        return;
      }
      const app = Apps.find(name);
      if (!app || !app.card) {
        body.replaceChildren(h("div", "m-empty", "沒有資料"));
        return;
      }
      head.replaceChildren(Apps.icon(app.icon, app.color, "m-icon lg"));
      const text = h("div", "m-detail-text");
      text.append(h("div", "m-detail-title", app.title), h("div", "m-detail-sub", app.card.summary || app.description));
      head.appendChild(text);
      const st = app.card.status;
      if (st && st !== "good") head.appendChild(h("span", "m-pill", `${STATUS_ICON[st]} ${Apps.STATUS_TEXT[st]}`));
      head.dataset.status = st || "";
      Apps.renderWidgets(body, app.card, {
        onAction: async (p) => {
          busy = true;
          const r = await p;
          busy = false;
          if (r.text) toast(r.text, r.ok);
          refresh();
        },
      });
    }

    refresh();
    timer = setInterval(() => {
      if (!document.hidden && top("overview")?.el === el) refresh();
    }, 3000);
    return { title, el, destroy: () => clearInterval(timer) };
  }

  // ====================================================================
  // 指令
  // ====================================================================
  function cliPage() {
    const el = h("div", "m-cli");
    el.innerHTML = `
      <div class="m-seg" role="tablist">
        <button data-v="menu" class="on">選單</button>
        <button data-v="out">輸出<span class="m-badge" hidden></span></button>
      </div>
      <div class="m-cli-menu">
        <div class="m-crumb"></div>
        <div class="m-recent"></div>
        <div class="m-list m-cands"></div>
      </div>
      <div class="m-cli-out" hidden></div>
      <form class="m-composer" autocomplete="off">
        <span class="m-prompt"></span>
        <input class="m-input" enterkeyhint="go" spellcheck="false" autocapitalize="off" autocorrect="off" placeholder="點選上面的指令, 或直接輸入">
        <button type="button" class="m-clear" aria-label="清除" hidden>×</button>
        <button type="submit" class="m-run">執行</button>
      </form>`;
    const menu = $(".m-cli-menu", el);
    const out = $(".m-cli-out", el);
    const crumb = $(".m-crumb", el);
    const recent = $(".m-recent", el);
    const cands = $(".m-cands", el);
    const input = $(".m-input", el);
    const clearBtn = $(".m-clear", el);
    const runBtn = $(".m-run", el);
    const badge = $(".m-badge", el);
    const promptEl = $(".m-prompt", el);
    let view = "menu";
    let unread = 0;

    function setView(v) {
      view = v;
      el.querySelectorAll(".m-seg button").forEach((b) => b.classList.toggle("on", b.dataset.v === v));
      menu.hidden = v !== "menu";
      out.hidden = v !== "out";
      if (v === "out") {
        unread = 0;
        badge.hidden = true;
        out.scrollTop = out.scrollHeight;
      }
    }
    el.querySelectorAll(".m-seg button").forEach((b) => b.addEventListener("click", () => setView(b.dataset.v)));

    // ---- 目前層次 ----
    function renderCrumb() {
      promptEl.textContent = Shell.prompt();
      crumb.replaceChildren();
      const path = Shell.path;
      const home = h("button", "m-crumb-item", "最上層");
      home.addEventListener("click", () => Shell.exec("/", { history: false }).then(refreshCands));
      crumb.appendChild(home);
      path.forEach((p, i) => {
        crumb.append(h("span", "m-crumb-sep", "›"));
        const b = h("button", "m-crumb-item", p);
        b.addEventListener("click", () => Shell.exec(`/${path.slice(0, i + 1).join(" ")}`, { history: false }).then(refreshCands));
        crumb.appendChild(b);
      });
    }

    // ---- 最近使用 ----
    function renderRecent() {
      recent.replaceChildren();
      const seen = new Set();
      const items = [];
      for (let i = Shell.history.length - 1; i >= 0 && items.length < 8; i--) {
        const l = Shell.history[i].trim();
        if (l && !seen.has(l)) {
          seen.add(l);
          items.push(l);
        }
      }
      if (!items.length) return;
      recent.appendChild(h("div", "m-section-title", "最近使用"));
      const row = h("div", "m-chips");
      for (const l of items) {
        const c = h("button", "m-chip", l);
        c.addEventListener("click", () => run(l));
        row.appendChild(c);
      }
      recent.appendChild(row);
    }

    // ---- 候選(點選式組指令) ----
    let candSeq = 0;
    async function refreshCands() {
      const seq = ++candSeq;
      const text = input.value;
      const list = await Shell.candidates(text);
      if (seq !== candSeq) return;
      const current = text.endsWith(" ") || !text ? "" : text.split(/\s+/).pop();
      const head = text.slice(0, text.length - current.length);
      cands.replaceChildren();
      recent.hidden = !!text;
      clearBtn.hidden = !text;
      runBtn.classList.toggle("ready", !!text.trim() && !list.some((c) => c.placeholder || c.more) ? true : false);
      if (!list.length) {
        cands.appendChild(h("div", "m-empty", text.trim() ? "沒有其他選項, 按「執行」" : "沒有可用的指令"));
        return;
      }
      if (text) cands.appendChild(h("div", "m-section-title", "接下來可以選"));
      for (const c of list) {
        const row = h("button", `m-row ${c.placeholder ? "param" : ""}`);
        const t = h("div", "m-row-text");
        t.append(h("div", "m-row-name", c.name), h("div", "m-row-desc", c.desc || ""));
        row.appendChild(t);
        if (c.placeholder) row.appendChild(h("span", "m-tag", "輸入"));
        else if (c.more) row.appendChild(chevron());
        row.addEventListener("click", () => {
          if (c.placeholder) {
            // 參數: 叫出鍵盤讓使用者輸入
            input.value = head;
            input.placeholder = `輸入 ${c.name}`;
            input.focus();
            refreshCands();
            return;
          }
          input.value = `${head}${c.name} `;
          refreshCands();
        });
        cands.appendChild(row);
      }
    }

    let candTimer = null;
    input.addEventListener("input", () => {
      clearTimeout(candTimer);
      candTimer = setTimeout(refreshCands, 150);
    });
    clearBtn.addEventListener("click", () => {
      input.value = "";
      input.placeholder = "點選上面的指令, 或直接輸入";
      refreshCands();
    });

    // ---- 執行 ----
    async function run(line) {
      if (!line.trim()) return;
      input.value = "";
      input.placeholder = "點選上面的指令, 或直接輸入";
      input.blur();
      setView("out");
      await Shell.exec(line.trim());
      renderRecent();
      refreshCands();
    }
    $(".m-composer", el).addEventListener("submit", (e) => {
      e.preventDefault();
      run(input.value);
    });

    // ---- 輸出: 每個指令一張卡片 ----
    function renderOut() {
      out.replaceChildren();
      let group = null;
      for (const m of Shell.messages) {
        if (m.level === "command" || !group) {
          group = h("div", "m-out-group");
          out.appendChild(group);
        }
        const line = h("div", `m-out-line lv-${m.level}`);
        if (m.level === "warn" || m.level === "error") line.appendChild(h("span", "m-out-icon", m.level === "warn" ? "▲" : "●"));
        line.appendChild(document.createTextNode(m.text));
        group.appendChild(line);
      }
      if (!Shell.messages.length) out.appendChild(h("div", "m-empty", "還沒有執行任何指令"));
      else {
        const clr = h("button", "m-out-clear", "清除輸出");
        clr.addEventListener("click", () => Shell.clearMessages());
        out.appendChild(clr);
      }
      out.scrollTop = out.scrollHeight;
    }
    Shell.on("message", () => {
      renderOut();
      if (view !== "out") {
        unread++;
        badge.textContent = unread > 99 ? "99+" : String(unread);
        badge.hidden = false;
      }
    });
    Shell.on("clear", renderOut);
    Shell.on("path", () => {
      renderCrumb();
      refreshCands();
    });

    renderCrumb();
    renderRecent();
    renderOut();
    let started = false;

    // 輸入時隱藏底部分頁, 留空間給鍵盤
    input.addEventListener("focus", () => document.body.classList.add("typing"));
    input.addEventListener("blur", () => setTimeout(() => document.body.classList.remove("typing"), 100));

    return {
      title: "指令",
      el,
      onShow() {
        if (!started) {
          started = true;
          Shell.sync().then(refreshCands);
        }
      },
    };
  }

  // ====================================================================
  // 設定
  // ====================================================================
  const groupOf = (key) => key.split(".")[0];
  function kindOf(row) {
    if (row.key.endsWith(".rect")) return "rect";
    if (row.default === "true" || row.default === "false") return "bool";
    if (/^\d+(\.\d+)?$/.test(row.default)) return "number";
    return "text";
  }
  function splitDesc(desc) {
    const m = desc.match(/^(.*?)(?:[,，]\s*|\s+)?\(([^()]*)\)\s*$/);
    return m ? [m[1], m[2]] : [desc, ""];
  }
  function metaOf(g) {
    const a = Apps.find(g);
    return a ? { title: a.title, icon: a.icon, color: a.color } : { title: g, icon: "other", color: "gray" };
  }

  let rows = [];
  const rowListeners = new Set();
  async function loadRows() {
    try {
      const res = await Apps.api("settings", "");
      const changed = JSON.stringify(res.settings) !== JSON.stringify(rows);
      rows = res.settings;
      if (changed) rowListeners.forEach((f) => f());
    } catch {
      /* 下次再試 */
    }
  }
  /** 執行 settings 指令, 成功回傳 null, 失敗回傳錯誤訊息 */
  async function runSetting(line) {
    const r = await Apps.exec(line);
    await loadRows();
    return r.ok ? null : r.text.replace(/^settings:\s*/, "");
  }

  function settingsPage() {
    const el = h("div", "m-settings");
    const render = () => {
      el.replaceChildren();
      // 這個瀏覽器
      el.appendChild(h("div", "m-section-title", "這個瀏覽器"));
      const local = h("div", "m-list");
      local.appendChild(navRow(Apps.icon("appearance", "indigo", "m-icon sm"), "外觀", themeName(), () => push(tab, appearancePage())));
      el.appendChild(local);
      // 系統設定(各 plugin 的分組)
      el.appendChild(h("div", "m-section-title", "系統設定"));
      const sys = h("div", "m-list");
      const order = Apps.list.map((a) => a.name);
      const groups = [...new Set(rows.map((r) => groupOf(r.key)))].sort(
        (a, b) => ((order.indexOf(a) + 1 || 99) - (order.indexOf(b) + 1 || 99)) || a.localeCompare(b),
      );
      for (const g of groups) {
        const m = metaOf(g);
        const changed = rows.filter((r) => groupOf(r.key) === g && r.value !== r.default).length;
        sys.appendChild(navRow(Apps.icon(m.icon, m.color, "m-icon sm"), m.title, changed ? `已修改 ${changed}` : "", () => push(tab, groupPage(g))));
      }
      if (!groups.length) sys.appendChild(h("div", "m-empty", "讀取中…"));
      el.appendChild(sys);
      el.appendChild(h("div", "m-foot", "系統設定會寫入 Pi 上的 settings.conf"));
    };
    rowListeners.add(render);
    let timer = null;
    return {
      title: "設定",
      el,
      onShow() {
        Promise.all([Apps.load().catch(() => {}), loadRows()]).then(render);
        render();
        clearInterval(timer);
        timer = setInterval(() => !document.hidden && !el.hidden && el.isConnected && loadRows(), 4000);
      },
    };
  }

  function navRow(icon, title, value, onClick) {
    const r = h("button", "m-row nav");
    if (icon) r.appendChild(icon);
    const t = h("div", "m-row-text");
    t.appendChild(h("div", "m-row-name ui", title));
    r.append(t);
    if (value) r.appendChild(h("span", "m-row-value", value));
    r.appendChild(chevron());
    r.addEventListener("click", onClick);
    return r;
  }

  // ---- 外觀 ----
  const THEMES = [
    ["system", "系統", "跟著手機的淺色/深色設定"],
    ["light", "淺色", ""],
    ["dark", "深色", ""],
  ];
  function getTheme() {
    try {
      const t = localStorage.getItem("cng6.theme");
      return t === "light" || t === "dark" ? t : "system";
    } catch {
      return "system";
    }
  }
  function setTheme(t) {
    if (t === "light" || t === "dark") document.documentElement.dataset.theme = t;
    else delete document.documentElement.dataset.theme;
    try {
      if (t === "system") localStorage.removeItem("cng6.theme");
      else localStorage.setItem("cng6.theme", t);
    } catch {
      /* 略過 */
    }
    window.dispatchEvent(new CustomEvent("cng6-theme", { detail: t }));
  }
  const themeName = () => THEMES.find((t) => t[0] === getTheme())[1];

  function appearancePage() {
    const el = h("div", "m-settings");
    const render = () => {
      el.replaceChildren();
      const list = h("div", "m-list");
      for (const [id, name, note] of THEMES) {
        const r = h("button", "m-row check");
        r.setAttribute("role", "radio");
        r.setAttribute("aria-checked", String(getTheme() === id));
        const t = h("div", "m-row-text");
        t.append(h("div", "m-row-name ui", name));
        if (note) t.append(h("div", "m-row-desc", note));
        r.append(t, h("span", "m-check", getTheme() === id ? "✓" : ""));
        r.addEventListener("click", () => {
          setTheme(id);
          render();
        });
        list.appendChild(r);
      }
      el.append(list, h("div", "m-foot", "只套用在這個瀏覽器"));
    };
    render();
    return { title: "外觀", el };
  }

  // ---- 一個分組的設定 ----
  function groupPage(g) {
    const el = h("div", "m-settings");
    let editingWp = false;
    const render = () => {
      // 正在輸入時不要重畫
      if (el.contains(document.activeElement) && document.activeElement.tagName === "INPUT") return;
      el.replaceChildren();
      if (g === "wallpaper") el.appendChild(wallpaperSection());
      const list = rows.filter((r) => groupOf(r.key) === g && !(g === "wallpaper" && r.key === "wallpaper.current"));
      el.appendChild(h("div", "m-section-title", "設定"));
      const box = h("div", "m-list");
      for (const row of list) box.appendChild(settingRow(row));
      el.appendChild(box);
      const modified = list.filter((r) => r.value !== r.default);
      if (modified.length) {
        const all = h("button", "m-link", `全部恢復預設 (${modified.length})`);
        all.addEventListener("click", async () => {
          if (!confirm(`把 ${metaOf(g).title} 的 ${modified.length} 項設定恢復預設?`)) return;
          for (const r of modified) await runSetting(`/settings reset ${r.key}`);
          toast("已恢復預設");
        });
        el.appendChild(all);
      }
    };

    // 桌布: 縮圖 + 從相簿上傳 + 編輯(刪除)
    function wallpaperSection() {
      const s = h("div", "m-wp");
      const headRow = h("div", "m-section-head");
      headRow.appendChild(h("div", "m-section-title", "桌布"));
      const edit = h("button", "m-link sm", editingWp ? "完成" : "編輯");
      edit.addEventListener("click", () => {
        editingWp = !editingWp;
        render();
      });
      headRow.appendChild(edit);
      s.appendChild(headRow);
      const grid = h("div", "m-wp-grid");
      s.appendChild(grid);
      const up = h("label", "m-btn");
      up.textContent = "從相簿上傳…";
      const file = h("input");
      file.type = "file";
      file.accept = "image/*";
      file.multiple = true;
      file.hidden = true;
      file.addEventListener("change", async () => {
        const files = [...file.files].filter((f) => /\.(jpe?g|png|webp|gif|avif)$/i.test(f.name));
        file.value = "";
        if (!files.length) return toast("只接受 jpg / png / webp / gif / avif", false);
        toast(`上傳中… (${files.length} 張)`);
        let ok = 0;
        for (const f of files) {
          const r = await fetch(`/api/wallpaper/${encodeURIComponent(f.name)}`, { method: "PUT", body: f }).catch(() => null);
          if (r && r.ok) ok++;
        }
        toast(ok === files.length ? `已上傳 ${ok} 張` : `上傳 ${ok}/${files.length} 張, 有些失敗`, ok === files.length);
        render();
      });
      up.appendChild(file);
      s.appendChild(up);
      s.appendChild(h("div", "m-foot", "iPhone 相簿的 HEIC 照片請先轉成 JPEG(分享時選「最相容」)"));

      fetch("/api/wallpaper", { cache: "no-store" })
        .then((r) => r.json())
        .then((info) => {
          const tile = (name, label, url) => {
            const t = h("button", `m-wp-tile ${info.showing === name ? "on" : ""}`);
            const th = h("span", `m-wp-thumb ${url ? "" : "default"}`);
            if (url) {
              const img = h("img");
              img.loading = "lazy";
              img.alt = "";
              img.src = url;
              th.appendChild(img);
            }
            t.append(th, h("span", "m-wp-name", label));
            if (editingWp && url) {
              t.appendChild(h("span", "m-wp-del", "−"));
              t.addEventListener("click", async () => {
                if (!confirm(`刪除桌布「${name}」?`)) return;
                const r = await fetch(`/api/wallpaper/${encodeURIComponent(name)}`, { method: "DELETE" }).catch(() => null);
                if (!r || !r.ok) return toast("刪除失敗", false);
                if (info.showing === name) await runSetting("/wallpaper set default");
                toast(`已刪除 ${name}`);
                render();
              });
            } else {
              t.addEventListener("click", async () => {
                if (info.showing === name) return;
                const err = await runSetting(`/wallpaper set ${quoteArg(name)}`);
                toast(err || `桌布已換成 ${label}`, !err);
                render();
              });
            }
            return t;
          };
          grid.appendChild(tile("default", "預設", null));
          for (const it of info.list) grid.appendChild(tile(it.name, it.name, it.url));
        })
        .catch(() => grid.appendChild(h("div", "m-empty", "無法讀取桌布")));
      return s;
    }

    rowListeners.add(render);
    render();
    return { title: metaOf(g).title, el, destroy: () => rowListeners.delete(render) };
  }

  function settingRow(row) {
    const kind = kindOf(row);
    const r = h("div", `m-row setting ${kind}`);
    const [title, hint] = splitDesc(row.desc);
    const t = h("div", "m-row-text");
    const name = h("div", "m-row-name ui", title);
    if (row.value !== row.default) name.appendChild(h("span", "m-dot"));
    const sub = h("div", "m-row-desc");
    sub.append(h("code", null, row.key));
    if (hint) sub.append(document.createTextNode(` · ${hint}`));
    const err = h("div", "m-err");
    err.hidden = true;
    t.append(name, sub, err);
    r.appendChild(t);

    async function apply(value) {
      if (value === row.value) return;
      const e = await runSetting(`/settings set ${row.key} ${quoteArg(value)}`);
      if (e) {
        toast(e, false);
        err.textContent = e;
        err.hidden = false;
      } else toast(`已儲存 ${row.key}`);
    }

    const ctl = h("div", "m-ctl");
    if (kind === "bool") {
      const sw = h("label", "m-switch");
      const box = h("input");
      box.type = "checkbox";
      box.checked = row.value === "true";
      box.setAttribute("aria-label", title);
      box.addEventListener("change", () => apply(box.checked ? "true" : "false"));
      sw.append(box, h("span", "m-track"));
      ctl.appendChild(sw);
    } else if (kind === "rect") {
      const vals = row.value.split(/\s+/);
      const fields = ["x1", "y1", "x2", "y2"].map((n, i) => {
        const f = h("label", "m-rect-f");
        const inp = h("input", "m-input-sm");
        inp.inputMode = "numeric";
        inp.value = vals[i] ?? "";
        inp.setAttribute("aria-label", n);
        f.append(h("span", null, n), inp);
        ctl.appendChild(f);
        return inp;
      });
      const commit = () =>
        setTimeout(() => {
          if (!fields.includes(document.activeElement)) apply(fields.map((f) => f.value.trim()).join(" "));
        }, 0);
      fields.forEach((f) => f.addEventListener("change", commit));
    } else {
      const inp = h("input", `m-input-sm ${kind}`);
      inp.value = row.value;
      inp.inputMode = kind === "number" ? "decimal" : "text";
      inp.enterKeyHint = "done";
      inp.spellcheck = false;
      inp.setAttribute("aria-label", title);
      inp.addEventListener("change", () => apply(inp.value.trim()));
      inp.addEventListener("keydown", (e) => e.key === "Enter" && inp.blur());
      ctl.appendChild(inp);
    }
    if (row.value !== row.default) {
      const rb = h("button", "m-reset", "↺");
      rb.title = `恢復預設 (${row.default})`;
      rb.setAttribute("aria-label", rb.title);
      rb.addEventListener("click", async () => {
        const e = await runSetting(`/settings reset ${row.key}`);
        toast(e || `${row.key} 已恢復預設`, !e);
      });
      ctl.prepend(rb);
    }
    r.appendChild(ctl);
    return r;
  }

  // ---------------------------------------------------------- 啟動
  push("overview", overviewPage());
  setTabs(DEFAULT_TABS);
  // 設定改了(在設定頁或別的地方): 底部分頁跟著換
  rowListeners.add(() => {
    const r = rows.find((x) => x.key === "web.mobile_tabs");
    if (r && r.value !== tabValue) setTabs(r.value);
  });
  loadRows();
  setInterval(() => !document.hidden && loadRows(), 10000);
  // 從背景回來時馬上更新
  document.addEventListener("visibilitychange", () => {
    if (document.hidden) return;
    top().onShow && top().onShow();
    loadRows();
    updateBadges();
  });
})();

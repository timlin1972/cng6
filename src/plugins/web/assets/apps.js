// cng6 網頁共用: plugin 清單、圖示、通用狀態卡片、自訂畫面註冊
//
// 每個 plugin 透過 /api/cli 的 "apps" 取得網頁描述(見 src/webui.rs):
// - title / icon / color: 名稱與圖示
// - card: 狀態卡片, 由這裡的 renderCard 繪製(PC 視窗與手機總覽共用)
// - view: 自訂畫面, 前端模組用 Apps.registerView(name, { mount(el, { device }) }) 註冊
"use strict";

const Apps = (() => {
  // ---- 圖示(白色線條, 24x24) ----
  const ICONS = {
    cli: `<path d="M5 7l5 5-5 5"/><path d="M12 18h7"/>`,
    system: `<rect x="6" y="6" width="12" height="12" rx="2"/><path d="M9 2v4M15 2v4M9 18v4M15 18v4M2 9h4M2 15h4M18 9h4M18 15h4"/>`,
    message: `<path d="M21 12a8 8 0 0 1-11.6 7.1L4 20l1.1-4.4A8 8 0 1 1 21 12z"/>`,
    web: `<circle cx="12" cy="12" r="9"/><path d="M3 12h18M12 3a14 14 0 0 1 0 18M12 3a14 14 0 0 0 0 18"/>`,
    wallpaper: `<rect x="3" y="5" width="18" height="14" rx="2"/><circle cx="9" cy="10" r="1.6"/><path d="M21 16l-5-5-8 8"/>`,
    settings: `<circle cx="12" cy="12" r="3"/><path d="M19.4 15a1.7 1.7 0 0 0 .3 1.8l.1.1a2 2 0 1 1-2.8 2.8l-.1-.1a1.7 1.7 0 0 0-1.8-.3 1.7 1.7 0 0 0-1 1.5V21a2 2 0 1 1-4 0v-.1a1.7 1.7 0 0 0-1.1-1.5 1.7 1.7 0 0 0-1.8.3l-.1.1a2 2 0 1 1-2.8-2.8l.1-.1a1.7 1.7 0 0 0 .3-1.8 1.7 1.7 0 0 0-1.5-1H3a2 2 0 1 1 0-4h.1a1.7 1.7 0 0 0 1.5-1.1 1.7 1.7 0 0 0-.3-1.8l-.1-.1a2 2 0 1 1 2.8-2.8l.1.1a1.7 1.7 0 0 0 1.8.3H9a1.7 1.7 0 0 0 1-1.5V3a2 2 0 1 1 4 0v.1a1.7 1.7 0 0 0 1 1.5 1.7 1.7 0 0 0 1.8-.3l.1-.1a2 2 0 1 1 2.8 2.8l-.1.1a1.7 1.7 0 0 0-.3 1.8V9a1.7 1.7 0 0 0 1.5 1H21a2 2 0 1 1 0 4h-.1a1.7 1.7 0 0 0-1.5 1z"/>`,
    panel: `<rect x="3" y="4" width="18" height="16" rx="2"/><path d="M12 4v16M3 12h9"/>`,
    appearance: `<circle cx="12" cy="12" r="8"/><path d="M12 4a8 8 0 0 1 0 16z" fill="#fff"/>`,
    // 給新 plugin 用的通用圖示
    power: `<path d="M12 3v9"/><path d="M6.3 6.3a8 8 0 1 0 11.4 0"/>`,
    clock: `<circle cx="12" cy="12" r="9"/><path d="M12 7v5l3 2"/>`,
    network: `<rect x="9" y="3" width="6" height="5" rx="1"/><rect x="3" y="16" width="6" height="5" rx="1"/><rect x="15" y="16" width="6" height="5" rx="1"/><path d="M12 8v4M6 16v-2h12v2"/>`,
    database: `<ellipse cx="12" cy="5" rx="8" ry="3"/><path d="M4 5v14c0 1.7 3.6 3 8 3s8-1.3 8-3V5"/><path d="M4 12c0 1.7 3.6 3 8 3s8-1.3 8-3"/>`,
    bell: `<path d="M18 8a6 6 0 0 0-12 0c0 7-3 9-3 9h18s-3-2-3-9"/><path d="M13.7 21a2 2 0 0 1-3.4 0"/>`,
    folder: `<path d="M3 7a2 2 0 0 1 2-2h4l2 2h8a2 2 0 0 1 2 2v8a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2z"/>`,
    chart: `<path d="M3 3v18h18"/><path d="M7 15l4-4 3 3 5-6"/>`,
    book: `<path d="M4 4.5A2.5 2.5 0 0 1 6.5 2H20v17H6.5A2.5 2.5 0 0 0 4 21.5z"/><path d="M4 21.5A2.5 2.5 0 0 1 6.5 19H20v3H6.5"/><path d="M9 7h7"/>`,
    music: `<path d="M9 18V5l12-2v13"/><circle cx="6" cy="18" r="3"/><circle cx="18" cy="16" r="3"/>`,
    weather: `<path d="M8 2.5v1.5M3.2 4.6l1 1M1.5 9H3M12.8 4.6l-1 1"/><path d="M5.3 10.5a3 3 0 1 1 5.3-2.4"/><path d="M8 20h10a4 4 0 0 0 .4-8 5.5 5.5 0 0 0-10.4 1.5A3.3 3.3 0 0 0 8 20z"/>`,
    other: `<circle cx="12" cy="12" r="3"/><path d="M12 2v3M12 19v3M2 12h3M19 12h3"/>`,
  };
  // 圖示底色(上 → 下漸層)
  const COLORS = {
    blue: ["#5ac8fa", "#0a84ff"],
    green: ["#34c759", "#248a3d"],
    teal: ["#64d2ff", "#30b0c7"],
    purple: ["#bf5af2", "#8e44ad"],
    orange: ["#ff9f0a", "#e07800"],
    red: ["#ff6961", "#d03b3b"],
    pink: ["#ff6fae", "#e0457b"],
    indigo: ["#5e5ce6", "#3634a3"],
    gray: ["#a1a1a6", "#636366"],
    dark: ["#3a3a3c", "#1c1c1e"],
  };

  const iconSvg = (name) =>
    `<svg viewBox="0 0 24 24" fill="none" stroke="#fff" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">${ICONS[name] || ICONS.other}</svg>`;

  /** 圖示元素(圓角方塊 + 漸層底色) */
  function icon(name, color, cls = "app-icon") {
    const el = document.createElement("span");
    el.className = cls;
    const [a, b] = COLORS[color] || COLORS.gray;
    el.style.background = `linear-gradient(180deg, ${a}, ${b})`;
    el.innerHTML = iconSvg(name);
    return el;
  }

  // ---- 資料 ----
  const SESSION = `apps-${Math.random().toString(36).slice(2, 10)}`;

  async function api(op, line) {
    const r = await fetch("/api/cli", {
      method: "POST",
      headers: { "content-type": "application/json" },
      body: JSON.stringify({ session: SESSION, op, line }),
    });
    if (!r.ok) throw new Error((await r.text()) || `HTTP ${r.status}`);
    return r.json();
  }

  let list = [];
  async function load() {
    list = (await api("apps", "")).apps || [];
    return list;
  }
  const find = (name) => list.find((a) => a.name === name);

  /** 執行指令, 回傳 { ok, text }(第一個錯誤, 否則最後一行輸出) */
  async function exec(command) {
    try {
      const res = await api("exec", command);
      const out = res.output.filter((o) => o.level !== "command");
      const err = out.find((o) => o.level === "error");
      if (err) return { ok: false, text: err.text };
      return { ok: true, text: out.length ? out[out.length - 1].text : "" };
    } catch (e) {
      // 例如 web restart: 連線會中斷, 視為已送出
      return { ok: true, text: "" };
    }
  }

  // ---- 自訂畫面 ----
  const views = {};
  const registerView = (name, view) => (views[name] = view);

  // ---- 通用狀態卡片 ----
  // 共用的狀態文字要中性: 同一個狀態可能是溫度過高, 也可能是低溫、大雨
  const STATUS_TEXT = { good: "正常", warning: "注意", critical: "警告", off: "停止" };
  const STATUS_ICON = { warning: "▲", critical: "●", off: "○" };

  function h(tag, cls, text) {
    const el = document.createElement(tag);
    if (cls) el.className = cls;
    if (text != null) el.textContent = text;
    return el;
  }

  /**
   * 把卡片的元件畫進 container。
   * onAction(promise): 按鈕或開關送出指令後呼叫, 用來顯示結果與重新整理
   */
  function renderWidgets(container, card, { onAction } = {}) {
    container.replaceChildren();
    let stats = null; // 連續的 Stat 排成同一列
    for (const w of card.widgets) {
      if (w.type !== "stat") stats = null;
      switch (w.type) {
        case "stat": {
          if (!stats) {
            stats = h("div", "cw-stats");
            container.appendChild(stats);
          }
          const s = h("div", "cw-stat");
          if (w.status) s.dataset.status = w.status;
          const v = h("div", "cw-stat-value");
          v.append(h("b", null, w.value));
          if (w.unit) v.append(h("span", "cw-unit", w.unit));
          s.append(h("div", "cw-stat-label", w.label), v);
          if (w.status === "warning" || w.status === "critical") {
            s.append(h("div", "cw-badge", `${STATUS_ICON[w.status]} ${STATUS_TEXT[w.status]}`));
          } else if (w.hint) {
            s.append(h("div", "cw-hint", w.hint));
          }
          stats.appendChild(s);
          break;
        }
        case "meter": {
          const m = h("div", "cw-meter");
          if (w.status) m.dataset.status = w.status;
          const top = h("div", "cw-meter-top");
          top.append(h("span", "cw-meter-label", w.label), h("span", "cw-meter-text", w.text));
          const bar = h("div", "cw-bar");
          const fill = h("div", "cw-bar-fill");
          fill.style.width = `${(w.ratio * 100).toFixed(1)}%`;
          bar.appendChild(fill);
          m.append(top, bar);
          container.appendChild(m);
          break;
        }
        case "table": {
          const t = h("div", "cw-table");
          for (const [k, v] of w.rows) t.append(h("div", "cw-k", k), h("div", "cw-v", v));
          container.appendChild(t);
          break;
        }
        case "text": {
          const t = h("div", "cw-text");
          if (w.status) {
            t.dataset.status = w.status;
            if (STATUS_ICON[w.status]) t.prepend(h("span", "cw-text-icon", STATUS_ICON[w.status]));
          }
          t.append(document.createTextNode(w.text));
          container.appendChild(t);
          break;
        }
        case "link": {
          const a = h("a", "cw-link", w.label);
          a.href = w.url;
          a.target = "_blank";
          a.rel = "noopener";
          container.appendChild(a);
          break;
        }
        case "toggle": {
          const row = h("label", "cw-toggle");
          const box = h("input");
          box.type = "checkbox";
          box.checked = w.value;
          box.addEventListener("change", () => {
            const p = exec(`/settings set ${w.setting} ${box.checked}`);
            if (onAction) onAction(p);
          });
          const sw = h("span", "cw-switch");
          sw.append(box, h("span", "cw-track"));
          row.append(h("span", null, w.label), sw);
          container.appendChild(row);
          break;
        }
        case "buttons": {
          const row = h("div", "cw-buttons");
          for (const b of w.buttons) {
            const btn = h("button", "cw-btn", b.label);
            btn.addEventListener("click", () => {
              if (b.confirm && !confirm(`確定要「${b.label}」?\n${b.command}`)) return;
              btn.disabled = true;
              const p = exec(b.command).finally(() => (btn.disabled = false));
              if (onAction) onAction(p);
            });
            row.appendChild(btn);
          }
          container.appendChild(row);
          break;
        }
      }
    }
  }

  /**
   * 把一個 plugin 的狀態卡片掛在 el(PC 視窗內容), 定時更新; 回傳停止函式
   */
  function mountCard(el, name) {
    el.classList.add("cw-root");
    el.innerHTML = `
      <header class="cw-head">
        <span class="cw-head-icon"></span>
        <div class="cw-head-text"><div class="cw-title"></div><div class="cw-summary"></div></div>
        <span class="cw-pill" hidden></span>
      </header>
      <div class="cw-body"></div>
      <div class="cw-result" hidden></div>`;
    const body = el.querySelector(".cw-body");
    const result = el.querySelector(".cw-result");
    const pill = el.querySelector(".cw-pill");
    let timer = null;
    let closed = false;
    let busy = false; // 使用者操作開關時, 不要被定時更新蓋掉

    function showResult({ ok, text }) {
      if (!text) return;
      result.textContent = text;
      result.dataset.ok = ok;
      result.hidden = false;
      clearTimeout(showResult.t);
      showResult.t = setTimeout(() => (result.hidden = true), 4000);
    }

    async function refresh() {
      if (busy) return;
      try {
        await load();
      } catch {
        return;
      }
      const app = find(name);
      if (closed || !app) return;
      const iconEl = el.querySelector(".cw-head-icon");
      iconEl.replaceChildren(icon(app.icon, app.color, "cw-icon"));
      el.querySelector(".cw-title").textContent = app.title;
      el.querySelector(".cw-summary").textContent = app.card?.summary || app.description;
      const st = app.card?.status;
      pill.hidden = !st;
      if (st) {
        pill.dataset.status = st;
        pill.textContent = `${STATUS_ICON[st] || "●"} ${STATUS_TEXT[st]}`;
      }
      if (app.card) {
        renderWidgets(body, app.card, {
          onAction: async (p) => {
            busy = true;
            showResult(await p);
            busy = false;
            refresh();
          },
        });
      }
    }

    refresh();
    timer = setInterval(() => !document.hidden && refresh(), 2000);
    return () => {
      closed = true;
      clearInterval(timer);
    };
  }

  return { api, ICONS, COLORS, iconSvg, icon, load, find, get list() { return list; }, exec, views, registerView, renderWidgets, mountCard, STATUS_TEXT };
})();

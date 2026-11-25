// cng6 PC 版桌面: Dock 與視窗管理(開啟、拖曳、改大小、最小化、最大化、關閉)
"use strict";

// 桌面上的程式: CLI 與設定是內建的, 其他來自 plugin(/api/cli 的 "apps", 見 apps.js)
// - plugin 有自訂畫面(view): 開啟對應的前端模組
// - plugin 只有狀態卡片(card): 開啟通用的卡片視窗
// 新 plugin 只要在 Rust 提供 web_meta / web_card / web_view, 這裡不用改。
const CORE = {
  cli: {
    size: [760, 460],
    // 預設位置: 視窗中心在桌面的 (x, y) 比例
    at: [0.5, 0.55],
    content: (el) => CliApp.mount(el),
  },
  settings: {
    size: [860, 560],
    at: [0.55, 0.42],
    content: (el) => SettingsApp.mount(el),
  },
};
// 自訂畫面的預設大小
const VIEW_SIZE = { system: [980, 720], music: [1040, 700], weather: [820, 720], worldclock: [820, 620], books: [1040, 760], files: [1000, 660], cluster: [900, 640], mail: [1080, 700], terminal: [900, 560] };

let APPS = [];

function buildApps(list) {
  const meta = (name, title, icon, color) => {
    const a = list.find((x) => x.name === name);
    return a ? { title: a.title, icon: a.icon, color: a.color } : { title, icon, color };
  };
  const apps = [{ id: "cli", ...meta("cli", "CLI", "cli", "dark"), ...CORE.cli }];
  for (const a of list) {
    if (a.name in CORE) continue;
    const base = { id: a.name, title: a.title, icon: a.icon, color: a.color, at: [0.5, 0.45] };
    if (a.view && Apps.views[a.view]) {
      apps.push({
        ...base,
        size: VIEW_SIZE[a.view] || [900, 640],
        content: (el) => Apps.views[a.view].mount(el, { device: "pc" }),
      });
    } else if (a.card) {
      apps.push({ ...base, size: [420, 460], content: (el) => Apps.mountCard(el, a.name) });
    }
  }
  apps.push({ id: "settings", ...meta("settings", "設定", "settings", "gray"), ...CORE.settings });
  return apps;
}

const BTN = {
  min: `<svg viewBox="0 0 10 10"><path d="M0 5.5h10"/></svg>`,
  max: `<svg viewBox="0 0 10 10"><rect x="0.5" y="0.5" width="9" height="9"/></svg>`,
  restore: `<svg viewBox="0 0 10 10"><rect x="0.5" y="2.5" width="7" height="7"/><path d="M2.5 2.5v-2h7v7h-2"/></svg>`,
  close: `<svg viewBox="0 0 10 10"><path d="M0 0l10 10M10 0L0 10"/></svg>`,
};

const desktop = document.getElementById("desktop");
const dock = document.getElementById("dock");
const activeLabel = document.querySelector("#menubar .active-app");

// 視窗預設大小改變時換版本號, 讓舊的位置記錄失效
const STORE_KEY = "cng6.windows.v2";
let zTop = 10;
let openCount = 0;
/** id -> { app, el, dockItem, minimized, maximized, restore: {x,y,w,h} } */
const wins = new Map();

// ---- 視窗位置記憶(只存在這個瀏覽器, 讀寫失敗就用預設) ----
function loadGeom() {
  try {
    return JSON.parse(localStorage.getItem(STORE_KEY)) || {};
  } catch {
    return {};
  }
}

function saveGeom() {
  const all = loadGeom();
  for (const [id, w] of wins) {
    all[id] = w.maximized ? { ...w.restore, maximized: true } : { ...rectOf(w.el), maximized: false };
  }
  try {
    localStorage.setItem(STORE_KEY, JSON.stringify(all));
  } catch {
    /* 無法儲存就算了 */
  }
}

// ---- 幾何 ----
function area() {
  // 視窗可用範圍: 選單列以下、Dock 以上
  const d = desktop.getBoundingClientRect();
  const dockTop = dock.getBoundingClientRect().top - d.top;
  return { w: d.width, h: Math.max(200, dockTop - 6) };
}

function rectOf(el) {
  return { x: el.offsetLeft, y: el.offsetTop, w: el.offsetWidth, h: el.offsetHeight };
}

function place(el, r) {
  el.style.left = `${r.x}px`;
  el.style.top = `${r.y}px`;
  el.style.width = `${r.w}px`;
  el.style.height = `${r.h}px`;
}

/** 讓標題列一定留在桌面內, 才拖得回來 */
function clamp(r) {
  const a = area();
  const w = Math.min(r.w, a.w);
  const h = Math.min(r.h, a.h);
  const x = Math.min(Math.max(r.x, 80 - w), a.w - 80);
  const y = Math.min(Math.max(r.y, 0), a.h - 34);
  return { x, y, w, h };
}

function defaultRect(app) {
  const a = area();
  const [w, h] = [Math.min(app.size[0], a.w - 40), Math.min(app.size[1], a.h - 40)];
  // 每多開一個視窗就往右下錯開, 不會完全疊在一起
  const offset = (openCount % 6) * 28;
  if (app.at) {
    const [fx, fy] = app.at;
    return {
      x: Math.max(10, a.w * fx - w / 2 + offset),
      y: Math.min(Math.max(10, a.h * fy - h / 2 + offset), a.h - h - 10),
      w,
      h,
    };
  }
  return { x: Math.max(20, (a.w - w) / 2 - 80 + offset), y: Math.max(20, (a.h - h) / 3 + offset), w, h };
}

// 有動畫的變化(最大化/還原/最小化)
function animate(el) {
  el.classList.add("animating");
  clearTimeout(el._animTimer);
  el._animTimer = setTimeout(() => el.classList.remove("animating"), 260);
}

// ---- focus / z-order ----
function focusWin(id) {
  for (const [wid, w] of wins) {
    w.el.classList.toggle("inactive", wid !== id);
  }
  const w = wins.get(id);
  if (w) {
    w.el.style.zIndex = ++zTop;
    activeLabel.textContent = w.app.title;
  } else {
    activeLabel.textContent = "";
  }
}

function topVisible() {
  let best = null;
  for (const [id, w] of wins) {
    if (!w.minimized && (!best || +w.el.style.zIndex > +best.el.style.zIndex)) best = { id, el: w.el };
  }
  return best && best.id;
}

// ---- 開啟 / 關閉 / 最小化 / 最大化 ----
function openApp(app) {
  const saved = loadGeom()[app.id];
  const el = document.createElement("div");
  el.className = "window opening";
  el.innerHTML = `
    <div class="titlebar">
      <span class="icon-slot"></span>
      <span class="title"></span>
      <div class="buttons">
        <button class="min" title="最小化">${BTN.min}</button>
        <button class="max" title="最大化">${BTN.max}</button>
        <button class="close" title="關閉">${BTN.close}</button>
      </div>
    </div>
    <div class="content"></div>
    ${["n", "s", "e", "w", "ne", "nw", "se", "sw"].map((d) => `<div class="resize ${d}" data-dir="${d}"></div>`).join("")}`;
  el.querySelector(".icon-slot").replaceWith(Apps.icon(app.icon, app.color, "icon"));
  el.querySelector(".title").textContent = app.title;
  const unmount = app.content(el.querySelector(".content"));
  desktop.appendChild(el);

  const w = { app, el, dockItem: dock.querySelector(`[data-app="${app.id}"]`), minimized: false, maximized: false, unmount };
  wins.set(app.id, w);
  w.restore = clamp(saved ? saved : defaultRect(app));
  place(el, w.restore);
  openCount++;
  if (saved && saved.maximized) setMaximized(w, true, false);

  w.dockItem.classList.add("running");
  focusWin(app.id);
  requestAnimationFrame(() => el.classList.remove("opening"));

  const tb = el.querySelector(".titlebar");
  el.addEventListener("pointerdown", () => focusWin(app.id));
  tb.querySelector(".min").addEventListener("click", () => minimize(w));
  tb.querySelector(".max").addEventListener("click", () => setMaximized(w, !w.maximized));
  tb.querySelector(".close").addEventListener("click", () => closeApp(w));
  tb.addEventListener("dblclick", (e) => {
    if (!e.target.closest("button")) setMaximized(w, !w.maximized);
  });
  tb.addEventListener("pointerdown", (e) => {
    if (!e.target.closest("button")) startDrag(w, e);
  });
  el.querySelectorAll(".resize").forEach((h) => h.addEventListener("pointerdown", (e) => startResize(w, e, h.dataset.dir)));
}

function closeApp(w) {
  saveGeom();
  if (typeof w.unmount === "function") w.unmount();
  w.el.classList.add("animating", "opening");
  w.dockItem.classList.remove("running");
  wins.delete(w.app.id);
  setTimeout(() => w.el.remove(), 200);
  focusWin(topVisible());
}

/** 縮到 Dock 圖示的位置 */
function minimize(w) {
  const r = rectOf(w.el);
  const icon = w.dockItem.getBoundingClientRect();
  const d = desktop.getBoundingClientRect();
  const dx = icon.left + icon.width / 2 - d.left - (r.x + r.w / 2);
  const dy = icon.top - d.top - (r.y + r.h / 2);
  animate(w.el);
  w.el.style.transform = `translate(${dx}px, ${dy}px) scale(0.08)`;
  w.el.classList.add("minimized");
  w.minimized = true;
  focusWin(topVisible());
}

function unminimize(w) {
  animate(w.el);
  w.el.style.transform = "";
  w.el.classList.remove("minimized");
  w.minimized = false;
  focusWin(w.app.id);
}

function setMaximized(w, on, withAnim = true) {
  if (withAnim) animate(w.el);
  if (on) {
    if (!w.maximized) w.restore = rectOf(w.el);
    const a = area();
    place(w.el, { x: 0, y: 0, w: a.w, h: a.h });
  } else {
    place(w.el, clamp(w.restore));
  }
  w.maximized = on;
  w.el.classList.toggle("maximized", on);
  const btn = w.el.querySelector(".max");
  btn.innerHTML = on ? BTN.restore : BTN.max;
  btn.title = on ? "還原" : "最大化";
  saveGeom();
}

// ---- 拖曳 / 改大小 ----
function track(e, onMove, onEnd) {
  e.preventDefault();
  document.body.classList.add("dragging");
  const move = (ev) => onMove(ev);
  const up = () => {
    document.removeEventListener("pointermove", move);
    document.removeEventListener("pointerup", up);
    document.body.classList.remove("dragging");
    onEnd && onEnd();
    saveGeom();
  };
  document.addEventListener("pointermove", move);
  document.addEventListener("pointerup", up);
}

function startDrag(w, e) {
  const sx = e.clientX;
  const sy = e.clientY;
  let start = rectOf(w.el);
  let moved = false;
  track(e, (ev) => {
    const dx = ev.clientX - sx;
    const dy = ev.clientY - sy;
    if (!moved && Math.abs(dx) + Math.abs(dy) < 3) return;
    moved = true;
    // 拖曳最大化的視窗: 先還原, 讓滑鼠維持在標題列相對位置
    if (w.maximized) {
      const ratio = (sx - w.el.getBoundingClientRect().left) / w.el.offsetWidth;
      w.maximized = false;
      w.el.classList.remove("maximized");
      w.el.querySelector(".max").innerHTML = BTN.max;
      const r = { ...w.restore, x: sx - desktop.getBoundingClientRect().left - w.restore.w * ratio, y: 0 };
      place(w.el, r);
      start = r;
    }
    place(w.el, clamp({ ...start, x: start.x + dx, y: start.y + dy }));
  });
}

function startResize(w, e, dir) {
  e.stopPropagation();
  focusWin(w.app.id);
  const sx = e.clientX;
  const sy = e.clientY;
  const s = rectOf(w.el);
  const minW = 260;
  const minH = 160;
  const a = area();
  track(e, (ev) => {
    const dx = ev.clientX - sx;
    const dy = ev.clientY - sy;
    let { x, y, w: width, h } = s;
    if (dir.includes("e")) width = Math.min(Math.max(minW, s.w + dx), a.w - s.x);
    if (dir.includes("s")) h = Math.min(Math.max(minH, s.h + dy), a.h - s.y);
    if (dir.includes("w")) {
      width = Math.max(minW, s.w - dx);
      x = s.x + s.w - width;
    }
    if (dir.includes("n")) {
      h = Math.max(minH, s.h - dy);
      y = Math.max(0, s.y + s.h - h);
      h = s.y + s.h - y;
    }
    place(w.el, { x, y, w: width, h });
  });
}

// ---- 外觀: 系統 / 淺色 / 深色(只存在這個瀏覽器) ----
const THEME_KEY = "cng6.theme";
function getTheme() {
  try {
    const t = localStorage.getItem(THEME_KEY);
    return t === "light" || t === "dark" ? t : "system";
  } catch {
    return "system";
  }
}
function setTheme(t) {
  if (t === "light" || t === "dark") document.documentElement.dataset.theme = t;
  else delete document.documentElement.dataset.theme;
  try {
    if (t === "light" || t === "dark") localStorage.setItem(THEME_KEY, t);
    else localStorage.removeItem(THEME_KEY);
  } catch {
    /* 無法儲存時只在這次有效 */
  }
  syncThemeMenu();
  window.dispatchEvent(new CustomEvent("cng6-theme", { detail: getTheme() }));
}

const themeBtn = document.querySelector(".theme-btn");
const themeMenu = document.querySelector(".theme-menu");
function syncThemeMenu() {
  const t = getTheme();
  themeMenu.querySelectorAll("button").forEach((b) => b.setAttribute("aria-checked", String(b.dataset.theme === t)));
}
function toggleThemeMenu(open) {
  themeMenu.hidden = !open;
  themeBtn.setAttribute("aria-expanded", String(open));
}
themeBtn.addEventListener("click", (e) => {
  e.stopPropagation();
  toggleThemeMenu(themeMenu.hidden);
});
themeMenu.querySelectorAll("button").forEach((b) =>
  b.addEventListener("click", () => {
    setTheme(b.dataset.theme);
    toggleThemeMenu(false);
  }),
);
document.addEventListener("pointerdown", (e) => {
  if (!themeMenu.hidden && !e.target.closest(".theme-wrap")) toggleThemeMenu(false);
});
document.addEventListener("keydown", (e) => {
  if (e.key === "Escape" && !themeMenu.hidden) toggleThemeMenu(false);
});
syncThemeMenu();

// ---- 桌布: 定時檢查, 換桌布時淡入 ----
let wpUrl = null;
let wpFront = 0;
let wpInfo = null; // 最近一次的桌布資訊(刪除按鈕用)
async function refreshWallpaper() {
  let info;
  try {
    info = await (await fetch("/api/wallpaper", { cache: "no-store" })).json();
  } catch {
    return;
  }
  wpInfo = info;
  if (info.url === wpUrl) return;
  wpUrl = info.url;
  const layers = document.querySelectorAll("#wallpaper .wp-layer");
  if (!info.url) {
    // 預設漸層: 圖片層全部淡出
    layers.forEach((l) => l.classList.remove("on"));
    return;
  }
  // 圖片載入完成後再淡入, 不會看到一半的圖
  const img = new Image();
  img.onload = () => {
    if (wpUrl !== info.url) return;
    const next = layers[1 - wpFront];
    next.style.backgroundImage = `url("${info.url}")`;
    next.classList.add("on");
    layers[wpFront].classList.remove("on");
    wpFront = 1 - wpFront;
  };
  img.src = info.url;
}
refreshWallpaper();
setInterval(() => {
  if (!document.hidden) refreshWallpaper();
}, 5000);

// ---- 桌布右上角的刪除按鈕: 滑鼠停在桌面空白處的右上角時出現 ----
const wpDel = document.createElement("button");
wpDel.className = "wp-delete";
wpDel.title = "刪除這張桌布";
wpDel.setAttribute("aria-label", "刪除這張桌布");
wpDel.innerHTML = `<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><path d="M3 6h18M8 6V4h8v2M6 6l1 14h10l1-14M10 11v5M14 11v5"/></svg>`;
desktop.appendChild(wpDel);
const WP_HOT = { w: 160, h: 110 }; // 右上角感應範圍
function wpDeletable() {
  return wpInfo && wpInfo.url && wpInfo.showing && wpInfo.showing !== "default";
}
desktop.addEventListener("pointermove", (e) => {
  const r = desktop.getBoundingClientRect();
  const inCorner = e.clientX > r.right - WP_HOT.w && e.clientY < r.top + WP_HOT.h;
  // 只有在桌面空白處(不是視窗上)才出現
  const onDesktop = e.target === desktop || e.target === wpDel || wpDel.contains(e.target);
  wpDel.classList.toggle("show", inCorner && onDesktop && wpDeletable());
});
desktop.addEventListener("pointerleave", () => wpDel.classList.remove("show"));
wpDel.addEventListener("click", async () => {
  if (!wpDeletable()) return;
  const name = wpInfo.showing;
  if (!confirm(`刪除這張桌布?\n\n${name}\n\n檔案會從桌布資料夾移除。`)) return;
  wpDel.classList.remove("show");
  // 先換到下一張(沒有其他圖片時換回預設), 再刪除檔案
  const others = (wpInfo.list || []).filter((w) => w.name !== name);
  await Apps.exec(others.length ? "/wallpaper next" : "/wallpaper set default");
  try {
    const r = await fetch(`/api/wallpaper/${encodeURIComponent(name)}`, { method: "DELETE" });
    if (!r.ok) throw new Error(await r.text());
  } catch (e) {
    alert(`刪除失敗: ${e.message}`);
  }
  refreshWallpaper();
});

// ---- 給其他程式用的介面 ----
window.Desktop = {
  refreshWallpaper,
  getTheme,
  setTheme,
  /** 開啟(或還原)視窗; focus: false 時不搶走目前視窗的 focus */
  open(id, { focus = true } = {}) {
    const app = APPS.find((a) => a.id === id);
    if (!app) return;
    const prev = topVisible();
    const w = wins.get(id);
    if (!w) openApp(app);
    else if (w.minimized) unminimize(w);
    else if (focus) focusWin(id);
    if (!focus && prev && prev !== id) focusWin(prev);
  },
};

// ---- Dock(從 plugin 清單產生) ----
function buildDock() {
  dock.replaceChildren();
  for (const app of APPS) {
    const item = document.createElement("button");
    item.className = "dock-item";
    item.dataset.app = app.id;
    const label = document.createElement("span");
    label.className = "label";
    label.textContent = app.title;
    const dot = document.createElement("span");
    dot.className = "dot";
    item.append(label, Apps.icon(app.icon, app.color), dot);
    if (wins.has(app.id)) item.classList.add("running");
    item.addEventListener("click", () => {
      const w = wins.get(app.id);
      if (!w) openApp(app);
      else if (w.minimized) unminimize(w);
      else if (topVisible() === app.id) minimize(w);
      else focusWin(app.id);
    });
    dock.appendChild(item);
  }
}

APPS = buildApps([]);
buildDock();

// 讓其他模組(例如新郵件通知)打開某個 App
window.DeskNav = {
  openApp(id) {
    const app = APPS.find((a) => a.id === id);
    if (!app) return;
    const w = wins.get(id);
    if (!w) openApp(app);
    else if (w.minimized) unminimize(w);
    else focusWin(id);
  },
};
Apps.load()
  .then((list) => {
    APPS = buildApps(list);
    buildDock();
  })
  .catch(() => {
    /* 讀不到 plugin 清單時只有 CLI 與設定 */
  });

// 點桌面空白處: 沒有作用中的視窗
desktop.addEventListener("pointerdown", (e) => {
  if (e.target === desktop) focusWin(null);
});

// 瀏覽器大小改變: 最大化的視窗跟著調整, 其他視窗保持在可見範圍
window.addEventListener("resize", () => {
  const a = area();
  for (const w of wins.values()) {
    if (w.maximized) place(w.el, { x: 0, y: 0, w: a.w, h: a.h });
    else if (!w.minimized) place(w.el, clamp(rectOf(w.el)));
  }
});

// ---- 選單列時鐘 ----
const clock = document.querySelector("#menubar .clock");
function tick() {
  const now = new Date();
  const wd = ["日", "一", "二", "三", "四", "五", "六"][now.getDay()];
  const hh = String(now.getHours()).padStart(2, "0");
  const mm = String(now.getMinutes()).padStart(2, "0");
  clock.textContent = `${now.getMonth() + 1}月${now.getDate()}日 (${wd}) ${hh}:${mm}`;
}
tick();
setInterval(tick, 1000);

// ---- 時鐘彈出: 月曆 + 世界時鐘的城市 ----
const clockPop = document.querySelector(".clock-pop");
const calTitle = clockPop.querySelector(".cal-title");
const calGrid = clockPop.querySelector(".cal-grid");
const wcList = clockPop.querySelector(".wc-list");
let calMonth = null; // 顯示中的月份(該月 1 日)
let wcZones = [];

function renderCal() {
  const today = new Date();
  const y = calMonth.getFullYear();
  const m = calMonth.getMonth();
  calTitle.textContent = `${y} 年 ${m + 1} 月`;
  const first = new Date(y, m, 1).getDay();
  const start = new Date(y, m, 1 - first);
  // 固定 6 週, 換月時高度不會跳
  let html = ["日", "一", "二", "三", "四", "五", "六"].map((w) => `<span class="wd">${w}</span>`).join("");
  for (let i = 0; i < 42; i++) {
    const d = new Date(start.getFullYear(), start.getMonth(), start.getDate() + i);
    const cls = ["d"];
    if (d.getMonth() !== m) cls.push("other");
    if (d.getDay() === 0 || d.getDay() === 6) cls.push("we");
    if (d.toDateString() === today.toDateString()) cls.push("today");
    html += `<span class="${cls.join(" ")}">${d.getDate()}</span>`;
  }
  calGrid.innerHTML = html;
}

// 某時區相對本機的分鐘差
function tzOffsetMin(tz, now) {
  const p = Object.fromEntries(
    new Intl.DateTimeFormat("en-US", { timeZone: tz, hourCycle: "h23", year: "numeric", month: "numeric", day: "numeric", hour: "numeric", minute: "numeric" })
      .formatToParts(now)
      .map((x) => [x.type, x.value]),
  );
  const there = Date.UTC(p.year, p.month - 1, p.day, p.hour, p.minute);
  const here = Date.UTC(now.getFullYear(), now.getMonth(), now.getDate(), now.getHours(), now.getMinutes());
  return Math.round((there - here) / 60000);
}

function renderWc() {
  const now = new Date();
  const esc = (t) => String(t).replace(/[&<>"]/g, (c) => ({ "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;" })[c]);
  wcList.innerHTML = wcZones
    .map((z) => {
      let tm = "--:--";
      let sub = "";
      try {
        tm = new Intl.DateTimeFormat("zh-TW", { timeZone: z.tz, hour: "2-digit", minute: "2-digit", hourCycle: "h23" }).format(now);
        const off = tzOffsetMin(z.tz, now);
        const there = new Date(now.getTime() + off * 60000);
        const dayDiff = Math.round((new Date(there.toDateString()) - new Date(now.toDateString())) / 86400000);
        const day = dayDiff === 0 ? "今天" : dayDiff === 1 ? "明天" : dayDiff === -1 ? "昨天" : `${dayDiff > 0 ? "+" : ""}${dayDiff} 天`;
        const h = off / 60;
        const hs = off === 0 ? "同時區" : `${off > 0 ? "+" : "−"}${Math.abs(Number.isInteger(h) ? h : h.toFixed(1))} 小時`;
        sub = `${day}, ${hs}`;
      } catch {}
      return `<div class="wc-row"><span class="nm">${esc(z.name)}<span class="sub">${sub}</span></span><span class="tm">${tm}</span></div>`;
    })
    .join("");
}

async function loadWc() {
  try {
    const r = await fetch("/api/worldclock");
    if (r.ok) wcZones = (await r.json()).zones || [];
  } catch {}
  if (!clockPop.hidden) renderWc();
}

let wcTimer = null;
function toggleClockPop(open) {
  clockPop.hidden = !open;
  clock.setAttribute("aria-expanded", String(open));
  clearInterval(wcTimer);
  if (open) {
    const now = new Date();
    calMonth = new Date(now.getFullYear(), now.getMonth(), 1);
    renderCal();
    renderWc();
    loadWc(); // 每次打開都重新讀, 世界時鐘的設定可能改過
    wcTimer = setInterval(() => {
      renderWc();
      // 過了午夜, 今天的標記跟著移動
      if (!calGrid.querySelector(".today") || new Date().getHours() === 0) renderCal();
    }, 10000);
  }
}
clock.addEventListener("click", (e) => {
  e.stopPropagation();
  toggleClockPop(clockPop.hidden);
});
clockPop.querySelectorAll(".cal-nav").forEach((b) =>
  b.addEventListener("click", () => {
    calMonth = new Date(calMonth.getFullYear(), calMonth.getMonth() + Number(b.dataset.d), 1);
    renderCal();
  }),
);
calTitle.addEventListener("click", () => {
  const now = new Date();
  calMonth = new Date(now.getFullYear(), now.getMonth(), 1);
  renderCal();
});
document.addEventListener("pointerdown", (e) => {
  if (!clockPop.hidden && !e.target.closest(".clock-wrap")) toggleClockPop(false);
});
document.addEventListener("keydown", (e) => {
  if (e.key === "Escape" && !clockPop.hidden) toggleClockPop(false);
});

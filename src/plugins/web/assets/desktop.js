// cng6 PC 版桌面: Dock 與視窗管理(開啟、拖曳、改大小、最小化、最大化、關閉)
"use strict";

// 桌面上的程式。之後要加新的視窗, 在這裡加一筆並提供 content()。
const APPS = [
  {
    id: "cli",
    title: "CLI",
    icon: `<svg viewBox="0 0 24 24" fill="none" stroke="#fff" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><path d="M5 7l5 5-5 5"/><path d="M12 18h7"/></svg>`,
    size: [760, 460],
    // 預設位置: 視窗中心在桌面的 (x, y) 比例
    at: [0.5, 0.55],
    content(el) {
      return CliApp.mount(el);
    },
  },
  {
    id: "system",
    title: "System",
    icon: `<svg viewBox="0 0 24 24" fill="none" stroke="#fff" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><rect x="6" y="6" width="12" height="12" rx="2"/><path d="M9 2v4M15 2v4M9 18v4M15 18v4M2 9h4M2 15h4M18 9h4M18 15h4"/></svg>`,
    size: [980, 720],
    at: [0.5, 0.45],
    content(el) {
      // 回傳的函式會在視窗關閉時呼叫(停止即時資料)
      return SystemApp.mount(el);
    },
  },
  {
    id: "settings",
    title: "Settings",
    icon: `<svg viewBox="0 0 24 24" fill="none" stroke="#fff" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><circle cx="12" cy="12" r="3"/><path d="M19.4 15a1.7 1.7 0 0 0 .3 1.8l.1.1a2 2 0 1 1-2.8 2.8l-.1-.1a1.7 1.7 0 0 0-1.8-.3 1.7 1.7 0 0 0-1 1.5V21a2 2 0 1 1-4 0v-.1a1.7 1.7 0 0 0-1.1-1.5 1.7 1.7 0 0 0-1.8.3l-.1.1a2 2 0 1 1-2.8-2.8l.1-.1a1.7 1.7 0 0 0 .3-1.8 1.7 1.7 0 0 0-1.5-1H3a2 2 0 1 1 0-4h.1a1.7 1.7 0 0 0 1.5-1.1 1.7 1.7 0 0 0-.3-1.8l-.1-.1a2 2 0 1 1 2.8-2.8l.1.1a1.7 1.7 0 0 0 1.8.3H9a1.7 1.7 0 0 0 1-1.5V3a2 2 0 1 1 4 0v.1a1.7 1.7 0 0 0 1 1.5 1.7 1.7 0 0 0 1.8-.3l.1-.1a2 2 0 1 1 2.8 2.8l-.1.1a1.7 1.7 0 0 0-.3 1.8V9a1.7 1.7 0 0 0 1.5 1H21a2 2 0 1 1 0 4h-.1a1.7 1.7 0 0 0-1.5 1z"/></svg>`,
    size: [860, 560],
    at: [0.55, 0.42],
    content(el) {
      return SettingsApp.mount(el);
    },
  },
];

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
  if (app.at) {
    const [fx, fy] = app.at;
    return { x: Math.max(10, a.w * fx - w / 2), y: Math.min(Math.max(10, a.h * fy - h / 2), a.h - h - 10), w, h };
  }
  const offset = (openCount % 6) * 28;
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
      <span class="icon app-icon ${app.id}" style="border-radius:4px;display:grid;place-items:center">${app.icon.replace("<svg", '<svg style="width:12px;height:12px"')}</span>
      <span class="title">${app.title}</span>
      <div class="buttons">
        <button class="min" title="最小化">${BTN.min}</button>
        <button class="max" title="最大化">${BTN.max}</button>
        <button class="close" title="關閉">${BTN.close}</button>
      </div>
    </div>
    <div class="content"></div>
    ${["n", "s", "e", "w", "ne", "nw", "se", "sw"].map((d) => `<div class="resize ${d}" data-dir="${d}"></div>`).join("")}`;
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
async function refreshWallpaper() {
  let info;
  try {
    info = await (await fetch("/api/wallpaper", { cache: "no-store" })).json();
  } catch {
    return;
  }
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

// ---- Dock ----
for (const app of APPS) {
  const item = document.createElement("button");
  item.className = "dock-item";
  item.dataset.app = app.id;
  item.innerHTML = `<span class="label">${app.title}</span><span class="app-icon ${app.id}">${app.icon}</span><span class="dot"></span>`;
  item.addEventListener("click", () => {
    const w = wins.get(app.id);
    if (!w) openApp(app);
    else if (w.minimized) unminimize(w);
    else if (topVisible() === app.id) minimize(w);
    else focusWin(app.id);
  });
  dock.appendChild(item);
}

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

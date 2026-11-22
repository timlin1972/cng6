// cng6 世界時間: 各城市的時鐘、時差、時間換算滑桿(PC 視窗與手機共用)
//
// 城市清單來自 /api/worldclock, 時間由瀏覽器依時區自己計算(Intl), 每秒更新。
"use strict";

const WorldClockApp = (() => {
  const WEEK = ["日", "一", "二", "三", "四", "五", "六"];
  const fmtCache = new Map();
  function parts(date, tz) {
    let f = fmtCache.get(tz);
    if (!f) {
      f = new Intl.DateTimeFormat("en-US", {
        timeZone: tz,
        hourCycle: "h23",
        year: "numeric",
        month: "numeric",
        day: "numeric",
        hour: "numeric",
        minute: "numeric",
        second: "numeric",
      });
      fmtCache.set(tz, f);
    }
    const p = {};
    for (const x of f.formatToParts(date)) if (x.type !== "literal") p[x.type] = +x.value;
    return p;
  }
  /** 時區在這個時刻的 UTC 偏移(分鐘) */
  function offset(date, tz) {
    const p = parts(date, tz);
    const asUtc = Date.UTC(p.year, p.month - 1, p.day, p.hour, p.minute, p.second);
    return Math.round((asUtc - Math.floor(date.getTime() / 1000) * 1000) / 60000);
  }
  function fmtDiff(min) {
    if (min === 0) return "同本地";
    const s = min > 0 ? "+" : "-";
    const m = Math.abs(min);
    return m % 60 ? `${s}${Math.floor(m / 60)}:${String(m % 60).padStart(2, "0")}` : `${s}${m / 60} 小時`;
  }
  const pad = (n) => String(n).padStart(2, "0");

  function h(tag, cls, text) {
    const el = document.createElement(tag);
    if (cls) el.className = cls;
    if (text != null) el.textContent = text;
    return el;
  }
  const quoteArg = (a) => (a !== "" && !/[\s"'\\]/.test(a) ? a : `"${a.replace(/\\/g, "\\\\").replace(/"/g, '\\"')}"`);

  // 指針式時鐘
  function clockFace() {
    const ticks = Array.from({ length: 12 }, (_, i) => {
      const a = (i * Math.PI) / 6;
      const r1 = i % 3 ? 41 : 37;
      return `<line x1="${50 + Math.sin(a) * r1}" y1="${50 - Math.cos(a) * r1}" x2="${50 + Math.sin(a) * 45}" y2="${50 - Math.cos(a) * 45}" class="${i % 3 ? "tick" : "tick major"}"/>`;
    }).join("");
    const el = h("span", "wc-face");
    el.innerHTML = `<svg viewBox="0 0 100 100" aria-hidden="true">
      <circle cx="50" cy="50" r="48" class="dial"/>${ticks}
      <line x1="50" y1="50" x2="50" y2="26" class="hand hour"/>
      <line x1="50" y1="50" x2="50" y2="14" class="hand min"/>
      <line x1="50" y1="58" x2="50" y2="12" class="hand sec"/>
      <circle cx="50" cy="50" r="3" class="pin"/></svg>`;
    return el;
  }
  function setHands(face, p, showSec) {
    const set = (cls, deg) => face.querySelector(`.${cls}`).setAttribute("transform", `rotate(${deg} 50 50)`);
    set("hour", (p.hour % 12) * 30 + p.minute * 0.5);
    set("min", p.minute * 6 + p.second * 0.1);
    const sec = face.querySelector(".sec");
    sec.style.display = showSec ? "" : "none";
    set("sec", p.second * 6);
  }

  // ======================================================================
  function mount(el, { device = "pc" } = {}) {
    el.classList.add("wc-root", device === "mobile" ? "wc-mobile" : "wc-pc");
    el.innerHTML = `
      <div class="wc-scroll">
        <header class="wc-top">
          <div class="wc-title"><b>世界時間</b><span class="wc-local"></span></div>
          <button class="wc-btn" data-act="add">＋ 城市</button>
        </header>
        <section class="wc-convert">
          <div class="wc-convert-head">
            <span class="wc-convert-label">時間換算</span>
            <b class="wc-convert-value">現在</b>
            <button class="wc-btn sm" data-act="now" hidden>回到現在</button>
          </div>
          <input type="range" class="wc-slider" min="0" max="47" step="1" aria-label="本地時間">
          <div class="wc-slider-scale"><span>0:00</span><span>6:00</span><span>12:00</span><span>18:00</span><span>24:00</span></div>
        </section>
        <div class="wc-grid"></div>
      </div>
      <div class="wc-toast" role="status"></div>`;
    const grid = el.querySelector(".wc-grid");
    const localEl = el.querySelector(".wc-local");
    const slider = el.querySelector(".wc-slider");
    const convValue = el.querySelector(".wc-convert-value");
    const nowBtn = el.querySelector('[data-act="now"]');
    const toastEl = el.querySelector(".wc-toast");
    let local = "Asia/Taipei";
    let zones = [];
    let pinned = null; // 換算中: 固定的時刻(Date); null = 現在
    let cards = [];
    let closed = false;
    let lastKey = "";

    function toast(text, ok = true) {
      toastEl.textContent = text;
      toastEl.dataset.ok = ok;
      toastEl.classList.add("show");
      clearTimeout(toast.t);
      toast.t = setTimeout(() => toastEl.classList.remove("show"), 2800);
    }
    async function run(cmd) {
      const r = await Apps.exec(cmd);
      if (r.text) toast(r.text, r.ok);
      setTimeout(load, 800);
      setTimeout(load, 5000);
    }
    el.querySelector('[data-act="add"]').addEventListener("click", () => {
      const name = prompt("加入城市(台灣: xx市xx區, 國外: 英文, 或時區名稱如 Europe/London)");
      if (name && name.trim()) run(`/worldclock add ${name.trim()}`);
    });

    // ---- 換算滑桿: 本地今天的 0:00 ~ 23:30 ----
    function localMidnight() {
      const now = new Date();
      const p = parts(now, local);
      const off = offset(now, local);
      return new Date(Date.UTC(p.year, p.month - 1, p.day) - off * 60000);
    }
    slider.addEventListener("input", () => {
      pinned = new Date(localMidnight().getTime() + slider.value * 30 * 60000);
      tick();
    });
    nowBtn.addEventListener("click", () => {
      pinned = null;
      tick();
    });

    function render() {
      grid.replaceChildren();
      const all = [{ name: "本地", tz: local, local: true }, ...zones];
      cards = all.map((z) => {
        const card = h("section", `wc-card ${z.local ? "local" : ""}`);
        const face = clockFace();
        const info = h("div", "wc-info");
        const name = h("div", "wc-name", z.name);
        const time = h("div", "wc-time");
        const sub = h("div", "wc-sub");
        info.append(name, time, sub);
        card.append(face, info);
        if (!z.local) {
          const more = h("button", "wc-more", "⋯");
          more.title = "城市設定";
          const acts = h("div", "wc-actions");
          acts.hidden = true;
          if (zones[0] !== z) {
            const b = h("button", "wc-btn sm", "移到最前面");
            b.addEventListener("click", () => run(`/worldclock first ${quoteArg(z.name)}`));
            acts.append(b);
          }
          const rm = h("button", "wc-btn sm danger", "移除");
          rm.addEventListener("click", () => confirm(`移除「${z.name}」?`) && run(`/worldclock remove ${quoteArg(z.name)}`));
          acts.append(rm);
          more.addEventListener("click", () => (acts.hidden = !acts.hidden));
          card.append(more, acts);
        }
        grid.append(card);
        return { z, card, face, time, sub };
      });
      if (!zones.length) {
        grid.append(h("div", "wc-empty", "按右上角「＋ 城市」加入其他城市"));
      }
      tick();
    }

    function tick() {
      const at = pinned || new Date();
      const lp = parts(at, local);
      const lo = offset(at, local);
      localEl.textContent = local;
      nowBtn.hidden = !pinned;
      convValue.textContent = pinned ? `本地 ${pad(lp.hour)}:${pad(lp.minute)}` : "現在";
      if (!pinned) slider.value = lp.hour * 2 + (lp.minute >= 30 ? 1 : 0);
      for (const c of cards) {
        const p = parts(at, c.z.tz);
        const diff = offset(at, c.z.tz) - lo;
        const dayDiff = Math.round((Date.UTC(p.year, p.month - 1, p.day) - Date.UTC(lp.year, lp.month - 1, lp.day)) / 86400000);
        const night = p.hour < 6 || p.hour >= 18;
        c.card.classList.toggle("night", night);
        setHands(c.face, p, !pinned);
        c.time.textContent = `${pad(p.hour)}:${pad(p.minute)}`;
        const weekday = WEEK[new Date(Date.UTC(p.year, p.month - 1, p.day)).getUTCDay()];
        const parts_ = [`${p.month}/${p.day} 週${weekday}`];
        if (!c.z.local) parts_.push(fmtDiff(diff));
        if (dayDiff === 1) parts_.push("明天");
        if (dayDiff === -1) parts_.push("昨天");
        c.sub.textContent = parts_.join(" · ");
      }
    }

    async function load() {
      try {
        const d = await (await fetch("/api/worldclock", { cache: "no-store" })).json();
        local = d.local;
        zones = d.zones;
      } catch {
        return;
      }
      const key = JSON.stringify([local, zones]);
      if (key !== lastKey) {
        lastKey = key;
        render();
      }
    }

    load();
    const t1 = setInterval(() => !closed && !pinned && tick(), 1000);
    const t2 = setInterval(() => !closed && !document.hidden && load(), 30000);
    return () => {
      closed = true;
      clearInterval(t1);
      clearInterval(t2);
    };
  }

  return { mount };
})();

if (typeof Apps !== "undefined") Apps.registerView("worldclock", WorldClockApp);

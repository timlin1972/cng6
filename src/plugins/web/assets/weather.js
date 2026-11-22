// cng6 天氣: 目前天氣、警示、逐小時與 7 天預報(PC 視窗與手機共用)
"use strict";

const WeatherApp = (() => {
  // ---- 圖示 ----
  const SUN = `<circle cx="12" cy="12" r="4.2" fill="#fdb813"/><g stroke="#fdb813" stroke-width="1.8" stroke-linecap="round"><path d="M12 2.5v2.2M12 19.3v2.2M2.5 12h2.2M19.3 12h2.2M5.3 5.3l1.6 1.6M17.1 17.1l1.6 1.6M5.3 18.7l1.6-1.6M17.1 6.9l1.6-1.6"/></g>`;
  const MOON = `<path d="M15.5 3.5a8 8 0 1 0 5 13.6A7 7 0 0 1 15.5 3.5z" fill="#f4d77a"/>`;
  const CLOUD = (fill = "#c7ccd6", dx = 0, dy = 0) =>
    `<path transform="translate(${dx} ${dy})" d="M7 19h10.5a4 4 0 0 0 .4-8 5.5 5.5 0 0 0-10.6-1.2A4.5 4.5 0 0 0 7 19z" fill="${fill}"/>`;
  const DROPS = (n = 3, color = "#4aa3f0") =>
    `<g stroke="${color}" stroke-width="1.8" stroke-linecap="round">${[8, 12, 16]
      .slice(0, n)
      .map((x) => `<path d="M${x} 19.5l-1 2.8"/>`)
      .join("")}</g>`;
  const ICON = {
    clear: (day) => (day ? SUN : MOON),
    partly: (day) =>
      `<g transform="translate(-3 -3) scale(.8)">${day ? SUN : MOON}</g>${CLOUD("#d5d9e1", 1.5, 1.5)}`,
    cloud: () => CLOUD("#b8bec9", 0, -1),
    fog: () => `${CLOUD("#c7ccd6", 0, -3)}<g stroke="#9aa1ad" stroke-width="1.6" stroke-linecap="round"><path d="M4 19h16M6 22h12"/></g>`,
    drizzle: () => `${CLOUD("#b8bec9", 0, -3)}${DROPS(2, "#6db6f2")}`,
    rain: () => `${CLOUD("#9aa3b2", 0, -3)}${DROPS(3)}`,
    snow: () => `${CLOUD("#b8bec9", 0, -3)}<g fill="#9fd0f7"><circle cx="8" cy="20.5" r="1.2"/><circle cx="12" cy="22" r="1.2"/><circle cx="16" cy="20.5" r="1.2"/></g>`,
    thunder: () => `${CLOUD("#7e8696", 0, -3)}<path d="M12.5 15.5l-3 4.5h2.5l-1.5 3.5 4-5h-2.5l1.5-3z" fill="#fdb813"/>`,
  };
  function kind(code) {
    if (code === 0) return "clear";
    if (code <= 2) return "partly";
    if (code === 3) return "cloud";
    if (code === 45 || code === 48) return "fog";
    if (code >= 51 && code <= 57) return "drizzle";
    if ((code >= 61 && code <= 67) || (code >= 80 && code <= 82)) return "rain";
    if ((code >= 71 && code <= 77) || code === 85 || code === 86) return "snow";
    if (code >= 95) return "thunder";
    return "cloud";
  }
  const DESC = {
    0: "晴", 1: "大致晴朗", 2: "局部多雲", 3: "陰", 45: "霧", 48: "霧", 51: "毛毛雨", 53: "毛毛雨", 55: "濃毛毛雨",
    56: "凍毛毛雨", 57: "凍毛毛雨", 61: "小雨", 63: "中雨", 65: "大雨", 66: "凍雨", 67: "凍雨", 71: "小雪", 73: "中雪",
    75: "大雪", 77: "雪粒", 80: "陣雨", 81: "較強陣雨", 82: "強陣雨", 85: "陣雪", 86: "陣雪", 95: "雷雨", 96: "雷雨伴有冰雹", 99: "雷雨伴有冰雹",
  };
  const desc = (c) => DESC[c] || "未知";
  function icon(code, day = true, cls = "wx-icon") {
    const s = document.createElement("span");
    s.className = cls;
    s.innerHTML = `<svg viewBox="0 0 24 24" aria-hidden="true">${ICON[kind(code)](day)}</svg>`;
    s.title = desc(code);
    return s;
  }

  function h(tag, cls, text) {
    const el = document.createElement(tag);
    if (cls) el.className = cls;
    if (text != null) el.textContent = text;
    return el;
  }
  const t = (v) => (v == null || Number.isNaN(v) ? "–" : `${Math.round(v)}°`);
  const WEEK = ["日", "一", "二", "三", "四", "五", "六"];
  function dayName(date, i) {
    if (i === 0) return "今天";
    if (i === 1) return "明天";
    return `週${WEEK[new Date(`${date}T00:00`).getDay()]}`;
  }
  const quoteArg = (a) => (a !== "" && !/[\s"'\\]/.test(a) ? a : `"${a.replace(/\\/g, "\\\\").replace(/"/g, '\\"')}"`);

  // ======================================================================
  function mount(el, { device = "pc" } = {}) {
    el.classList.add("wx-root", device === "mobile" ? "wx-mobile" : "wx-pc");
    el.innerHTML = `
      <div class="wx-scroll">
        <header class="wx-top">
          <div class="wx-cities" role="tablist"></div>
          <div class="wx-actions">
            <span class="wx-updated"></span>
            <button class="wx-btn" data-act="refresh" title="立即更新">更新</button>
            <button class="wx-btn" data-act="add" title="加入城市">＋ 城市</button>
          </div>
        </header>
        <div class="wx-content"></div>
      </div>
      <div class="wx-toast" role="status"></div>`;
    const citiesEl = el.querySelector(".wx-cities");
    const content = el.querySelector(".wx-content");
    const updatedEl = el.querySelector(".wx-updated");
    const toastEl = el.querySelector(".wx-toast");
    let data = { cities: [], version: 0 };
    let selected = null;
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
      // 搜尋城市要連網, 晚一點再抓一次
      setTimeout(load, 4000);
      return r;
    }

    el.querySelector('[data-act="refresh"]').addEventListener("click", () => run("/weather refresh"));
    el.querySelector('[data-act="add"]').addEventListener("click", () => {
      const name = prompt("加入城市(例如 台北、花蓮、新店區、Tokyo)");
      if (name && name.trim()) run(`/weather add ${name.trim()}`);
    });

    function render() {
      const cities = data.cities;
      if (!cities.find((c) => c.name === selected)) selected = cities[0]?.name;
      // 城市切換
      citiesEl.replaceChildren();
      citiesEl.hidden = cities.length < 2;
      for (const c of cities) {
        const b = h("button", `wx-city ${c.name === selected ? "on" : ""}`);
        b.append(h("span", null, c.name));
        if (c.forecast) b.append(h("b", null, t(c.forecast.current.temp)));
        if (c.forecast?.alerts.length) b.append(h("span", "wx-city-alert", "▲"));
        b.addEventListener("click", () => {
          selected = c.name;
          render();
        });
        citiesEl.appendChild(b);
      }
      const city = cities.find((c) => c.name === selected);
      updatedEl.textContent = city?.updated ? `${city.updated} 更新` : "";
      content.replaceChildren();
      if (!city) {
        content.append(h("div", "wx-empty", data.loaded ? "還沒有設定城市, 按右上角「＋ 城市」加入" : "讀取中…"));
        return;
      }
      if (!city.forecast) {
        content.append(h("div", "wx-empty", city.error || "讀取中…"));
        return;
      }
      const f = city.forecast;
      const today = f.daily[0];

      // ---- 目前天氣 ----
      const hero = h("section", "wx-hero");
      const left = h("div", "wx-hero-main");
      left.append(h("div", "wx-place", city.name));
      const tempRow = h("div", "wx-temp-row");
      tempRow.append(icon(f.current.code, f.current.is_day, "wx-icon big"), h("span", "wx-temp", t(f.current.temp)));
      left.append(tempRow, h("div", "wx-desc", desc(f.current.code)));
      const right = h("div", "wx-hero-side");
      const facts = [
        ["最高 / 最低", `${t(today?.max)} / ${t(today?.min)}`],
        ["體感", t(f.current.feels)],
        ["降雨機率", `${Math.round(today?.prob ?? 0)}%`],
        ["風速", `${Math.round(f.current.wind)} km/h`],
      ];
      for (const [k, v] of facts) {
        const row = h("div", "wx-fact");
        row.append(h("span", "wx-fact-k", k), h("b", null, v));
        right.append(row);
      }
      const more = h("button", "wx-more", "⋯");
      more.title = "城市設定";
      hero.append(left, right, more);
      content.append(hero);
      // ⋯: 展開城市設定按鈕
      const acts = h("div", "wx-city-actions");
      acts.hidden = true;
      if (data.cities[0]?.name !== city.name) {
        const b = h("button", "wx-btn", "設為主要城市");
        b.title = "總覽與卡片會顯示這個城市";
        b.addEventListener("click", () => run(`/weather first ${quoteArg(city.name)}`));
        acts.append(b);
      }
      if (data.cities.length > 1) {
        const b = h("button", "wx-btn danger", `移除「${city.name}」`);
        b.addEventListener("click", () => confirm(`移除城市「${city.name}」?`) && run(`/weather remove ${quoteArg(city.name)}`));
        acts.append(b);
      }
      if (!acts.children.length) acts.append(h("span", "wx-hint", "用上方的「＋ 城市」加入更多城市"));
      more.addEventListener("click", () => (acts.hidden = !acts.hidden));
      content.append(acts);

      // ---- 警示 ----
      for (const a of f.alerts) {
        const al = h("div", `wx-alert ${a.status}`);
        al.append(h("span", "wx-alert-icon", a.status === "critical" ? "●" : "▲"));
        const tx = h("div");
        tx.append(h("b", null, `${a.title}${a.status === "critical" ? "(嚴重)" : ""}`), h("div", "wx-alert-detail", a.detail));
        al.append(tx);
        content.append(al);
      }

      content.append(hourly(f.hourly.slice(0, 25)), daily(f.daily));
      content.append(h("div", "wx-foot", "資料來源 Open-Meteo · 警示依預報數值判斷, 不是氣象署的正式警特報"));
    }

    // ---- 逐小時: 時間 / 圖示 / 溫度曲線 / 降雨機率 ----
    function hourly(hours) {
      const sec = h("section", "wx-card");
      sec.append(h("h3", null, "逐小時預報"));
      const wrap = h("div", "wx-hours-wrap");
      const grid = h("div", "wx-hours");
      const COL = 52;
      const W = COL * hours.length;
      grid.style.width = `${W}px`;
      // 每欄: 時間 + 圖示
      const cols = h("div", "wx-hour-cols");
      hours.forEach((x, i) => {
        const c = h("div", "wx-hour");
        c.append(h("div", "wx-hour-time", i === 0 ? "現在" : `${x.time.slice(11, 13)}時`), icon(x.code, x.is_day, "wx-icon sm"));
        cols.append(c);
      });
      grid.append(cols);
      // 溫度曲線(SVG), 標上數值
      const temps = hours.map((x) => x.temp);
      const lo = Math.min(...temps);
      const hi = Math.max(...temps);
      const H = 70;
      const y = (v) => 18 + (hi === lo ? 0.5 : 1 - (v - lo) / (hi - lo)) * (H - 30);
      const pts = temps.map((v, i) => [i * COL + COL / 2, y(v)]);
      const path = pts.map((p, i) => `${i ? "L" : "M"}${p[0].toFixed(1)} ${p[1].toFixed(1)}`).join(" ");
      const area = `${path} L${pts[pts.length - 1][0]} ${H} L${pts[0][0]} ${H} Z`;
      const svg = `<svg class="wx-temp-chart" width="${W}" height="${H}" viewBox="0 0 ${W} ${H}">
        <defs><linearGradient id="wxg" x1="0" x2="0" y1="0" y2="1"><stop offset="0" stop-color="var(--wx-temp)" stop-opacity=".25"/><stop offset="1" stop-color="var(--wx-temp)" stop-opacity="0"/></linearGradient></defs>
        <path d="${area}" fill="url(#wxg)"/>
        <path d="${path}" fill="none" stroke="var(--wx-temp)" stroke-width="2" stroke-linejoin="round" stroke-linecap="round"/>
        ${pts.map((p, i) => `<circle cx="${p[0]}" cy="${p[1]}" r="3" fill="var(--wx-temp)" stroke="var(--wx-card)" stroke-width="2"/><text x="${p[0]}" y="${p[1] - 8}" text-anchor="middle">${Math.round(temps[i])}°</text>`).join("")}
      </svg>`;
      const chart = h("div", "wx-chart");
      chart.innerHTML = svg;
      grid.append(chart);
      // 降雨機率(長條)
      const rain = h("div", "wx-rain");
      hours.forEach((x) => {
        const c = h("div", "wx-rain-col");
        c.title = `降雨機率 ${Math.round(x.prob)}% · 雨量 ${x.precip.toFixed(1)} mm`;
        const bar = h("div", "wx-rain-bar");
        bar.style.height = `${Math.max(2, x.prob * 0.36)}px`;
        bar.style.opacity = x.prob ? 1 : 0.35;
        c.append(bar, h("div", "wx-rain-val", `${Math.round(x.prob)}%`));
        rain.append(c);
      });
      grid.append(rain);
      wrap.append(grid);
      sec.append(wrap, h("div", "wx-legend", "曲線: 溫度 · 長條: 降雨機率"));
      return sec;
    }

    // ---- 7 天: 星期 / 圖示 / 降雨 / 溫度區間 ----
    function daily(days) {
      const sec = h("section", "wx-card");
      sec.append(h("h3", null, "7 天預報"));
      const lo = Math.min(...days.map((d) => d.min));
      const hi = Math.max(...days.map((d) => d.max));
      const span = hi - lo || 1;
      const list = h("div", "wx-days");
      days.forEach((d, i) => {
        const row = h("div", "wx-day");
        row.title = `${desc(d.code)} · 雨量 ${d.precip.toFixed(1)} mm · 陣風 ${Math.round(d.gust)} km/h`;
        const rain = h("span", "wx-day-rain", d.prob >= 10 ? `${Math.round(d.prob)}%` : "");
        const range = h("div", "wx-range");
        const fill = h("div", "wx-range-fill");
        fill.style.left = `${((d.min - lo) / span) * 100}%`;
        fill.style.width = `${Math.max(4, ((d.max - d.min) / span) * 100)}%`;
        range.append(fill);
        row.append(h("span", "wx-day-name", dayName(d.date, i)), icon(d.code, true, "wx-icon sm"), rain, h("span", "wx-day-min", t(d.min)), range, h("span", "wx-day-max", t(d.max)));
        list.append(row);
      });
      sec.append(list);
      return sec;
    }

    async function load() {
      try {
        const r = await fetch("/api/weather", { cache: "no-store" });
        data = await r.json();
        data.loaded = true;
      } catch {
        return;
      }
      const key = JSON.stringify([data.version, selected]);
      if (key !== lastKey) {
        lastKey = key;
        render();
      }
    }
    const timer = setInterval(() => !closed && !document.hidden && load(), 30000);
    load();
    return () => {
      closed = true;
      clearInterval(timer);
    };
  }

  return { mount, icon, desc };
})();

if (typeof Apps !== "undefined") Apps.registerView("weather", WeatherApp);

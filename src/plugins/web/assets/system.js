// cng6 System 視窗: 即時系統資訊儀表板(環形儀表、曲線圖、核心熱度圖)
"use strict";

const SystemApp = (() => {
  const WINDOW_MS = 5 * 60 * 1000; // 圖表顯示最近 5 分鐘
  const MAX_SAMPLES = 300;

  // ---------- 格式化 ----------
  const GiB = 1024 ** 3;
  const fmtGiB = (b) => (b / GiB >= 10 ? (b / GiB).toFixed(0) : (b / GiB).toFixed(1));
  function fmtRate(v) {
    const u = ["B/s", "KB/s", "MB/s", "GB/s"];
    let i = 0;
    while (v >= 1000 && i < u.length - 1) {
      v /= 1000;
      i++;
    }
    return `${v >= 100 || i === 0 ? v.toFixed(0) : v.toFixed(1)} ${u[i]}`;
  }
  function fmtUptime(s) {
    const d = Math.floor(s / 86400);
    const h = Math.floor((s % 86400) / 3600);
    const m = Math.floor((s % 3600) / 60);
    if (d > 0) return `${d} 天 ${h} 小時`;
    if (h > 0) return `${h} 小時 ${m} 分`;
    return `${m} 分`;
  }
  function fmtAgo(ms) {
    const s = Math.round(ms / 1000);
    if (s <= 1) return "現在";
    if (s < 60) return `${s} 秒前`;
    return `${Math.floor(s / 60)} 分 ${s % 60} 秒前`;
  }
  const fmtClock = (t) => new Date(t).toLocaleTimeString("zh-TW", { hour12: false });

  // 標籤是外部資料, 一律用 textContent
  function h(tag, cls, text) {
    const el = document.createElement(tag);
    if (cls) el.className = cls;
    if (text != null) el.textContent = text;
    return el;
  }

  const css = (el, name) => getComputedStyle(el).getPropertyValue(name).trim();

  // 狀態: 依門檻回傳 good / warning / critical
  function status(v, warn, crit) {
    if (v >= crit) return "critical";
    if (v >= warn) return "warning";
    return "good";
  }
  const STATUS_TEXT = { warning: "偏高", critical: "過高" };

  // ---------- 刻度 ----------
  function niceStep(range, count) {
    const raw = range / count;
    const mag = 10 ** Math.floor(Math.log10(raw));
    const n = raw / mag;
    return (n <= 1 ? 1 : n <= 2 ? 2 : n <= 2.5 ? 2.5 : n <= 5 ? 5 : 10) * mag;
  }

  // 單調三次插值(Fritsch–Carlson): 曲線平滑但不會超出資料範圍
  function smoothPath(ctx, pts) {
    const n = pts.length;
    if (n < 2) return;
    const dx = [];
    const m = [];
    for (let i = 0; i < n - 1; i++) {
      dx[i] = pts[i + 1][0] - pts[i][0];
      m[i] = dx[i] === 0 ? 0 : (pts[i + 1][1] - pts[i][1]) / dx[i];
    }
    const t = [m[0]];
    for (let i = 1; i < n - 1; i++) t[i] = m[i - 1] * m[i] <= 0 ? 0 : (m[i - 1] + m[i]) / 2;
    t[n - 1] = m[n - 2];
    for (let i = 0; i < n - 1; i++) {
      if (m[i] === 0) {
        t[i] = t[i + 1] = 0;
        continue;
      }
      const a = t[i] / m[i];
      const b = t[i + 1] / m[i];
      const s = a * a + b * b;
      if (s > 9) {
        const k = 3 / Math.sqrt(s);
        t[i] = k * a * m[i];
        t[i + 1] = k * b * m[i];
      }
    }
    ctx.moveTo(pts[0][0], pts[0][1]);
    for (let i = 0; i < n - 1; i++) {
      const d = dx[i] / 3;
      ctx.bezierCurveTo(
        pts[i][0] + d,
        pts[i][1] + t[i] * d,
        pts[i + 1][0] - d,
        pts[i + 1][1] - t[i + 1] * d,
        pts[i + 1][0],
        pts[i + 1][1],
      );
    }
  }

  function setupCanvas(canvas, w, hgt) {
    const dpr = window.devicePixelRatio || 1;
    if (canvas.width !== Math.round(w * dpr) || canvas.height !== Math.round(hgt * dpr)) {
      canvas.width = Math.round(w * dpr);
      canvas.height = Math.round(hgt * dpr);
      canvas.style.width = `${w}px`;
      canvas.style.height = `${hgt}px`;
    }
    const ctx = canvas.getContext("2d");
    ctx.setTransform(dpr, 0, 0, dpr, 0, 0);
    ctx.clearRect(0, 0, w, hgt);
    return ctx;
  }

  // ---------- 共用 tooltip ----------
  class Tooltip {
    constructor(root) {
      this.el = h("div", "sys-tip");
      root.appendChild(this.el);
    }
    show(x, y, title, rows) {
      this.el.replaceChildren();
      this.el.appendChild(h("div", "sys-tip-title", title));
      for (const r of rows) {
        const row = h("div", "sys-tip-row");
        const key = h("span", "sys-key");
        key.style.background = r.color;
        row.append(key, h("span", "sys-tip-val", r.value), h("span", "sys-tip-name", r.name));
        this.el.appendChild(row);
      }
      this.el.classList.add("show");
      // 不要超出視窗
      const root = this.el.parentElement.getBoundingClientRect();
      const tw = this.el.offsetWidth;
      const th = this.el.offsetHeight;
      let left = x + 14;
      if (left + tw > root.width - 6) left = x - tw - 14;
      let top = Math.max(6, Math.min(y - th / 2, root.height - th - 6));
      this.el.style.transform = `translate(${Math.max(6, left)}px, ${top}px)`;
    }
    hide() {
      this.el.classList.remove("show");
    }
  }

  // ---------- 曲線圖 ----------
  class LineChart {
    /**
     * opts: series [{name, color(css var)}], yMax (固定最大值, 省略則自動),
     *       fmt (數值格式), height, refs [{v, label}](參考線), refFn(動態參考線)
     */
    constructor(card, tip, opts) {
      this.opts = { height: 150, ...opts };
      this.tip = tip;
      this.legend = h("div", "sys-legend");
      if (opts.series.length > 1) {
        for (const s of opts.series) {
          const item = h("span", "sys-legend-item");
          const key = h("span", "sys-key");
          key.style.background = `var(${s.color})`;
          item.append(key, document.createTextNode(s.name));
          this.legend.appendChild(item);
        }
        card.appendChild(this.legend);
      }
      this.wrap = h("div", "sys-canvas-wrap");
      this.canvas = h("canvas");
      this.wrap.appendChild(this.canvas);
      card.appendChild(this.wrap);
      this.times = [];
      this.values = opts.series.map(() => []);
      this.hover = null;
      this.canvas.addEventListener("pointermove", (e) => {
        this.hover = e.offsetX;
        this.draw();
      });
      this.canvas.addEventListener("pointerleave", () => {
        this.hover = null;
        this.tip.hide();
        this.draw();
      });
    }

    setData(times, values) {
      this.times = times;
      this.values = values;
      this.draw();
    }

    draw() {
      const W = this.wrap.clientWidth;
      const H = this.opts.height;
      if (W <= 0) return;
      const ctx = setupCanvas(this.canvas, W, H);
      const root = this.wrap;
      const grid = css(root, "--sys-grid");
      const muted = css(root, "--sys-text-muted");
      const surface = css(root, "--sys-card");
      const colors = this.opts.series.map((s) => css(root, s.color));
      const fmt = this.opts.fmt || ((v) => v.toFixed(0));

      const n = this.times.length;
      const tEnd = n ? this.times[n - 1] : Date.now();
      const tStart = tEnd - WINDOW_MS;

      // Y 範圍
      let max = this.opts.yMax;
      const refs = [...(this.opts.refs || []), ...(this.opts.refFn ? this.opts.refFn() : [])];
      if (max == null) {
        let m = 0;
        for (const vs of this.values) for (const v of vs) if (v > m) m = v;
        for (const r of refs) if (r.v > m && r.always) m = r.v;
        const step = niceStep(Math.max(m * 1.1, this.opts.minMax || 1), 3);
        max = Math.max(step * Math.ceil((m * 1.1) / step), step);
      }
      const step = this.opts.yStep || niceStep(max, 3);

      // 版面: 左側刻度寬度依標籤長度
      ctx.font = "11px -apple-system, BlinkMacSystemFont, 'Segoe UI', sans-serif";
      let labelW = 0;
      for (let v = 0; v <= max + 1e-9; v += step) labelW = Math.max(labelW, ctx.measureText(fmt(v)).width);
      const pad = { l: Math.ceil(labelW) + 10, r: 12, t: 10, b: 20 };
      const pw = W - pad.l - pad.r;
      const ph = H - pad.t - pad.b;
      const x = (t) => pad.l + ((t - tStart) / WINDOW_MS) * pw;
      const y = (v) => pad.t + ph - (Math.min(v, max) / max) * ph;

      // 格線與 Y 刻度
      ctx.lineWidth = 1;
      ctx.strokeStyle = grid;
      ctx.fillStyle = muted;
      ctx.textAlign = "right";
      ctx.textBaseline = "middle";
      for (let v = 0; v <= max + 1e-9; v += step) {
        const yy = Math.round(y(v)) + 0.5;
        ctx.beginPath();
        ctx.moveTo(pad.l, yy);
        ctx.lineTo(W - pad.r, yy);
        ctx.stroke();
        ctx.fillText(fmt(v), pad.l - 8, yy);
      }
      // X 刻度: 每分鐘
      ctx.textAlign = "center";
      ctx.textBaseline = "top";
      for (let k = 0; k <= 5; k++) {
        const t = tEnd - k * 60000;
        const label = k === 0 ? "現在" : `-${k} 分`;
        const xx = x(t);
        const tw = ctx.measureText(label).width;
        ctx.fillText(label, Math.min(Math.max(xx, pad.l + tw / 2), W - pad.r - tw / 2), H - pad.b + 6);
      }

      // 參考線(門檻)
      for (const r of refs) {
        if (r.v > max) continue;
        const yy = Math.round(y(r.v)) + 0.5;
        ctx.strokeStyle = css(root, r.color || "--sys-text-muted");
        ctx.globalAlpha = 0.55;
        ctx.setLineDash([4, 4]);
        ctx.beginPath();
        ctx.moveTo(pad.l, yy);
        ctx.lineTo(W - pad.r, yy);
        ctx.stroke();
        ctx.setLineDash([]);
        ctx.globalAlpha = 1;
        ctx.fillStyle = muted;
        ctx.textAlign = "left";
        // 太靠近頂端時, 標籤放在線的下方, 才不會被切掉
        // 放在下方時靠右, 避免和下一條參考線的標籤疊在一起
        if (yy - pad.t < 14) {
          ctx.textAlign = "right";
          ctx.textBaseline = "top";
          ctx.fillText(r.label, W - pad.r - 4, yy + 3);
        } else {
          ctx.textBaseline = "bottom";
          ctx.fillText(r.label, pad.l + 4, yy - 2);
        }
      }

      if (n < 2) return;
      const ptsOf = (vs) => {
        const pts = [];
        for (let i = 0; i < n; i++) if (this.times[i] >= tStart - 2000) pts.push([x(this.times[i]), y(vs[i])]);
        return pts;
      };

      // 區域漸層 + 曲線
      this.values.forEach((vs, si) => {
        const pts = ptsOf(vs);
        if (pts.length < 2) return;
        const c = colors[si];
        ctx.save();
        ctx.beginPath();
        ctx.rect(pad.l, pad.t - 2, pw, ph + 4);
        ctx.clip();
        // 漸層填色(單一系列較明顯, 多系列較淡)
        const grad = ctx.createLinearGradient(0, pad.t, 0, pad.t + ph);
        const a = this.values.length === 1 ? 0.28 : 0.1;
        grad.addColorStop(0, hexAlpha(c, a));
        grad.addColorStop(1, hexAlpha(c, 0));
        ctx.beginPath();
        smoothPath(ctx, pts);
        ctx.lineTo(pts[pts.length - 1][0], pad.t + ph);
        ctx.lineTo(pts[0][0], pad.t + ph);
        ctx.closePath();
        ctx.fillStyle = grad;
        ctx.fill();
        ctx.beginPath();
        smoothPath(ctx, pts);
        ctx.strokeStyle = c;
        ctx.lineWidth = 2;
        ctx.lineJoin = "round";
        ctx.lineCap = "round";
        ctx.stroke();
        ctx.restore();
        // 最新值的端點
        const [ex, ey] = pts[pts.length - 1];
        dot(ctx, ex, ey, c, surface);
      });

      // 十字線 + tooltip
      if (this.hover != null && this.hover >= pad.l && this.hover <= W - pad.r) {
        const t = tStart + ((this.hover - pad.l) / pw) * WINDOW_MS;
        let i = 0;
        let best = Infinity;
        for (let k = 0; k < n; k++) {
          const d = Math.abs(this.times[k] - t);
          if (d < best) {
            best = d;
            i = k;
          }
        }
        const xx = Math.round(x(this.times[i])) + 0.5;
        ctx.strokeStyle = muted;
        ctx.lineWidth = 1;
        ctx.beginPath();
        ctx.moveTo(xx, pad.t);
        ctx.lineTo(xx, pad.t + ph);
        ctx.stroke();
        this.values.forEach((vs, si) => dot(ctx, xx, y(vs[i]), colors[si], surface));
        const rows = this.opts.series.map((s, si) => ({ name: s.name, color: colors[si], value: fmt(this.values[si][i], true) }));
        const box = this.canvas.getBoundingClientRect();
        const rootBox = this.tip.el.parentElement.getBoundingClientRect();
        this.tip.show(
          box.left - rootBox.left + xx,
          box.top - rootBox.top + pad.t + ph / 2,
          `${fmtClock(this.times[i])} · ${fmtAgo(tEnd - this.times[i])}`,
          rows,
        );
      }
    }
  }

  function dot(ctx, x, y, color, surface) {
    ctx.beginPath();
    ctx.arc(x, y, 6, 0, Math.PI * 2);
    ctx.fillStyle = surface;
    ctx.fill();
    ctx.beginPath();
    ctx.arc(x, y, 4, 0, Math.PI * 2);
    ctx.fillStyle = color;
    ctx.fill();
  }

  function hexAlpha(color, a) {
    const m = color.match(/^#([0-9a-f]{6})$/i);
    if (!m) return color;
    const n = parseInt(m[1], 16);
    return `rgba(${(n >> 16) & 255}, ${(n >> 8) & 255}, ${n & 255}, ${a})`;
  }

  // ---------- 核心熱度圖 ----------
  class Heatmap {
    constructor(card, tip) {
      this.tip = tip;
      this.wrap = h("div", "sys-canvas-wrap");
      this.canvas = h("canvas");
      this.wrap.appendChild(this.canvas);
      card.appendChild(this.wrap);
      this.times = [];
      this.rows = [];
      this.hover = null;
      this.canvas.addEventListener("pointermove", (e) => {
        this.hover = [e.offsetX, e.offsetY];
        this.draw();
      });
      this.canvas.addEventListener("pointerleave", () => {
        this.hover = null;
        this.tip.hide();
        this.draw();
      });
    }
    setData(times, rows) {
      this.times = times;
      this.rows = rows;
      this.draw();
    }
    draw() {
      const W = this.wrap.clientWidth;
      const nRows = this.rows.length || 1;
      const rowH = 14;
      const gap = 2;
      const padL = 46;
      const padR = 12;
      const H = nRows * (rowH + gap) - gap + 4;
      if (W <= 0) return;
      const ctx = setupCanvas(this.canvas, W, H);
      const root = this.wrap;
      const base = css(root, "--sys-heat");
      const surface = css(root, "--sys-card");
      const muted = css(root, "--sys-text-muted");
      const n = this.times.length;
      const tEnd = n ? this.times[n - 1] : Date.now();
      const tStart = tEnd - WINDOW_MS;
      const pw = W - padL - padR;
      const cellW = pw / MAX_SAMPLES;

      ctx.font = "11px -apple-system, BlinkMacSystemFont, 'Segoe UI', sans-serif";
      ctx.textAlign = "right";
      ctx.textBaseline = "middle";
      this.rows.forEach((vals, r) => {
        const yy = 2 + r * (rowH + gap);
        ctx.fillStyle = muted;
        ctx.fillText(`核心 ${r}`, padL - 8, yy + rowH / 2);
        // 底色(0%)
        ctx.fillStyle = hexAlpha(base, 0.08);
        roundRect(ctx, padL, yy, pw, rowH, 4);
        ctx.fill();
        ctx.save();
        roundRect(ctx, padL, yy, pw, rowH, 4);
        ctx.clip();
        for (let i = 0; i < n; i++) {
          if (this.times[i] < tStart) continue;
          const xx = padL + ((this.times[i] - tStart) / WINDOW_MS) * pw;
          ctx.fillStyle = hexAlpha(base, 0.1 + 0.9 * Math.min(1, vals[i] / 100));
          ctx.fillRect(xx - cellW, yy, cellW + 0.6, rowH);
        }
        ctx.restore();
      });

      if (this.hover && n) {
        const [hx, hy] = this.hover;
        const r = Math.floor((hy - 2) / (rowH + gap));
        if (hx >= padL && hx <= W - padR && r >= 0 && r < this.rows.length) {
          const t = tStart + ((hx - padL) / pw) * WINDOW_MS;
          let i = 0;
          let best = Infinity;
          for (let k = 0; k < n; k++) {
            const d = Math.abs(this.times[k] - t);
            if (d < best) {
              best = d;
              i = k;
            }
          }
          const xx = padL + ((this.times[i] - tStart) / WINDOW_MS) * pw;
          const yy = 2 + r * (rowH + gap);
          ctx.strokeStyle = surface;
          ctx.lineWidth = 2;
          ctx.strokeRect(xx - cellW - 1, yy - 1, cellW + 2, rowH + 2);
          const box = this.canvas.getBoundingClientRect();
          const rootBox = this.tip.el.parentElement.getBoundingClientRect();
          this.tip.show(box.left - rootBox.left + xx, box.top - rootBox.top + yy + rowH / 2, `核心 ${r} · ${fmtAgo(tEnd - this.times[i])}`, [
            { name: "使用率", color: base, value: `${this.rows[r][i].toFixed(0)}%` },
          ]);
        } else {
          this.tip.hide();
        }
      }
    }
  }

  function roundRect(ctx, x, y, w, hgt, r) {
    ctx.beginPath();
    ctx.roundRect(x, y, w, hgt, r);
  }

  // ---------- 環形儀表 ----------
  function ring(label) {
    const el = h("div", "sys-ring");
    el.innerHTML = `
      <svg viewBox="0 0 120 120" aria-hidden="true">
        <circle class="track" cx="60" cy="60" r="50"/>
        <circle class="fill" cx="60" cy="60" r="50" pathLength="100" stroke-dasharray="0 100"/>
      </svg>
      <div class="sys-ring-center"><div class="sys-ring-value"><span class="num">–</span><span class="unit"></span></div></div>`;
    const meta = h("div", "sys-ring-meta");
    const name = h("div", "sys-ring-label", label);
    const sub = h("div", "sys-ring-sub", "");
    const badge = h("div", "sys-badge");
    meta.append(name, sub, badge);
    const box = h("div", "sys-ring-box");
    box.append(el, meta);
    return {
      el: box,
      set(value, unit, frac, sub_, st) {
        el.querySelector(".num").textContent = value;
        el.querySelector(".unit").textContent = unit;
        el.querySelector(".fill").setAttribute("stroke-dasharray", `${Math.max(0, Math.min(1, frac)) * 100} 100`);
        el.dataset.status = st;
        sub.textContent = sub_;
        badge.dataset.status = st;
        badge.textContent = st === "good" ? "" : `${st === "critical" ? "●" : "▲"} ${STATUS_TEXT[st]}`;
      },
    };
  }

  // ---------- 卡片 ----------
  function card(grid, title, cls) {
    const c = h("section", `sys-card ${cls || ""}`);
    const head = h("header", "sys-card-head");
    const t = h("h3", null, title);
    const right = h("div", "sys-card-now");
    head.append(t, right);
    c.appendChild(head);
    grid.appendChild(c);
    return { card: c, head, now: right };
  }

  function nowStat(container, items) {
    container.replaceChildren();
    for (const it of items) {
      const s = h("span", "sys-now-item");
      if (it.color) {
        const k = h("span", "sys-key");
        k.style.background = `var(${it.color})`;
        s.appendChild(k);
      }
      s.appendChild(h("b", null, it.value));
      if (it.label) s.appendChild(h("span", "sys-now-label", it.label));
      container.appendChild(s);
    }
  }

  // ---------- 掛載 ----------
  function mount(el) {
    el.classList.add("sys-root");
    const root = h("div", "sys-scroll");
    el.appendChild(root);
    const tip = new Tooltip(el);

    // 主機資訊列
    const infoBar = h("div", "sys-info");
    root.appendChild(infoBar);

    // 環形儀表
    const rings = h("div", "sys-rings");
    root.appendChild(rings);
    const rCpu = ring("CPU");
    const rMem = ring("記憶體");
    const rTemp = ring("溫度");
    const rDisk = ring("磁碟");
    rings.append(rCpu.el, rMem.el, rTemp.el, rDisk.el);

    const grid = h("div", "sys-grid");
    root.appendChild(grid);

    // CPU
    const cCpu = card(grid, "CPU 使用率", "wide");
    const chCpu = new LineChart(cCpu.card, tip, {
      series: [{ name: "CPU", color: "--sys-s1" }],
      yMax: 100,
      yStep: 25,
      fmt: (v) => `${Math.round(v)}%`,
      height: 170,
    });
    const coreHead = h("div", "sys-sub-head", "各核心");
    cCpu.card.appendChild(coreHead);
    const heat = new Heatmap(cCpu.card, tip);

    // 記憶體
    const cMem = card(grid, "記憶體");
    const chMem = new LineChart(cMem.card, tip, {
      series: [
        { name: "記憶體", color: "--sys-s1" },
        { name: "Swap", color: "--sys-s2" },
      ],
      yMax: 100,
      yStep: 50,
      fmt: (v) => `${Math.round(v)}%`,
    });

    // 溫度
    const cTemp = card(grid, "溫度");
    let chTemp = null;

    // 負載
    const cLoad = card(grid, "負載");
    let cores = 1;
    const chLoad = new LineChart(cLoad.card, tip, {
      series: [
        { name: "1 分", color: "--sys-s1" },
        { name: "5 分", color: "--sys-s2" },
        { name: "15 分", color: "--sys-s3" },
      ],
      fmt: (v, tipMode) => (tipMode ? v.toFixed(2) : `${+v.toFixed(1)}`),
      refFn: () => [{ v: cores, label: `${cores} 核心`, always: true }],
    });

    // 網路
    const cNet = card(grid, "網路");
    const sel = h("select", "sys-select");
    cNet.head.insertBefore(sel, cNet.now);
    const chNet = new LineChart(cNet.card, tip, {
      series: [
        { name: "下載", color: "--sys-s1" },
        { name: "上傳", color: "--sys-s2" },
      ],
      fmt: (v) => fmtRate(v),
      minMax: 1000,
    });
    let netIf = null;
    try {
      netIf = localStorage.getItem("cng6.sys.netif");
    } catch {
      /* 沒有 localStorage 也沒關係 */
    }
    sel.addEventListener("change", () => {
      netIf = sel.value;
      try {
        localStorage.setItem("cng6.sys.netif", netIf);
      } catch {
        /* 略過 */
      }
      render();
    });

    // 磁碟 I/O
    const cIo = card(grid, "磁碟 I/O");
    const chIo = new LineChart(cIo.card, tip, {
      series: [
        { name: "讀取", color: "--sys-s1" },
        { name: "寫入", color: "--sys-s2" },
      ],
      fmt: (v) => fmtRate(v),
      minMax: 1000,
    });

    // 磁碟空間
    const cDisk = card(grid, "磁碟空間");
    const diskList = h("div", "sys-disks");
    cDisk.card.appendChild(diskList);

    // ---- 資料 ----
    let es = null;
    let connected = false;
    let closed = false;
    let info = null;
    let samples = [];

    function renderInfo(last) {
      infoBar.replaceChildren();
      const items = [
        ["主機", info.host],
        ["系統", info.os],
        ["核心", `${info.kernel} · ${info.arch}`],
        ["處理器", `${info.cpu} × ${info.cores}`],
        ["開機", last ? fmtUptime(last.uptime) : "–"],
        ["程序", last ? String(last.procs) : "–"],
      ];
      for (const [k, v] of items) {
        const it = h("div", "sys-info-item");
        it.append(h("span", "k", k), h("span", "v", v));
        infoBar.appendChild(it);
      }
      const live = h("div", "sys-live");
      live.dataset.state = connected ? "on" : "off";
      live.append(h("span", "pulse"), document.createTextNode(connected ? "即時" : "重新連線中…"));
      infoBar.appendChild(live);
    }

    function pickNet(last) {
      const names = last.net.map((n) => n.name);
      if (sel.options.length !== names.length || [...sel.options].some((o, i) => o.value !== names[i])) {
        sel.replaceChildren(...names.map((n) => Object.assign(h("option", null, n), { value: n })));
      }
      if (!netIf || !names.includes(netIf)) {
        // 預設: 流量最大的實體網卡
        let best = null;
        let bestV = -1;
        for (const name of names) {
          if (/^(docker|br-|virbr)/.test(name)) continue;
          let v = 0;
          for (const s of samples) {
            const d = s.net.find((x) => x.name === name);
            if (d) v += d.rx + d.tx;
          }
          if (v > bestV) {
            bestV = v;
            best = name;
          }
        }
        netIf = best || names[0];
      }
      sel.value = netIf;
    }

    function render() {
      if (!info || !samples.length) return;
      const last = samples[samples.length - 1];
      const t = samples.map((s) => s.t);
      cores = info.cores;
      renderInfo(last);

      // 環形儀表
      rCpu.set(last.cpu.toFixed(0), "%", last.cpu / 100, `${info.cores} 核心 · ${info.cpu}`, status(last.cpu, 75, 90));
      const memPct = (last.mem_used / last.mem_total) * 100;
      rMem.set(memPct.toFixed(0), "%", memPct / 100, `${fmtGiB(last.mem_used)} / ${fmtGiB(last.mem_total)} GiB`, status(memPct, 80, 90));
      const hot = last.temps.reduce((a, b) => (b.c > (a ? a.c : -1) ? b : a), null);
      if (hot) rTemp.set(hot.c.toFixed(1), "°C", hot.c / 100, hot.name, status(hot.c, 65, 80));
      else rTemp.set("–", "", 0, "沒有溫度感測器", "good");
      const rootDisk = last.disks.find((d) => d.mount === "/") || last.disks[0];
      if (rootDisk) {
        const p = (rootDisk.used / rootDisk.total) * 100;
        rDisk.set(p.toFixed(0), "%", p / 100, `${fmtGiB(rootDisk.used)} / ${fmtGiB(rootDisk.total)} GiB · ${rootDisk.mount}`, status(p, 85, 95));
      }

      // CPU
      nowStat(cCpu.now, [{ value: `${last.cpu.toFixed(1)}%`, label: "目前" }]);
      chCpu.setData(t, [samples.map((s) => s.cpu)]);
      heat.setData(t, Array.from({ length: info.cores }, (_, c) => samples.map((s) => s.cores[c] ?? 0)));

      // 記憶體
      const swapPct = (s) => (s.swap_total ? (s.swap_used / s.swap_total) * 100 : 0);
      nowStat(cMem.now, [
        { color: "--sys-s1", value: `${memPct.toFixed(0)}%` },
        { color: "--sys-s2", value: `${swapPct(last).toFixed(0)}%` },
      ]);
      chMem.setData(t, [samples.map((s) => (s.mem_used / s.mem_total) * 100), samples.map(swapPct)]);

      // 溫度: 感測器數量決定系列
      const names = last.temps.map((x) => x.name).slice(0, 3);
      if (!chTemp || chTemp.names.join() !== names.join()) {
        cTemp.card.querySelectorAll(".sys-legend, .sys-canvas-wrap").forEach((e) => e.remove());
        chTemp = new LineChart(cTemp.card, tip, {
          series: names.map((n, i) => ({ name: n.replace(/ temp\d+$/, ""), color: `--sys-s${i + 1}` })),
          fmt: (v, tipMode) => (tipMode ? `${v.toFixed(1)}°C` : `${Math.round(v)}°`),
          refs: [
            { v: 65, label: "65° 偏高", color: "--sys-warning" },
            { v: 80, label: "80° 過高", color: "--sys-critical" },
          ],
          minMax: 50,
        });
        chTemp.opts.yMax = null;
        chTemp.names = names;
      }
      // 溫度範圍: 一定看得到 80° 門檻
      const tMax = Math.max(...samples.flatMap((s) => s.temps.map((x) => x.c)), 0);
      chTemp.opts.yMax = tMax < 76 ? 80 : 100;
      chTemp.opts.yStep = 20;
      nowStat(
        cTemp.now,
        last.temps.slice(0, 3).map((x, i) => ({ color: names.length > 1 ? `--sys-s${i + 1}` : null, value: `${x.c.toFixed(1)}°C` })),
      );
      chTemp.setData(
        t,
        names.map((n) => samples.map((s) => (s.temps.find((x) => x.name === n) || { c: 0 }).c)),
      );

      // 負載
      nowStat(
        cLoad.now,
        last.load.map((v, i) => ({ color: `--sys-s${i + 1}`, value: v.toFixed(2) })),
      );
      chLoad.setData(t, [0, 1, 2].map((k) => samples.map((s) => s.load[k])));

      // 網路
      pickNet(last);
      const netOf = (s) => s.net.find((x) => x.name === netIf) || { rx: 0, tx: 0 };
      const ln = netOf(last);
      nowStat(cNet.now, [
        { color: "--sys-s1", value: fmtRate(ln.rx) },
        { color: "--sys-s2", value: fmtRate(ln.tx) },
      ]);
      chNet.setData(t, [samples.map((s) => netOf(s).rx), samples.map((s) => netOf(s).tx)]);

      // 磁碟 I/O
      nowStat(cIo.now, [
        { color: "--sys-s1", value: fmtRate(last.disk_read) },
        { color: "--sys-s2", value: fmtRate(last.disk_write) },
      ]);
      chIo.setData(t, [samples.map((s) => s.disk_read), samples.map((s) => s.disk_write)]);

      // 磁碟空間
      diskList.replaceChildren();
      for (const d of last.disks) {
        const p = (d.used / d.total) * 100;
        const st = status(p, 85, 95);
        const row = h("div", "sys-disk");
        const top = h("div", "sys-disk-top");
        top.append(h("span", "sys-disk-mount", d.mount), h("span", "sys-disk-val", `${fmtGiB(d.used)} / ${fmtGiB(d.total)} GiB`), h("b", null, `${p.toFixed(0)}%`));
        const meter = h("div", "sys-meter");
        meter.dataset.status = st;
        const fill = h("div", "sys-meter-fill");
        fill.style.width = `${p}%`;
        meter.appendChild(fill);
        row.append(top, meter);
        diskList.appendChild(row);
      }
    }

    // 視窗大小改變時重畫
    const ro = new ResizeObserver(() => render());
    ro.observe(el);
    // 切換外觀: 圖表顏色馬上更新
    const onTheme = () => render();
    window.addEventListener("cng6-theme", onTheme);

    // ---- 連線 ----

    async function load() {
      try {
        const r = await fetch("/api/system", { cache: "no-store" });
        const d = await r.json();
        info = d.info;
        samples = d.history.slice(-MAX_SAMPLES);
        render();
      } catch {
        /* 等 EventSource 重新連線時再試 */
      }
    }

    function connect() {
      es = new EventSource("/api/system/stream");
      es.onopen = () => {
        const wasOff = !connected;
        connected = true;
        // 斷線後重新連上: 補回中間缺少的資料
        if (wasOff) load();
      };
      es.onmessage = (e) => {
        const s = JSON.parse(e.data);
        if (samples.length && s.t <= samples[samples.length - 1].t) return;
        samples.push(s);
        if (samples.length > MAX_SAMPLES) samples.shift();
        render();
      };
      es.onerror = () => {
        connected = false;
        if (info) render();
      };
    }

    load().then(() => {
      if (!closed) connect();
    });

    return () => {
      closed = true;
      ro.disconnect();
      window.removeEventListener("cng6-theme", onTheme);
      if (es) es.close();
    };
  }

  return { mount };
})();

// cng6 Cluster: 群組裡每台機器的狀態(PC 視窗與手機共用)
"use strict";

const ClusterApp = (() => {
  function h(tag, cls, text) {
    const el = document.createElement(tag);
    if (cls) el.className = cls;
    if (text != null) el.textContent = text;
    return el;
  }
  function bytes(b) {
    const u = ["KB", "MB", "GB", "TB"];
    let v = b / 1024;
    let i = 0;
    while (v >= 1024 && i < u.length - 1) {
      v /= 1024;
      i++;
    }
    return `${v < 10 ? v.toFixed(1) : Math.round(v)} ${u[i]}`;
  }
  function dur(s) {
    s = Math.max(0, Math.round(s));
    const d = Math.floor(s / 86400);
    const hr = Math.floor((s % 86400) / 3600);
    const m = Math.floor((s % 3600) / 60);
    if (d) return `${d} 天 ${hr} 小時`;
    if (hr) return `${hr} 小時 ${m} 分`;
    if (m) return `${m} 分鐘`;
    return `${s} 秒`;
  }
  const pct = (u, t) => (t ? (u * 100) / t : 0);
  const level = (p) => (p >= 90 ? "crit" : p >= 75 ? "warn" : "ok");

  function meter(label, p, text) {
    const m = h("div", "cl-meter");
    m.append(h("span", "cl-meter-label", label));
    const bar = h("div", `cl-bar ${level(p)}`);
    const fill = h("i");
    fill.style.width = `${Math.min(100, Math.max(0, p)).toFixed(1)}%`;
    bar.append(fill);
    m.append(bar, h("span", "cl-meter-text", text));
    return m;
  }

  function mount(el, { device = "pc" } = {}) {
    el.classList.add("cl-root", device === "mobile" ? "cl-mobile" : "cl-pc");
    el.innerHTML = `
      <header class="cl-head">
        <div class="cl-self"></div>
        <div class="cl-conn"></div>
      </header>
      <div class="cl-grid"></div>`;
    const selfEl = el.querySelector(".cl-self");
    const connEl = el.querySelector(".cl-conn");
    const grid = el.querySelector(".cl-grid");
    let timer = null;
    let stopped = false;

    function card(v, now) {
      const n = v;
      const i = n.info;
      const state = n.me ? "me" : !n.online ? "off" : v.stale ? "stale" : "on";
      const c = h("article", `cl-card ${state}`);
      const head = h("div", "cl-card-head");
      head.append(h("span", "cl-dot"), h("b", "cl-name", n.name), h("span", `cl-role ${n.role}`, n.role === "server" ? "Server" : "Client"));
      if (n.me) head.append(h("span", "cl-me", "這台"));
      c.append(head);
      const ago = dur(now - n.last_seen);
      const stText = n.me ? "本機" : !n.online ? `離線 · ${ago}前` : v.stale ? `沒有回應 · ${ago}` : "上線";
      c.append(h("div", "cl-state", stText));
      if (!i) {
        c.append(h("div", "cl-empty", "還沒有收到資料"));
        return c;
      }
      const live = n.me || (n.online && !v.stale);
      const meters = h("div", `cl-meters ${live ? "" : "dim"}`);
      meters.append(
        meter("CPU", i.cpu_usage, `${Math.round(i.cpu_usage)}%`),
        meter("記憶體", pct(i.mem_used, i.mem_total), `${bytes(i.mem_used)} / ${bytes(i.mem_total)}`),
        meter("磁碟", pct(i.disk_used, i.disk_total), `${bytes(i.disk_used)} / ${bytes(i.disk_total)}`)
      );
      c.append(meters);
      const dl = h("dl", "cl-facts");
      const add = (k, val) => val && dl.append(h("dt", null, k), h("dd", null, val));
      add("負載", i.load.map((x) => x.toFixed(2)).join("  "));
      if (i.temp != null) add("溫度", `${i.temp.toFixed(1)}°C`);
      add("開機", dur(i.uptime));
      add("cng6", `${i.version} · 執行 ${dur(now - i.started)}`);
      add("系統", `${i.os} · ${i.kernel}`);
      add("CPU", `${i.cpu || i.arch} ×${i.cores}`);
      add("IP", i.ips.join(", "));
      c.append(dl);
      return c;
    }

    async function load() {
      let s;
      try {
        s = await (await fetch("/api/cluster", { cache: "no-store" })).json();
      } catch {
        connEl.textContent = "無法讀取";
        connEl.dataset.state = "err";
        return;
      }
      selfEl.replaceChildren();
      if (s.role === "off") {
        selfEl.append(h("b", null, "Cluster 未啟用"));
      } else {
        selfEl.append(h("b", null, s.name), h("span", `cl-role ${s.role}`, s.role === "server" ? "Server" : "Client"));
        if (s.role === "client") selfEl.append(h("span", "cl-arrow", `→ ${s.server || "?"}`));
      }
      if (s.role === "off" || s.idle) {
        connEl.textContent = "";
        connEl.dataset.state = "off";
      } else if (s.connected) {
        connEl.textContent = `已連線 ${s.broker}`;
        connEl.dataset.state = "ok";
      } else {
        connEl.textContent = s.error ? `無法連線: ${s.error}` : `連線中 ${s.broker}…`;
        connEl.dataset.state = s.error ? "err" : "wait";
      }
      grid.replaceChildren();
      if (s.role === "off" || s.idle) {
        const help = h("div", "cl-help");
        help.append(h("h3", null, s.idle || "設定角色後開始"));
        const steps = [
          ["每台設定同一把金鑰", "cluster genkey 產生, 再 settings set cluster.key <金鑰>"],
          ["一台當 server", "settings set cluster.role server(名稱預設是 hostname)"],
          ["其他台當 client", "settings set cluster.role client 與 settings set cluster.server <server 名稱>"],
        ];
        const ol = h("ol");
        for (const [t, d] of steps) {
          const li = h("li");
          li.append(h("b", null, t), h("code", null, d));
          ol.append(li);
        }
        help.append(ol);
        grid.append(help);
        return;
      }
      for (const v of s.nodes) grid.append(card(v, s.now));
      if (s.nodes.length <= 1) grid.append(h("div", "cl-wait", s.role === "server" ? "等待 client 加入…" : `等待 server「${s.server}」…`));
    }

    async function loop() {
      if (stopped) return;
      if (!document.hidden) await load();
      timer = setTimeout(loop, 3000);
    }
    loop();
    return () => {
      stopped = true;
      clearTimeout(timer);
    };
  }

  return { mount };
})();

if (typeof Apps !== "undefined") Apps.registerView("cluster", ClusterApp);

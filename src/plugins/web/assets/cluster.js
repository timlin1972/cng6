// cng6 Cluster: 群組裡每台機器的狀態、通知、共用剪貼簿,
// 點一台機器可以用它的 CLI、管理它的檔案(透過加密的 MQTT 轉送)
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
  const nodeBase = (name) => `/api/cluster/node/${encodeURIComponent(name)}`;

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

  /** 一台機器的狀態卡片 */
  /** 一台機器的狀態卡片; full = 完整資料(點進去的「狀態」), 否則只有重點 */
  function nodeCard(n, now, { clickable = false, full = false } = {}) {
    const i = n.info;
    const state = n.me ? "me" : !n.online ? "off" : n.stale ? "stale" : "on";
    const c = h("article", `cl-card ${state} ${clickable ? "clickable" : ""}`);
    const head = h("div", "cl-card-head");
    head.append(h("span", "cl-dot"), h("b", "cl-name", n.name), h("span", `cl-role ${n.role}`, n.role === "server" ? "Server" : "Client"));
    if (n.me) head.append(h("span", "cl-me", clickable ? "這台 ›" : "這台"));
    else if (clickable) head.append(h("span", "cl-open", reachable(n) ? "詳細 · 指令 · 檔案 ›" : "詳細 ›"));
    c.append(head);
    const ago = dur(now - n.last_seen);
    c.append(h("div", "cl-state", n.me ? "本機" : !n.online ? `離線 · ${ago}前` : n.stale ? `沒有回應 · ${ago}` : "上線"));
    if (!i) {
      c.append(h("div", "cl-empty", "還沒有收到資料"));
      return c;
    }
    const live = n.me || (n.online && !n.stale);
    const meters = h("div", `cl-meters ${live ? "" : "dim"}`);
    meters.append(
      meter("CPU", i.cpu_usage, `${Math.round(i.cpu_usage)}%`),
      meter("記憶體", pct(i.mem_used, i.mem_total), `${bytes(i.mem_used)} / ${bytes(i.mem_total)}`),
      meter("磁碟", pct(i.disk_used, i.disk_total), `${bytes(i.disk_used)} / ${bytes(i.disk_total)}`)
    );
    c.append(meters);
    if (!full) {
      // 重點: 溫度 · 開機 · 負載, 其他點進去看
      const bits = [];
      if (i.temp != null) bits.push(`${i.temp.toFixed(0)}°C`);
      bits.push(`開機 ${dur(i.uptime)}`);
      bits.push(`負載 ${i.load[0].toFixed(2)}`);
      c.append(h("div", `cl-summary ${live ? "" : "dim"}`, bits.join(" · ")));
      return c;
    }
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

  /** 可以用它的指令與檔案(上線中的別台) */
  const reachable = (n) => !n.me && n.online && !n.stale;

  // ---------------------------------------------------------------- 遠端 CLI
  function terminal(host, name) {
    host.classList.add("cl-term");
    const out = h("div", "cl-term-out");
    const row = h("form", "cl-term-row");
    const prompt = h("span", "cl-term-prompt", `${name}>`);
    const input = h("input", "cl-term-input");
    input.placeholder = "輸入指令, 例如 system info、help";
    input.spellcheck = false;
    input.setAttribute("autocapitalize", "off");
    input.setAttribute("autocomplete", "off");
    row.append(prompt, input);
    host.append(out, row);
    const session = `cluster-web-${Math.random().toString(36).slice(2, 10)}`;
    const history = [];
    let hi = 0;
    const line = (text, cls = "") => {
      out.append(h("div", `cl-term-line ${cls}`, text));
      out.scrollTop = out.scrollHeight;
    };
    line(`連到 ${name}(透過加密的 MQTT)。指令和本機的 CLI 一樣。`, "dim");
    async function run(cmd) {
      line(`${prompt.textContent} ${cmd}`, "cmd");
      input.disabled = true;
      try {
        const r = await fetch(`${nodeBase(name)}/api/cli`, {
          method: "POST",
          headers: { "content-type": "application/json" },
          body: JSON.stringify({ session, op: "exec", line: cmd }),
        });
        if (!r.ok) throw new Error((await r.text()) || r.statusText);
        const d = await r.json();
        for (const o of d.output) line(o.text, o.level);
        prompt.textContent = `${name}${d.path?.length ? " " + d.path.join(" ") : ""}>`;
      } catch (e) {
        line(e.message, "error");
      }
      input.disabled = false;
      input.focus();
    }
    row.addEventListener("submit", (e) => {
      e.preventDefault();
      const cmd = input.value.trim();
      if (!cmd) return;
      history.push(cmd);
      hi = history.length;
      input.value = "";
      run(cmd);
    });
    input.addEventListener("keydown", (e) => {
      if (e.key === "ArrowUp" && hi > 0) {
        input.value = history[--hi];
        e.preventDefault();
      } else if (e.key === "ArrowDown") {
        hi = Math.min(history.length, hi + 1);
        input.value = history[hi] || "";
        e.preventDefault();
      }
    });
    setTimeout(() => input.focus(), 50);
  }

  // ---------------------------------------------------------------- 主畫面
  function mount(el, { device = "pc" } = {}) {
    const mobile = device === "mobile";
    el.classList.add("cl-root", mobile ? "cl-mobile" : "cl-pc");
    el.innerHTML = `
      <div class="cl-main">
        <header class="cl-head">
          <div class="cl-self"></div>
          <div class="cl-conn"></div>
        </header>
        <div class="cl-body">
        <section class="cl-alerts" hidden></section>
        <div class="cl-grid"></div>
        <section class="cl-clip" hidden>
          <h3>共用剪貼簿</h3>
          <form class="cl-clip-form">
            <textarea class="cl-clip-input" rows="2" placeholder="輸入文字或網址, 送出後每台都收得到"></textarea>
            <button class="cl-btn primary">送出</button>
          </form>
          <div class="cl-clip-list"></div>
        </section>
        </div>
      </div>`;
    const selfEl = el.querySelector(".cl-self");
    const connEl = el.querySelector(".cl-conn");
    const grid = el.querySelector(".cl-grid");
    const alertsEl = el.querySelector(".cl-alerts");
    const clipEl = el.querySelector(".cl-clip");
    const clipList = el.querySelector(".cl-clip-list");
    const clipInput = el.querySelector(".cl-clip-input");
    let timer = null;
    let stopped = false;
    let detail = null; // 打開的機器 { name, close }
    let lastClips = "";

    function toast(text, ok = true) {
      let t = el.querySelector(".cl-toast");
      if (!t) {
        t = h("div", "cl-toast");
        el.append(t);
      }
      t.textContent = text;
      t.dataset.ok = ok;
      t.classList.add("show");
      clearTimeout(t._timer);
      t._timer = setTimeout(() => t.classList.remove("show"), 2400);
    }

    el.querySelector(".cl-clip-form").addEventListener("submit", async (e) => {
      e.preventDefault();
      const text = clipInput.value;
      if (!text.trim()) return;
      try {
        const r = await fetch("/api/cluster/clip", {
          method: "POST",
          headers: { "content-type": "application/json" },
          body: JSON.stringify({ text }),
        });
        if (!r.ok) throw new Error(await r.text());
        clipInput.value = "";
        toast("已送到共用剪貼簿");
        setTimeout(load, 600);
      } catch (err) {
        toast(err.message, false);
      }
    });
    clipInput.addEventListener("keydown", (e) => {
      if (e.key === "Enter" && (e.ctrlKey || e.metaKey)) el.querySelector(".cl-clip-form").requestSubmit();
    });

    function renderClips(s) {
      const key = JSON.stringify(s.clips);
      if (key === lastClips) return;
      lastClips = key;
      clipList.replaceChildren();
      if (!s.clips.length) clipList.append(h("div", "cl-empty", "還沒有內容"));
      for (const c of s.clips) {
        const item = h("div", "cl-clip-item");
        const meta = h("div", "cl-clip-meta", `${c.from} · ${dur(s.now - c.ts)}前`);
        const text = h("div", "cl-clip-text", c.text);
        const copy = h("button", "cl-btn small", "複製");
        copy.addEventListener("click", async () => {
          try {
            await navigator.clipboard.writeText(c.text);
            toast("已複製");
          } catch {
            // 沒有剪貼簿權限: 選取文字讓使用者自己複製
            const r = document.createRange();
            r.selectNodeContents(text);
            getSelection().removeAllRanges();
            getSelection().addRange(r);
            toast("請按 Ctrl+C / 長按複製", false);
          }
        });
        item.append(meta, copy, text);
        if (/^https?:\/\/\S+$/.test(c.text.trim())) {
          const a = h("a", "cl-btn small", "開啟");
          a.href = c.text.trim();
          a.target = "_blank";
          a.rel = "noopener";
          item.insertBefore(a, text);
        }
        clipList.append(item);
      }
    }

    function renderAlerts(s) {
      const recent = s.alerts.filter((a) => s.now - a.ts < 86400).slice(0, 8);
      alertsEl.hidden = !s.alerts_active.length && !recent.length;
      if (alertsEl.hidden) return;
      alertsEl.replaceChildren(h("h3", null, "通知"));
      const list = h("div", "cl-alert-list");
      const shown = new Set();
      for (const a of s.alerts_active) {
        shown.add(`${a.node}:${a.kind}`);
        const r = h("div", "cl-alert active");
        r.append(h("b", null, a.node), h("span", null, a.text), h("time", null, `${dur(s.now - a.ts)}前`));
        list.append(r);
      }
      for (const a of recent) {
        if (a.active && shown.has(`${a.node}:${a.kind}`)) continue;
        const r = h("div", "cl-alert");
        r.append(h("b", null, a.node), h("span", null, `${a.text}(已恢復)`), h("time", null, `${dur(s.now - a.ts)}前`));
        list.append(r);
      }
      alertsEl.append(list);
    }

    // ---- 一台機器的詳細畫面: 狀態 / 指令 / 終端機 / 檔案
    function openNode(n, s) {
      closeNode();
      const wrap = h("div", "cl-detail");
      const bar = h("header", "cl-detail-bar");
      const back = h("button", "cl-btn", mobile ? "‹ 返回" : "‹ 所有機器");
      back.dataset.edgeBack = "";
      const title = h("b", null, n.name);
      const tabs = h("nav", "cl-tabs");
      bar.append(back, title, tabs);
      const body = h("div", "cl-detail-body");
      wrap.append(bar, body);
      el.append(wrap);
      let cleanup = null;
      const show = (tab) => {
        if (cleanup) cleanup();
        cleanup = null;
        body.replaceChildren();
        body.className = `cl-detail-body tab-${tab}`;
        tabs.querySelectorAll("button").forEach((b) => b.classList.toggle("on", b.dataset.tab === tab));
        if (tab === "status") {
          const cur = (lastSnap?.nodes || s.nodes).find((x) => x.name === n.name) || n;
          body.append(nodeCard(cur, lastSnap?.now || s.now, { full: true }));
        } else if (tab === "cli") {
          terminal(body, n.name);
        } else if (tab === "files") {
          const host = h("div", "cl-files");
          body.append(host);
          cleanup = FilesApp.mount(host, { device, base: nodeBase(n.name), node: n.name });
        } else if (tab === "term") {
          const host = h("div", "cl-shell");
          body.append(host);
          cleanup = TermApp.mount(host, { device, base: nodeBase(n.name), node: n.name });
        }
      };
      // 這台、離線的機器只有狀態
      const list = reachable(n) && s.connected ? [["status", "狀態"], ["cli", "指令"], ["term", "終端機"], ["files", "檔案"]] : [["status", "狀態"]];
      tabs.hidden = list.length < 2;
      for (const [k, label] of list) {
        const b = h("button", "cl-tab", label);
        b.dataset.tab = k;
        b.addEventListener("click", () => show(k));
        tabs.append(b);
      }
      back.addEventListener("click", closeNode);
      detail = {
        name: n.name,
        close() {
          if (cleanup) cleanup();
          wrap.remove();
        },
      };
      show("status");
    }
    function closeNode() {
      if (detail) detail.close();
      detail = null;
    }

    let lastSnap = null;
    async function load() {
      let s;
      try {
        s = await (await fetch("/api/cluster", { cache: "no-store" })).json();
      } catch {
        connEl.textContent = "無法讀取";
        connEl.dataset.state = "err";
        return;
      }
      lastSnap = s;
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
      const active = s.role !== "off" && !s.idle;
      clipEl.hidden = !active;
      if (!active) {
        alertsEl.hidden = true;
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
      renderAlerts(s);
      renderClips(s);
      for (const n of s.nodes) {
        const card = nodeCard(n, s.now, { clickable: !!n.info });
        if (n.info) card.addEventListener("click", () => openNode(n, s));
        grid.append(card);
      }
      if (s.nodes.length <= 1) grid.append(h("div", "cl-wait", s.role === "server" ? "等待 client 加入…" : `等待 server「${s.server}」…`));
    }

    async function loop() {
      if (stopped) return;
      if (!document.hidden && !detail) await load();
      timer = setTimeout(loop, 3000);
    }
    loop();
    return () => {
      stopped = true;
      clearTimeout(timer);
      closeNode();
    };
  }

  return { mount };
})();

if (typeof Apps !== "undefined") Apps.registerView("cluster", ClusterApp);

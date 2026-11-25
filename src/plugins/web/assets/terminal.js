// cng6 終端機(PC 視窗與手機共用)
//
// - xterm.js 畫終端機, shell 在伺服器的 PTY 裡
// - 用一般的 HTTP 溝通: 長輪詢讀輸出、POST 送鍵盤輸入, 所以 cluster 代理就能開別台的
// - 關掉視窗不會結束 shell, 下次打開會接回最近的那個
// - 手機: 下方多一列按鍵(Esc、Tab、Ctrl、方向鍵、常用符號)
"use strict";

const TermApp = (() => {
  function h(tag, cls, text) {
    const el = document.createElement(tag);
    if (cls) el.className = cls;
    if (text != null) el.textContent = text;
    return el;
  }

  // xterm.js 用到才載入(300 KB)
  let loading = null;
  function loadXterm() {
    if (window.Terminal && window.FitAddon) return Promise.resolve();
    if (loading) return loading;
    const script = (src) =>
      new Promise((ok, fail) => {
        const s = document.createElement("script");
        s.src = src;
        s.onload = ok;
        s.onerror = () => fail(new Error(`無法載入 ${src}`));
        document.head.append(s);
      });
    const css = document.createElement("link");
    css.rel = "stylesheet";
    css.href = "/xterm/xterm.css";
    document.head.append(css);
    loading = script("/xterm/xterm.js").then(() => script("/xterm/addon-fit.js"));
    loading.catch(() => (loading = null));
    return loading;
  }

  function b64bytes(s) {
    const bin = atob(s);
    const out = new Uint8Array(bin.length);
    for (let i = 0; i < bin.length; i++) out[i] = bin.charCodeAt(i);
    return out;
  }

  // 手機的按鍵列: [顯示, 送出的字元]; ctrl 是黏著的修飾鍵(下一個字變成 Ctrl-字)
  const KEYS = [
    ["Esc", "\x1b"],
    ["Tab", "\t"],
    ["Ctrl", "ctrl"],
    ["↑", "\x1b[A"],
    ["↓", "\x1b[B"],
    ["←", "\x1b[D"],
    ["→", "\x1b[C"],
    ["|", "|"],
    ["/", "/"],
    ["-", "-"],
    ["~", "~"],
    ["^C", "\x03"],
  ];

  // base / node: 固定開某一台(例如 Cluster 裡那台機器的「終端機」分頁), 不顯示機器選單
  function mount(el, { device = "pc", base: fixedBase = null, node = null } = {}) {
    const mobile = device === "mobile";
    let base = fixedBase ?? "";
    let term = null;
    let fit = null;
    let sid = null; // 目前接著的終端機
    let gen = 0; // 換終端機時讓舊的輪詢停下來
    let closed = false;
    let ctrl = false;

    const root = h("div", "tm-root");
    root.dataset.device = device;
    root.innerHTML = `
      <div class="tm-top">
        <select class="tm-node" title="哪一台機器" hidden></select>
        <select class="tm-sess" title="終端機"></select>
        <button class="tm-btn tm-new" title="開新的終端機">＋</button>
        <button class="tm-btn tm-kill" title="結束這個終端機">結束</button>
        <span class="tm-status"></span>
      </div>
      <div class="tm-screen"></div>
      <div class="tm-keys" ${mobile ? "" : "hidden"}></div>`;
    el.replaceChildren(root);
    const $ = (s) => root.querySelector(s);
    const nodeSel = $(".tm-node");
    const sessSel = $(".tm-sess");
    const screen = $(".tm-screen");
    const status = $(".tm-status");

    const api = (p) => `${base}/api/term${p}`;
    async function req(url, opts) {
      const r = await fetch(url, opts);
      let d = null;
      try {
        d = await r.json();
      } catch {}
      if (!r.ok) throw new Error((d && d.error) || `${r.status} ${r.statusText}`);
      return d;
    }
    const post = (url, body) => req(url, { method: "POST", headers: { "content-type": "application/json" }, body: JSON.stringify(body) });

    function setStatus(text, bad = false) {
      status.textContent = text;
      status.classList.toggle("bad", bad);
    }

    // ---- 機器 ----
    async function loadNodes() {
      if (fixedBase !== null) return;
      try {
        const s = await req("/api/cluster");
        const others = (s.nodes || []).filter((n) => !n.me && n.online && !n.stale);
        if (!others.length) return;
        nodeSel.replaceChildren(new Option(`這台 (${s.name})`, ""));
        for (const n of others) nodeSel.append(new Option(n.name, `/api/cluster/node/${encodeURIComponent(n.name)}`));
        nodeSel.hidden = false;
      } catch {}
    }
    nodeSel.onchange = () => {
      base = nodeSel.value;
      start();
    };

    // ---- 終端機清單 ----
    async function refreshSessions(select) {
      const d = await req(api(""));
      sessSel.replaceChildren(
        ...d.sessions.map((s) => {
          const t = new Date(s.created * 1000);
          const hm = `${String(t.getHours()).padStart(2, "0")}:${String(t.getMinutes()).padStart(2, "0")}`;
          return new Option(`#${s.id} · ${hm}${s.ended ? " (已結束)" : ""}`, s.id);
        }),
      );
      if (select != null) sessSel.value = select;
      return d;
    }
    sessSel.onchange = () => attach(Number(sessSel.value));

    async function openNew() {
      const { cols, rows } = term;
      const r = await post(api(""), { cols, rows });
      await refreshSessions(r.id);
      attach(r.id);
    }
    $(".tm-new").onclick = () => openNew().catch((e) => setStatus(e.message, true));
    $(".tm-kill").onclick = async () => {
      if (sid == null) return;
      try {
        await req(api(`/${sid}`), { method: "DELETE" });
      } catch {}
      const d = await refreshSessions().catch(() => null);
      const live = d?.sessions.filter((s) => !s.ended) || [];
      if (live.length) attach(live[live.length - 1].id, true);
      else openNew().catch((e) => setStatus(e.message, true));
    };

    // ---- 接上一個終端機: 從頭讀輸出(畫面會回到之前的樣子), 之後長輪詢 ----
    async function attach(id, select = false) {
      const my = ++gen;
      sid = id;
      if (select) sessSel.value = id;
      term.reset();
      setStatus("");
      sendSize(true);
      let from = 0;
      let fails = 0;
      while (!closed && my === gen) {
        try {
          const r = await req(api(`/${id}/out?from=${from}`));
          if (my !== gen) return;
          fails = 0;
          if (r.data) term.write(b64bytes(r.data));
          from = r.next;
          if (r.ended) {
            term.write("\r\n\x1b[2m[已結束, 按 ＋ 開新的]\x1b[0m\r\n");
            setStatus("已結束");
            return;
          }
          setStatus("");
        } catch (e) {
          if (my !== gen) return;
          if (/不在了/.test(e.message)) {
            setStatus("已結束", true);
            return;
          }
          fails++;
          setStatus(`連線中斷, 重試中…`, true);
          await new Promise((r) => setTimeout(r, Math.min(5000, 500 * fails)));
        }
      }
    }

    // ---- 鍵盤輸入: 同時只送一個請求, 送出期間打的字合在一起下次送 ----
    // (遠端時每個請求都要經過 MQTT broker, 一個字一個請求會被限流, 反而很慢)
    // 序號讓伺服器依順序寫入; 失敗時用同一個序號重送
    const client = Math.random().toString(36).slice(2);
    let seq = 0;
    let queue = "";
    let sending = false;
    function send(data) {
      if (sid == null) return;
      if (ctrl && data.length === 1) {
        const c = data.toUpperCase().charCodeAt(0);
        if (c >= 64 && c <= 95) data = String.fromCharCode(c - 64);
        setCtrl(false);
      }
      queue += data;
      flush();
    }
    async function flush() {
      if (sending) return;
      sending = true;
      while (queue && !closed && sid != null) {
        const body = { data: queue, client: `${client}-${sid}`, seq: seq++ };
        queue = "";
        const id = sid;
        for (let i = 0; i < 4; i++) {
          try {
            await post(api(`/${id}/in`), body);
            break;
          } catch (e) {
            if (/不在了/.test(e.message)) break;
            if (i === 3) setStatus(`送出失敗: ${e.message}`, true);
            else await new Promise((r) => setTimeout(r, 300 * (i + 1)));
          }
        }
      }
      sending = false;
    }

    // ---- 大小 ----
    let sizeTimer = null;
    let lastSize = "";
    function sendSize(force = false) {
      clearTimeout(sizeTimer);
      sizeTimer = setTimeout(() => {
        if (sid == null || !term) return;
        const key = `${term.cols}x${term.rows}`;
        if (!force && key === lastSize) return;
        lastSize = key;
        post(api(`/${sid}/resize`), { cols: term.cols, rows: term.rows }).catch(() => {});
      }, force ? 0 : 200);
    }
    const ro = new ResizeObserver(() => {
      if (!fit || !screen.offsetWidth) return;
      try {
        fit.fit();
      } catch {}
      sendSize();
    });

    // ---- 手機按鍵列 ----
    function setCtrl(on) {
      ctrl = on;
      root.querySelector('.tm-key[data-k="ctrl"]')?.classList.toggle("on", on);
    }
    if (mobile) {
      const keys = $(".tm-keys");
      for (const [label, k] of KEYS) {
        const b = h("button", "tm-key", label);
        b.dataset.k = k;
        // 不要搶走 xterm 的焦點(鍵盤會收起來)
        b.addEventListener("pointerdown", (e) => e.preventDefault());
        b.onclick = () => {
          if (k === "ctrl") setCtrl(!ctrl);
          else send(k);
          term?.focus();
        };
        keys.append(b);
      }
    }

    async function start() {
      gen++;
      sid = null;
      try {
        const d = await refreshSessions();
        if (!d.enabled) {
          term.reset();
          term.write("這台機器關閉了終端機功能(terminal.enabled = false)\r\n");
          setStatus("已關閉", true);
          return;
        }
        // 接回最近一個還在跑的, 沒有就開新的
        const live = d.sessions.filter((s) => !s.ended);
        if (live.length) attach(live[live.length - 1].id, true);
        else await openNew();
      } catch (e) {
        term.reset();
        term.write(`無法連線: ${e.message}\r\n`);
        setStatus("無法連線", true);
      }
    }

    (async () => {
      try {
        await loadXterm();
      } catch (e) {
        screen.textContent = e.message;
        return;
      }
      if (closed) return;
      term = new window.Terminal({
        cursorBlink: true,
        fontFamily: 'ui-monospace, "SF Mono", Menlo, "DejaVu Sans Mono", "Noto Sans Mono CJK TC", monospace',
        fontSize: mobile ? 13 : 14,
        scrollback: 5000,
        theme: { background: "#1e1e1e", foreground: "#e6e6e6", cursor: "#e6e6e6" },
      });
      fit = new window.FitAddon.FitAddon();
      term.loadAddon(fit);
      term.open(screen);
      try {
        fit.fit();
      } catch {}
      term.onData(send);
      term.onBinary(send);
      ro.observe(screen);
      if (!mobile) term.focus();
      await loadNodes();
      start();
    })();

    return () => {
      // 只是關掉畫面, shell 繼續跑(下次接回來)
      closed = true;
      gen++;
      ro.disconnect();
      term?.dispose();
    };
  }

  return { mount };
})();

if (typeof Apps !== "undefined") Apps.registerView("terminal", TermApp);

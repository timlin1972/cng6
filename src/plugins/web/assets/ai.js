// cng6 AI 助理(Google Gemini; PC 視窗與手機共用)
//
// - 左邊是對話清單(手機: 清單一頁、對話一頁, 可以邊緣滑回清單)
// - 回答一邊產生一邊顯示(long-poll), 可以中途停止; 出錯可以重試
// - 可以貼上或選擇圖片(先在瀏覽器縮小再送出)
// - API key 只放在一台; 別台的網頁經由 cluster 使用那台的 AI(伺服器端轉送)
// - 郵件用 AiApp.inline() 在信件上方顯示摘要/翻譯
"use strict";

const AiApp = (() => {
  function h(tag, cls, text) {
    const el = document.createElement(tag);
    if (cls) el.className = cls;
    if (text != null) el.textContent = text;
    return el;
  }

  const ICON = {
    plus: `<svg viewBox="0 0 24 24"><path d="M12 5v14M5 12h14"/></svg>`,
    send: `<svg viewBox="0 0 24 24"><path d="M5 12l14-7-5 15-3-6-6-2z"/></svg>`,
    stop: `<svg viewBox="0 0 24 24"><rect x="7" y="7" width="10" height="10" rx="1.5"/></svg>`,
    image: `<svg viewBox="0 0 24 24"><rect x="3" y="5" width="18" height="14" rx="2"/><circle cx="9" cy="10" r="1.6"/><path d="M21 16l-5-5-8 8"/></svg>`,
    gear: `<svg viewBox="0 0 24 24"><circle cx="12" cy="12" r="3"/><path d="M12 2v3M12 19v3M4.2 4.2l2.1 2.1M17.7 17.7l2.1 2.1M2 12h3M19 12h3M4.2 19.8l2.1-2.1M17.7 6.3l2.1-2.1"/></svg>`,
    back: `<svg viewBox="0 0 24 24"><path d="M15 5l-7 7 7 7"/></svg>`,
    trash: `<svg viewBox="0 0 24 24"><path d="M4 7h16M10 11v6M14 11v6M6 7l1 13h10l1-13M9 7V4h6v3"/></svg>`,
    copy: `<svg viewBox="0 0 24 24"><rect x="8" y="8" width="12" height="12" rx="2"/><path d="M16 8V5a1 1 0 0 0-1-1H5a1 1 0 0 0-1 1v10a1 1 0 0 0 1 1h3"/></svg>`,
    x: `<svg viewBox="0 0 24 24"><path d="M6 6l12 12M18 6L6 18"/></svg>`,
  };

  async function req(url, opts = {}) {
    if (opts.json !== undefined) {
      opts = { ...opts, method: opts.method || "POST", headers: { "content-type": "application/json" }, body: JSON.stringify(opts.json) };
    }
    const r = await fetch(url, { cache: "no-store", ...opts });
    const d = await r.json().catch(() => ({}));
    if (!r.ok) throw new Error(d.error || `HTTP ${r.status}`);
    return d;
  }

  // ---- Markdown(只產生安全的元素: 先跳脫 HTML, 連結只允許 http/https) ----
  function esc(s) {
    return s.replace(/[&<>"']/g, (c) => ({ "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;", "'": "&#39;" })[c]);
  }
  function inline(s) {
    const codes = [];
    s = s.replace(/`([^`\n]+)`/g, (_, c) => {
      codes.push(c);
      return `\u0000${codes.length - 1}\u0000`;
    });
    s = esc(s)
      .replace(/\[([^\]\n]+)\]\((https?:\/\/[^\s)]+)\)/g, (_, t, u) => `<a href="${u}" target="_blank" rel="noopener noreferrer">${t}</a>`)
      .replace(/(^|[\s(])(https?:\/\/[^\s<)]+)/g, (_, p, u) => `${p}<a href="${u}" target="_blank" rel="noopener noreferrer">${u}</a>`)
      .replace(/\*\*([^*\n]+)\*\*/g, "<b>$1</b>")
      .replace(/(^|[^*\w])\*([^*\n]+)\*(?!\w)/g, "$1<i>$2</i>")
      .replace(/~~([^~\n]+)~~/g, "<s>$1</s>");
    return s.replace(/\u0000(\d+)\u0000/g, (_, i) => `<code>${esc(codes[+i])}</code>`);
  }
  function markdown(src) {
    const lines = src.replace(/\r/g, "").split("\n");
    const out = [];
    let i = 0;
    const isTableSep = (l) => /^\s*\|?\s*:?-{2,}:?\s*(\|\s*:?-{2,}:?\s*)*\|?\s*$/.test(l);
    const cells = (l) => l.trim().replace(/^\||\|$/g, "").split("|").map((c) => inline(c.trim()));
    while (i < lines.length) {
      const l = lines[i];
      let m;
      if ((m = l.match(/^\s*```\s*([\w+-]*)/))) {
        const buf = [];
        i++;
        while (i < lines.length && !/^\s*```/.test(lines[i])) buf.push(lines[i++]);
        i++;
        out.push(`<div class="ai-code"><div class="ai-code-head"><span>${esc(m[1] || "")}</span><button class="ai-copy" type="button">複製</button></div><pre><code>${esc(buf.join("\n"))}</code></pre></div>`);
        continue;
      }
      if (/^\s*$/.test(l)) {
        i++;
        continue;
      }
      if ((m = l.match(/^(#{1,6})\s+(.*)/))) {
        const n = Math.min(m[1].length + 2, 6);
        out.push(`<h${n}>${inline(m[2])}</h${n}>`);
        i++;
        continue;
      }
      if (/^\s*([-*_])(\s*\1){2,}\s*$/.test(l)) {
        out.push("<hr>");
        i++;
        continue;
      }
      if (l.includes("|") && i + 1 < lines.length && isTableSep(lines[i + 1])) {
        const head = cells(l);
        i += 2;
        const rows = [];
        while (i < lines.length && lines[i].includes("|") && lines[i].trim()) rows.push(cells(lines[i++]));
        out.push(`<div class="ai-table"><table><thead><tr>${head.map((c) => `<th>${c}</th>`).join("")}</tr></thead><tbody>${rows
          .map((r) => `<tr>${r.map((c) => `<td>${c}</td>`).join("")}</tr>`)
          .join("")}</tbody></table></div>`);
        continue;
      }
      if (/^\s*>/.test(l)) {
        const buf = [];
        while (i < lines.length && /^\s*>/.test(lines[i])) buf.push(lines[i++].replace(/^\s*>\s?/, ""));
        out.push(`<blockquote>${markdown(buf.join("\n"))}</blockquote>`);
        continue;
      }
      if ((m = l.match(/^(\s*)([-*+]|\d+[.)])\s+/))) {
        const ordered = /\d/.test(m[2]);
        const items = [];
        while (i < lines.length) {
          const x = lines[i].match(/^(\s*)([-*+]|\d+[.)])\s+(.*)/);
          if (x && /\d/.test(x[2]) === ordered && x[1].length <= m[1].length + 1) {
            items.push([x[3]]);
            i++;
          } else if (items.length && /^\s{2,}\S/.test(lines[i])) {
            items[items.length - 1].push(lines[i++].trim());
          } else break;
        }
        const tag = ordered ? "ol" : "ul";
        out.push(`<${tag}>${items.map((it) => `<li>${inline(it[0])}${it.length > 1 ? markdown(it.slice(1).join("\n")) : ""}</li>`).join("")}</${tag}>`);
        continue;
      }
      const buf = [];
      while (i < lines.length && lines[i].trim() && !/^(\s*```|#{1,6}\s|\s*>|\s*([-*+]|\d+[.)])\s+)/.test(lines[i])) {
        if (lines[i].includes("|") && i + 1 < lines.length && isTableSep(lines[i + 1])) break;
        buf.push(inline(lines[i++]));
      }
      if (!buf.length) buf.push(inline(lines[i++]));
      out.push(`<p>${buf.join("<br>")}</p>`);
    }
    return out.join("");
  }

  function renderMarkdown(el, text) {
    el.innerHTML = markdown(text);
    el.querySelectorAll(".ai-copy").forEach((b) => {
      b.onclick = async () => {
        try {
          await navigator.clipboard.writeText(b.closest(".ai-code").querySelector("code").textContent);
          b.textContent = "已複製";
        } catch {
          b.textContent = "無法複製";
        }
        setTimeout(() => (b.textContent = "複製"), 1500);
      };
    });
  }

  // ---- 圖片: 縮小到最長邊 1600px 的 JPEG ----
  function shrink(file) {
    return new Promise((resolve, reject) => {
      const url = URL.createObjectURL(file);
      const img = new Image();
      img.onload = () => {
        URL.revokeObjectURL(url);
        const s = Math.min(1, 1600 / Math.max(img.width, img.height));
        const c = document.createElement("canvas");
        c.width = Math.round(img.width * s);
        c.height = Math.round(img.height * s);
        c.getContext("2d").drawImage(img, 0, 0, c.width, c.height);
        const data = c.toDataURL("image/jpeg", 0.85);
        resolve({ mime: "image/jpeg", data: data.slice(data.indexOf(",") + 1), preview: data });
      };
      img.onerror = () => {
        URL.revokeObjectURL(url);
        reject(new Error("無法讀取圖片"));
      };
      img.src = url;
    });
  }

  // 依時間分組的標題
  function when(ts) {
    const d = new Date(ts * 1000);
    const now = new Date();
    const day = (x) => new Date(x.getFullYear(), x.getMonth(), x.getDate()).getTime();
    const diff = Math.round((day(now) - day(d)) / 86400000);
    if (diff === 0) return d.toLocaleTimeString("zh-TW", { hour: "2-digit", minute: "2-digit", hour12: false });
    if (diff === 1) return "昨天";
    if (diff < 7) return `${diff} 天前`;
    return d.getFullYear() === now.getFullYear() ? `${d.getMonth() + 1}/${d.getDate()}` : `${d.getFullYear()}/${d.getMonth() + 1}/${d.getDate()}`;
  }

  // ---- 一個對話的即時內容(App 與郵件的摘要共用) ----
  // onUpdate(view) 每次有變化時呼叫; 回傳停止函式
  function follow(id, onUpdate, onError) {
    let stopped = false;
    let ver = null;
    (async () => {
      while (!stopped) {
        try {
          const d = await req(`/api/ai/chats/${encodeURIComponent(id)}${ver === null ? "" : `?after=${ver}`}`);
          if (stopped) return;
          ver = d.ver;
          onUpdate(d.chat);
        } catch (e) {
          if (stopped) return;
          if (onError && onError(e) === false) return;
          await new Promise((r) => setTimeout(r, 3000));
        }
        // 背景分頁: 慢一點
        if (document.hidden) await new Promise((r) => setTimeout(r, 2000));
      }
    })();
    return () => (stopped = true);
  }

  // ---- 郵件用: 在 el 裡顯示這個對話的最後一則回答 ----
  function inline_(el, id, { onOpen } = {}) {
    el.replaceChildren();
    el.classList.add("ai-inline");
    const body = h("div", "ai-md ai-inline-body");
    const status = h("div", "ai-inline-status", "思考中…");
    const foot = h("div", "ai-inline-foot");
    const open = h("button", "ai-link", "在 AI 繼續聊");
    open.onclick = () => {
      if (onOpen) onOpen(id);
    };
    foot.append(open);
    el.append(status, body, foot);
    const stop = follow(id, (c) => {
      // 信已經關掉: 不用再更新
      if (!el.isConnected) return stop();
      const last = [...c.messages].reverse().find((m) => m.role === "model");
      const text = c.running ? c.partial || "" : last ? last.text : "";
      renderMarkdown(body, text);
      status.hidden = !c.running && !c.error;
      status.textContent = c.error ? `錯誤: ${c.error}` : c.status || (c.running ? "回答中…" : "");
      status.classList.toggle("err", !!c.error);
    });
    return stop;
  }

  function mount(el, { device = "pc" } = {}) {
    const mobile = device === "mobile";
    let info = null; // /api/ai
    let current = null; // 目前的對話 id
    let view = null; // 目前對話的內容
    let unfollow = null;
    let closed = false;
    let pending = []; // 要送出的圖片
    let listVer = null;

    const root = h("div", "ai-root");
    root.dataset.device = device;
    root.innerHTML = `
      <aside class="ai-side">
        <div class="ai-side-head">
          <button class="ai-new" type="button">${ICON.plus}<span>新對話</span></button>
          <button class="ai-icon ai-gear" type="button" title="設定" aria-label="設定">${ICON.gear}</button>
        </div>
        <div class="ai-list"></div>
      </aside>
      <section class="ai-main">
        <div class="ai-bar">
          <button class="ai-icon ai-back" type="button" title="回到清單" aria-label="回到清單" data-edge-back data-edge-el=".ai-main">${ICON.back}</button>
          <div class="ai-title"></div>
          <button class="ai-icon ai-del" type="button" title="刪除這個對話" aria-label="刪除這個對話">${ICON.trash}</button>
        </div>
        <div class="ai-msgs"></div>
        <form class="ai-compose">
          <div class="ai-pending"></div>
          <div class="ai-compose-row">
            <button class="ai-icon ai-attach" type="button" title="加入圖片" aria-label="加入圖片">${ICON.image}</button>
            <textarea class="ai-input" rows="1" placeholder="問點什麼…(可以貼上圖片)" enterkeyhint="send"></textarea>
            <button class="ai-icon ai-send" type="submit" title="送出" aria-label="送出">${ICON.send}</button>
          </div>
          <input class="ai-file" type="file" accept="image/*" multiple hidden>
        </form>
      </section>
      <div class="ai-overlay" hidden></div>
      <div class="ai-toast" role="status"></div>`;
    el.replaceChildren(root);
    const $ = (s) => root.querySelector(s);
    const listEl = $(".ai-list");
    const msgsEl = $(".ai-msgs");
    const input = $(".ai-input");
    const sendBtn = $(".ai-send");
    const pendingEl = $(".ai-pending");
    const overlay = $(".ai-overlay");
    const titleEl = $(".ai-title");

    let toastTimer;
    function toast(text, ok = true) {
      const t = $(".ai-toast");
      t.textContent = text;
      t.classList.toggle("err", !ok);
      t.classList.add("show");
      clearTimeout(toastTimer);
      toastTimer = setTimeout(() => t.classList.remove("show"), 3000);
    }

    // 手機: 清單頁 / 對話頁
    function showPage(chat) {
      root.classList.toggle("in-chat", !!chat);
    }

    // ---- 清單 ----
    function renderList() {
      listEl.replaceChildren();
      if (!info) return;
      if (!info.chats.length) listEl.append(h("div", "ai-empty-list", "還沒有對話"));
      for (const c of info.chats) {
        const b = h("button", "ai-item");
        b.type = "button";
        b.classList.toggle("on", c.id === current);
        b.append(h("span", "ai-item-title", c.title), h("span", "ai-item-time", c.running ? "回答中…" : when(c.updated)));
        b.onclick = () => openChat(c.id);
        listEl.append(b);
      }
    }

    async function loadInfo(wait = false) {
      const d = await req(`/api/ai${wait && listVer !== null ? `?after=${listVer}` : ""}`);
      listVer = d.ver;
      info = d;
      renderList();
      return d;
    }

    // 清單也即時更新(別的裝置新增的對話)
    (async () => {
      while (!closed) {
        try {
          await loadInfo(true);
        } catch {
          await new Promise((r) => setTimeout(r, 5000));
        }
        if (document.hidden) await new Promise((r) => setTimeout(r, 3000));
      }
    })();

    // ---- 對話 ----
    function welcome() {
      const w = h("div", "ai-welcome");
      w.append(h("div", "ai-welcome-title", "有什麼可以幫忙的?"));
      const tips = h("div", "ai-tips");
      for (const t of ["今天有什麼重要的信?", "我的待辦有哪些?", "這週天氣如何?", "有中獎的發票嗎?", "cluster 的機器都正常嗎?"]) {
        const b = h("button", "ai-tip", t);
        b.type = "button";
        b.onclick = () => {
          input.value = t;
          submit();
        };
        tips.append(b);
      }
      w.append(tips);
      return w;
    }

    function renderChat() {
      const atBottom = msgsEl.scrollHeight - msgsEl.scrollTop - msgsEl.clientHeight < 60;
      msgsEl.replaceChildren();
      titleEl.textContent = view ? view.title : "新對話";
      $(".ai-del").hidden = !view;
      if (!view || (!view.messages.length && !view.running)) {
        msgsEl.append(welcome());
        updateSend();
        return;
      }
      for (const m of view.messages) msgsEl.append(message(m));
      if (view.running) {
        const m = h("div", "ai-msg model");
        if (view.status) m.append(h("div", "ai-status", view.status));
        if (view.partial) {
          const b = h("div", "ai-md");
          renderMarkdown(b, view.partial);
          m.append(b);
        } else if (!view.status) m.append(h("div", "ai-status", "思考中…"));
        msgsEl.append(m);
      }
      if (view.error) {
        const e = h("div", "ai-error");
        e.append(h("span", "", view.error));
        const retry = h("button", "ai-link", "重試");
        retry.onclick = async () => {
          try {
            await req(`/api/ai/chats/${current}/retry`, { json: {} });
          } catch (err) {
            toast(err.message, false);
          }
        };
        e.append(retry);
        msgsEl.append(e);
      }
      if (atBottom || view.running) msgsEl.scrollTop = msgsEl.scrollHeight;
      updateSend();
    }

    function message(m) {
      const el = h("div", `ai-msg ${m.role}`);
      if (m.tools.length) {
        const t = h("div", "ai-tools");
        for (const name of [...new Set(m.tools)]) t.append(h("span", "ai-tool", name));
        el.append(t);
      }
      if (m.images.length) {
        const imgs = h("div", "ai-imgs");
        for (const [c, p] of m.images) {
          const img = document.createElement("img");
          img.src = `/api/ai/chats/${current}/img/${c}/${p}`;
          img.alt = "圖片";
          img.loading = "lazy";
          img.onclick = () => window.open(img.src, "_blank");
          imgs.append(img);
        }
        el.append(imgs);
      }
      if (m.text) {
        const b = h("div", m.role === "model" ? "ai-md" : "ai-bubble");
        if (m.role === "model") renderMarkdown(b, m.text);
        else b.textContent = m.text;
        el.append(b);
        if (m.role === "model") {
          const c = h("button", "ai-icon ai-copy-msg");
          c.type = "button";
          c.title = "複製";
          c.innerHTML = ICON.copy;
          c.onclick = async () => {
            try {
              await navigator.clipboard.writeText(m.text);
              toast("已複製");
            } catch {
              toast("無法複製", false);
            }
          };
          el.append(c);
        }
      }
      return el;
    }

    function openChat(id) {
      if (unfollow) unfollow();
      unfollow = null;
      current = id;
      view = null;
      renderList();
      renderChat();
      showPage(true);
      if (!id) {
        if (!mobile) input.focus();
        return;
      }
      unfollow = follow(id, (c) => {
        if (current !== id) return;
        view = c;
        renderChat();
      }, (e) => {
        if (/找不到/.test(e.message)) {
          openChat(null);
          return false;
        }
      });
    }

    function updateSend() {
      const running = view && view.running;
      sendBtn.innerHTML = running ? ICON.stop : ICON.send;
      sendBtn.title = running ? "停止" : "送出";
      sendBtn.classList.toggle("stop", !!running);
    }

    function renderPending() {
      pendingEl.replaceChildren();
      pending.forEach((p, i) => {
        const w = h("div", "ai-thumb");
        const img = document.createElement("img");
        img.src = p.preview;
        const x = h("button", "ai-thumb-x");
        x.type = "button";
        x.innerHTML = ICON.x;
        x.title = "移除";
        x.onclick = () => {
          pending.splice(i, 1);
          renderPending();
        };
        w.append(img, x);
        pendingEl.append(w);
      });
      pendingEl.hidden = !pending.length;
    }

    async function addImages(files) {
      for (const f of files) {
        if (!f.type.startsWith("image/")) continue;
        if (pending.length >= (info?.max_images || 4)) {
          toast(`一次最多 ${info?.max_images || 4} 張圖片`, false);
          break;
        }
        try {
          pending.push(await shrink(f));
        } catch (e) {
          toast(e.message, false);
        }
      }
      renderPending();
    }

    async function submit() {
      if (view && view.running) {
        try {
          await req(`/api/ai/chats/${current}/stop`, { json: {} });
        } catch {}
        return;
      }
      const text = input.value.trim();
      if (!text && !pending.length) return;
      const images = pending.map(({ mime, data }) => ({ mime, data }));
      sendBtn.disabled = true;
      try {
        if (!current) {
          const d = await req("/api/ai/chats", { json: { text, images } });
          input.value = "";
          pending = [];
          renderPending();
          autosize();
          openChat(d.id);
        } else {
          await req(`/api/ai/chats/${current}/send`, { json: { text, images } });
          input.value = "";
          pending = [];
          renderPending();
          autosize();
        }
      } catch (e) {
        toast(e.message, false);
      }
      sendBtn.disabled = false;
    }

    function autosize() {
      input.style.height = "auto";
      input.style.height = `${Math.min(input.scrollHeight, 180)}px`;
    }

    $(".ai-compose").onsubmit = (e) => {
      e.preventDefault();
      submit();
    };
    input.addEventListener("input", autosize);
    input.addEventListener("keydown", (e) => {
      // 電腦: Enter 送出, Shift+Enter 換行; 手機用送出鈕
      if (e.key === "Enter" && !e.shiftKey && !e.isComposing && !mobile) {
        e.preventDefault();
        submit();
      }
    });
    input.addEventListener("paste", (e) => {
      const files = [...(e.clipboardData?.files || [])].filter((f) => f.type.startsWith("image/"));
      if (files.length) {
        e.preventDefault();
        addImages(files);
      }
    });
    $(".ai-attach").onclick = () => $(".ai-file").click();
    $(".ai-file").onchange = (e) => {
      addImages([...e.target.files]);
      e.target.value = "";
    };
    $(".ai-new").onclick = () => openChat(null);
    $(".ai-back").onclick = () => {
      if (unfollow) unfollow();
      unfollow = null;
      current = null;
      view = null;
      renderList();
      showPage(false);
    };
    $(".ai-del").onclick = async () => {
      if (!current || !confirm(`刪除「${view?.title || "這個對話"}」?`)) return;
      try {
        await req(`/api/ai/chats/${current}`, { method: "DELETE" });
        openChat(null);
        if (mobile) showPage(false);
      } catch (e) {
        toast(e.message, false);
      }
    };
    root.addEventListener("dragover", (e) => e.preventDefault());
    root.addEventListener("drop", (e) => {
      e.preventDefault();
      addImages([...(e.dataTransfer?.files || [])]);
    });

    // ---- 設定 ----
    function openSettings(first = false) {
      overlay.hidden = false;
      overlay.replaceChildren();
      const box = h("div", "ai-dialog");
      box.append(h("h3", "", first ? "設定 AI 助理" : "AI 設定"));
      const help = h("div", "ai-help");
      help.innerHTML = `使用 Google Gemini。到 <a href="https://aistudio.google.com/apikey" target="_blank" rel="noopener">aistudio.google.com/apikey</a> 建立 API key, 貼到下面。
        <br>key 只存在<b>這台機器</b>(權限 600); cluster 的其他機器會自動改用這台的 AI。`;
      box.append(help);
      if (info && info.configured) {
        box.append(h("div", "ai-cur", `目前: ${info.node} · 模型 ${info.model}`));
      }
      const form = h("form", "ai-form");
      form.innerHTML = `
        <label>API key<input name="key" type="password" autocomplete="off" placeholder="AIza… 或 AQ.…" ${first ? "required" : ""}></label>
        <div class="ai-form-err" hidden></div>
        <div class="ai-form-btns">
          ${info && info.configured ? '<button type="button" class="ai-link danger ai-clear">移除 key</button>' : ""}
          <span class="ai-spacer"></span>
          <button type="button" class="ai-cancel">${first ? "稍後" : "關閉"}</button>
          <button type="submit" class="ai-primary">儲存</button>
        </div>`;
      const errEl = form.querySelector(".ai-form-err");
      form.querySelector(".ai-cancel").onclick = () => (overlay.hidden = true);
      const clear = form.querySelector(".ai-clear");
      if (clear) {
        clear.onclick = async () => {
          if (!confirm("移除這台的 API key?")) return;
          try {
            await req("/api/ai/key", { json: { key: "" } });
            overlay.hidden = true;
            toast("已移除");
            loadInfo().then(check);
          } catch (e) {
            toast(e.message, false);
          }
        };
      }
      form.onsubmit = async (e) => {
        e.preventDefault();
        const key = String(new FormData(form).get("key") || "").trim();
        if (!key) {
          overlay.hidden = true;
          return;
        }
        const btn = form.querySelector(".ai-primary");
        btn.disabled = true;
        btn.textContent = "確認中…";
        errEl.hidden = true;
        try {
          await req("/api/ai/key", { json: { key } });
          overlay.hidden = true;
          toast("API key 可以用, 已儲存");
          // 伺服器那邊同步 ai.node 需要一點時間
          setTimeout(() => loadInfo().then(check), 800);
        } catch (err) {
          errEl.textContent = err.message;
          errEl.hidden = false;
        }
        btn.disabled = false;
        btn.textContent = "儲存";
      };
      box.append(form);
      overlay.append(box);
      // 注意: onclick 回傳 false 會取消預設動作(表單送不出去), 所以要用大括號
      overlay.onclick = (e) => {
        if (e.target === overlay) overlay.hidden = true;
      };
      setTimeout(() => form.querySelector("input").focus(), 50);
    }
    $(".ai-gear").onclick = () => openSettings(false);

    function check() {
      root.classList.toggle("no-key", !info.configured);
    }

    // 郵件等其他 App 要求打開某個對話
    function takeRequest() {
      const r = window.AiOpenRequest;
      if (!r) return false;
      window.AiOpenRequest = null;
      openChat(r.id);
      return true;
    }
    const onOpenReq = () => takeRequest();
    window.addEventListener("cng6-ai-open", onOpenReq);

    loadInfo()
      .then((d) => {
        check();
        if (!d.configured) openSettings(true);
        if (takeRequest()) return;
        if (!mobile) openChat(d.chats.length && d.chats[0].running ? d.chats[0].id : null);
        else showPage(false);
      })
      .catch((e) => {
        msgsEl.replaceChildren(h("div", "ai-error", `讀取失敗: ${e.message}`));
      });
    renderPending();
    autosize();

    return () => {
      closed = true;
      if (unfollow) unfollow();
      window.removeEventListener("cng6-ai-open", onOpenReq);
    };
  }

  // 其他 App 開一個新的 AI 對話: 回傳對話 id
  async function start(text, display) {
    const d = await req("/api/ai/chats", { json: { text, display } });
    return d.id;
  }

  function open(id) {
    window.AiOpenRequest = { id };
    window.dispatchEvent(new CustomEvent("cng6-ai-open"));
    if (window.DeskNav) window.DeskNav.openApp("ai");
    else if (window.MobileNav) window.MobileNav.openApp("ai");
  }

  return { mount, inline: inline_, start, open, markdown };
})();

if (typeof Apps !== "undefined") Apps.registerView("ai", AiApp);

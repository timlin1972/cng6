// cng6 郵件(Gmail, 只收信; PC 視窗與手機共用)
//
// - 左: 資料夾與標籤, 中: 信件列表, 右: 讀信; 視窗窄時變成單欄
// - 信件內文放在 sandbox 的 iframe 裡(不執行程式), 外部圖片預設不載入
// - 可以切換到 cluster 裡的其他機器, 看那台設定的信箱(密碼留在那台)
"use strict";

const MailApp = (() => {
  function h(tag, cls, text) {
    const el = document.createElement(tag);
    if (cls) el.className = cls;
    if (text != null) el.textContent = text;
    return el;
  }
  const enc = encodeURIComponent;
  const qs = (o) => new URLSearchParams(o).toString();
  const svg = (d) =>
    `<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.9" stroke-linecap="round" stroke-linejoin="round">${d}</svg>`;
  const ICON = {
    back: svg(`<path d="M15 5l-7 7 7 7"/>`),
    refresh: svg(`<path d="M20 11a8 8 0 0 0-14.9-3.9"/><path d="M4 4v4h4"/><path d="M4 13a8 8 0 0 0 14.9 3.9"/><path d="M20 20v-4h-4"/>`),
    archive: svg(`<rect x="3" y="4" width="18" height="5" rx="1"/><path d="M5 9v10a1 1 0 0 0 1 1h12a1 1 0 0 0 1-1V9M10 13h4"/>`),
    inbox: svg(`<path d="M3 13l3-8h12l3 8v6a1 1 0 0 1-1 1H4a1 1 0 0 1-1-1z"/><path d="M3 13h5l1 3h6l1-3h5"/>`),
    trash: svg(`<path d="M3 6h18M8 6V4h8v2M6 6l1 14h10l1-14"/>`),
    unread: svg(`<rect x="3" y="5" width="18" height="14" rx="2"/><path d="M3 7l9 6 9-6"/>`),
    star: svg(`<path d="M12 3l2.8 5.7 6.2.9-4.5 4.4 1 6.2L12 17.3 6.5 20.2l1-6.2L3 9.6l6.2-.9z"/>`),
    ext: svg(`<path d="M14 4h6v6M20 4l-9 9"/><path d="M18 14v5a1 1 0 0 1-1 1H5a1 1 0 0 1-1-1V7a1 1 0 0 1 1-1h5"/>`),
    clip: svg(`<path d="M21 11l-8.5 8.5a5 5 0 0 1-7-7L14 4a3.5 3.5 0 0 1 5 5l-8.5 8.5a2 2 0 0 1-3-3L15 7"/>`),
    user: svg(`<circle cx="12" cy="8" r="4"/><path d="M4 21a8 8 0 0 1 16 0"/>`),
  };
  // 固定的資料夾: q 是 Gmail 搜尋條件(在「所有郵件」裡找), special 表示要直接開那個資料夾
  const FOLDERS = [
    { key: "inbox", title: "收件匣", q: "in:inbox" },
    { key: "starred", title: "已加星號", q: "is:starred" },
    { key: "important", title: "重要郵件", q: "is:important" },
    { key: "sent", title: "寄件備份", q: "in:sent" },
    { key: "drafts", title: "草稿", q: "in:drafts" },
    { key: "all", title: "所有郵件", q: "" },
    { key: "junk", title: "垃圾郵件", special: "junk" },
    { key: "trash", title: "垃圾桶", special: "trash" },
  ];

  // 頭像: 公司寄的信用公司圖示(Pi 代抓), 個人信箱用名字的第一個字
  const AVA_COLORS = ["#e57373", "#f06292", "#ba68c8", "#7986cb", "#4fa3e0", "#4db6ac", "#66bb6a", "#f4a742", "#a1887f", "#78909c"];
  function avatar(api, name, addr, org, cls = "ml-ava") {
    const el = document.createElement("div");
    el.className = cls;
    el.setAttribute("aria-hidden", "true");
    let hash = 0;
    for (const c of (addr || name || "?").toLowerCase()) hash = (hash * 31 + c.codePointAt(0)) >>> 0;
    el.style.background = AVA_COLORS[hash % AVA_COLORS.length];
    const first = [...String(name || addr || "?").replace(/^["'\s(<\[]+/, "")][0] || "?";
    el.textContent = first.toUpperCase();
    if (org) {
      const img = new Image();
      img.alt = "";
      img.onload = () => {
        el.textContent = "";
        el.classList.add("logo");
        el.append(img);
      };
      img.src = api(`/avatar/${encodeURIComponent(org)}`);
    }
    return el;
  }

  function when(ts, full = false) {
    if (!ts) return "";
    const d = new Date(ts * 1000);
    const now = new Date();
    const hm = `${String(d.getHours()).padStart(2, "0")}:${String(d.getMinutes()).padStart(2, "0")}`;
    if (full) return `${d.getFullYear()}/${d.getMonth() + 1}/${d.getDate()} ${hm}`;
    if (d.toDateString() === now.toDateString()) return hm;
    if (d.getFullYear() === now.getFullYear()) return `${d.getMonth() + 1}月${d.getDate()}日`;
    return `${d.getFullYear()}/${d.getMonth() + 1}/${d.getDate()}`;
  }

  function size(n) {
    if (n < 1024) return `${n} B`;
    if (n < 1024 * 1024) return `${(n / 1024).toFixed(0)} KB`;
    return `${(n / 1024 / 1024).toFixed(1)} MB`;
  }

  // ---------------------------------------------------------------- 畫面
  function mount(el, { device = "pc" } = {}) {
    const mobile = device === "mobile";
    let base = ""; // 別台: /api/cluster/node/<name>
    let accounts = [];
    let acct = null;
    let labels = [];
    let folder = FOLDERS[0];
    let label = null; // 一般標籤(資料夾名稱)
    let search = "";
    let more = false; // 正在載入後面的信
    let swipe = { left: "archive", right: "trash" }; // 手機列表滑動(這台的設定)
    let total = 0;
    let items = [];
    let current = null; // 讀信中: { info, summary }
    let closed = false;
    let loading = 0;

    const root = h("div", "ml-root");
    root.dataset.device = device;
    root.innerHTML = `
      <div class="ml-top">
        <select class="ml-node" title="哪一台機器的信箱" hidden></select>
        <select class="ml-acct" title="帳號"></select>
        <select class="ml-folder" title="資料夾"></select>
        <input class="ml-search" type="search" placeholder="搜尋(Gmail 語法, 例如 from:xxx has:attachment)" enterkeyhint="search">
        <button class="ml-btn ml-refresh" title="重新整理">${ICON.refresh}</button>
        <button class="ml-btn ml-accounts" title="帳號與設定">${ICON.user}</button>
      </div>
      <div class="ml-body">
        <nav class="ml-side"></nav>
        <section class="ml-list"><div class="ml-items"></div></section>
        <section class="ml-read"></section>
      </div>
      <div class="ml-overlay" hidden></div>`;
    el.replaceChildren(root);
    const $ = (s) => root.querySelector(s);
    const nodeSel = $(".ml-node");
    const acctSel = $(".ml-acct");
    const folderSel = $(".ml-folder");
    const searchBox = $(".ml-search");
    const side = $(".ml-side");
    const itemsEl = $(".ml-items");
    const reader = $(".ml-read");
    const overlay = $(".ml-overlay");

    async function req(url, opts) {
      const r = await fetch(url, opts);
      let data = null;
      try {
        data = await r.json();
      } catch {}
      if (!r.ok) throw new Error((data && data.error) || `${r.status} ${r.statusText}`);
      return data;
    }
    const api = (p) => `${base}/api/mail${p}`;
    const acctApi = (p) => api(`/a/${enc(acct)}${p}`);

    function busy(on) {
      loading += on ? 1 : -1;
      root.classList.toggle("loading", loading > 0);
    }

    // ---- 機器(cluster) ----
    async function loadNodes() {
      try {
        const s = await req("/api/cluster");
        const others = (s.nodes || []).filter((n) => !n.me && n.online && !n.stale);
        if (!others.length) return;
        nodeSel.replaceChildren(new Option(`這台 (${s.name})`, ""));
        for (const n of others) nodeSel.append(new Option(n.name, `/api/cluster/node/${enc(n.name)}`));
        nodeSel.value = base;
        nodeSel.hidden = false;
      } catch {}
    }
    nodeSel.onchange = async () => {
      base = nodeSel.value;
      acct = null;
      await loadAccounts();
    };

    // ---- 帳號 ----
    async function loadAccounts(keep = true) {
      let ov;
      try {
        ov = await req(api(""));
      } catch (e) {
        accounts = [];
        renderAccounts();
        itemsEl.replaceChildren(h("div", "ml-empty", `無法讀取: ${e.message}`));
        return;
      }
      accounts = ov.accounts || [];
      if (!keep || !accounts.find((a) => a.name === acct)) acct = accounts[0]?.name || null;
      renderAccounts();
      if (!acct) {
        side.replaceChildren();
        itemsEl.replaceChildren();
        showEmptyReader();
        openAccounts();
        return;
      }
      await loadLabels();
      await loadList();
    }

    function renderAccounts() {
      acctSel.replaceChildren(
        ...accounts.map((a) => new Option(`${a.name}${a.unread ? ` (${a.unread})` : ""}`, a.name)),
      );
      acctSel.hidden = accounts.length < 2;
      if (acct) acctSel.value = acct;
      const a = accounts.find((x) => x.name === acct);
      root.querySelector(".ml-conn")?.remove();
      if (a && a.error) {
        const c = h("div", "ml-conn", `${a.name}: ${a.error}`);
        root.querySelector(".ml-body").before(c);
      }
    }
    acctSel.onchange = async () => {
      acct = acctSel.value;
      folder = FOLDERS[0];
      label = null;
      await loadLabels();
      await loadList();
    };

    // ---- 資料夾與標籤 ----
    async function loadLabels() {
      labels = [];
      try {
        labels = await req(acctApi("/labels"));
      } catch {}
      renderFolders();
    }

    function specialName(kind) {
      return labels.find((l) => l.special === kind)?.name || null;
    }

    function userLabels() {
      return labels.filter((l) => !l.special && l.name !== "INBOX" && !l.name.startsWith("[Gmail]"));
    }

    function renderFolders() {
      const unread = accounts.find((a) => a.name === acct)?.unread || 0;
      side.replaceChildren();
      folderSel.replaceChildren();
      for (const f of FOLDERS) {
        if (f.special && !specialName(f.special)) continue;
        const b = h("button", "ml-fold");
        b.append(h("span", "", f.title));
        if (f.key === "inbox" && unread) b.append(h("span", "ml-badge", String(unread)));
        b.classList.toggle("on", !label && folder.key === f.key);
        b.onclick = () => choose(f, null);
        side.append(b);
        folderSel.append(new Option(f.title + (f.key === "inbox" && unread ? ` (${unread})` : ""), `f:${f.key}`));
      }
      const ul = userLabels();
      if (ul.length) {
        side.append(h("div", "ml-side-title", "標籤"));
        const grp = document.createElement("optgroup");
        grp.label = "標籤";
        for (const l of ul) {
          const b = h("button", "ml-fold ml-label", l.display || l.name);
          b.title = l.display || l.name;
          b.classList.toggle("on", label === l.name);
          b.onclick = () => choose(null, l.name);
          side.append(b);
          grp.append(new Option(l.display || l.name, `l:${l.name}`));
        }
        folderSel.append(grp);
      }
      folderSel.value = label ? `l:${label}` : `f:${folder.key}`;
    }
    folderSel.onchange = () => {
      const v = folderSel.value;
      if (v.startsWith("l:")) choose(null, v.slice(2));
      else choose(FOLDERS.find((f) => `f:${f.key}` === v), null);
    };

    function choose(f, l) {
      if (f) folder = f;
      label = l;
      search = "";
      searchBox.value = "";
      renderFolders();
      closeReader();
      loadList();
    }

    // ---- 列表 ----
    function listParams() {
      if (label) return { mailbox: label, q: search };
      if (folder.special) return { mailbox: specialName(folder.special), q: search };
      return { q: [folder.q, search].filter(Boolean).join(" ") };
    }

    const PAGE = 50;

    async function loadList(silent = false) {
      if (!acct) return;
      if (pending) {
        // 背景自動更新: 等送出後再讀, 不然被刪的信會跑回來
        if (silent) return;
        await commitPending();
      }
      if (!silent) {
        itemsEl.replaceChildren(h("div", "ml-empty", "讀取中…"));
        itemsEl.scrollTop = 0;
      }
      busy(true);
      try {
        const r = await req(acctApi(`/list?${qs({ ...listParams(), offset: 0 })}`));
        if (closed) return;
        items = r.items;
        total = r.total;
        renderList();
        fillUp();
      } catch (e) {
        if (!silent) itemsEl.replaceChildren(h("div", "ml-empty", `讀取失敗: ${e.message}`));
      } finally {
        busy(false);
      }
    }

    // 往下捲到接近底部: 載入後面的信
    async function loadMore() {
      if (more || !acct || items.length >= total) return;
      more = true;
      const key = JSON.stringify([acct, listParams()]);
      renderFooter();
      try {
        const r = await req(acctApi(`/list?${qs({ ...listParams(), offset: items.length })}`));
        // 載入期間換了資料夾或搜尋: 丟掉
        if (closed || key !== JSON.stringify([acct, listParams()])) return;
        const have = new Set(items.map((m) => m.id));
        const add = r.items.filter((m) => !have.has(m.id));
        total = r.total;
        items = items.concat(add);
        itemsEl.querySelector(".ml-foot")?.remove();
        for (const m of add) itemsEl.append(row(m));
        // 沒有新的(例如剛好被刪掉): 不要一直重試
        if (!add.length) total = items.length;
      } catch (e) {
        toast(`讀取失敗: ${e.message}`, false);
      } finally {
        more = false;
        renderFooter();
      }
      fillUp();
    }

    // 列表還沒填滿畫面(大螢幕)時繼續載入
    function fillUp() {
      if (itemsEl.scrollHeight <= itemsEl.clientHeight + 40) loadMore();
    }

    itemsEl.addEventListener("scroll", () => {
      if (itemsEl.scrollTop + itemsEl.clientHeight > itemsEl.scrollHeight - 400) loadMore();
    });

    function renderFooter() {
      itemsEl.querySelector(".ml-foot")?.remove();
      if (!items.length) return;
      const text = more ? "載入中…" : items.length < total ? "往下捲動載入更多" : `共 ${total} 封`;
      itemsEl.append(h("div", "ml-foot", text));
    }

    function renderList() {
      itemsEl.replaceChildren();
      if (!items.length) {
        itemsEl.append(h("div", "ml-empty", search ? "找不到符合的郵件" : "沒有郵件"));
      }
      for (const m of items) itemsEl.append(row(m));
      renderFooter();
    }

    function row(m) {
      const r = h("div", "ml-row");
      r.tabIndex = 0;
      r.dataset.id = m.id;
      r.classList.toggle("unread", m.unread);
      r.classList.toggle("on", current?.summary.id === m.id);
      const star = h("button", "ml-star");
      star.innerHTML = ICON.star;
      star.title = m.starred ? "取消星號" : "加上星號";
      star.classList.toggle("on", m.starred);
      star.onclick = async (e) => {
        e.stopPropagation();
        m.starred = !m.starred;
        star.classList.toggle("on", m.starred);
        await op([m.id], m.starred ? "star" : "unstar");
      };
      const main = h("div", "ml-row-main");
      const top = h("div", "ml-row-top");
      top.append(h("span", "ml-from", m.from || "(沒有寄件者)"));
      if (m.attach) {
        const c = h("span", "ml-clip");
        c.innerHTML = ICON.clip;
        top.append(c);
      }
      top.append(h("span", "ml-date", when(m.ts)));
      const sub = h("div", "ml-subject", m.subject || "(沒有主旨)");
      main.append(top, sub);
      if (m.labels.length) {
        const ls = h("div", "ml-tags");
        for (const l of m.labels.slice(0, 3)) ls.append(h("span", "ml-tag", l));
        main.append(ls);
      }
      const inner = h("div", "ml-row-in");
      inner.append(avatar(api, m.from, m.from_addr, m.org), main, star);
      r.append(inner);
      r.onclick = () => {
        // 剛滑過: 不要當成點一下
        if (r.dataset.swiped) return delete r.dataset.swiped;
        openMessage(m);
      };
      r.onkeydown = (e) => e.key === "Enter" && openMessage(m);
      enableSwipe(r, inner, m);
      return r;
    }

    // ---- 手機: 列表左右滑 ----
    const SWIPE = {
      archive: { title: "封存", icon: () => ICON.archive, color: "#2e9d5b" },
      trash: { title: "刪除", icon: () => ICON.trash, color: "#d03b3b" },
      read: { title: (m) => (m.unread ? "已讀" : "未讀"), icon: () => ICON.unread, color: "#0a84ff" },
      star: { title: (m) => (m.starred ? "取消星號" : "星號"), icon: () => ICON.star, color: "#f0a020" },
    };

    function enableSwipe(r, inner, m) {
      let id = null;
      let sx = 0;
      let sy = 0;
      let dx = 0;
      let mode = null;
      let bg = null;
      const actionFor = (x) => (x < 0 ? swipe.left : swipe.right);
      const reset = () => {
        inner.style.transition = "transform .2s";
        inner.style.transform = "";
        setTimeout(() => {
          bg?.remove();
          bg = null;
          inner.style.transition = "";
        }, 200);
      };
      r.addEventListener("pointerdown", (e) => {
        if (e.pointerType !== "touch" || e.target.closest(".ml-star")) return;
        id = e.pointerId;
        sx = e.clientX;
        sy = e.clientY;
        dx = 0;
        mode = null;
      });
      r.addEventListener("pointermove", (e) => {
        if (e.pointerId !== id) return;
        const x = e.clientX - sx;
        const y = e.clientY - sy;
        if (!mode) {
          if (Math.abs(x) > 12 && Math.abs(x) > Math.abs(y) * 1.5) {
            mode = "h";
            try {
              r.setPointerCapture(id);
            } catch {}
          } else if (Math.abs(y) > 10) {
            id = null; // 上下捲動
            return;
          } else return;
        }
        const act = actionFor(x);
        dx = act === "none" ? 0 : x;
        inner.style.transform = `translateX(${dx}px)`;
        if (!dx) return;
        const a = SWIPE[act];
        if (!bg) {
          bg = h("div", "ml-swipe-bg");
          r.prepend(bg);
        }
        const ready = Math.abs(dx) > r.offsetWidth * 0.3;
        const title = typeof a.title === "function" ? a.title(m) : a.title;
        bg.style.background = a.color;
        bg.classList.toggle("left", dx < 0);
        bg.classList.toggle("ready", ready);
        bg.innerHTML = `${a.icon()}<span></span>`;
        bg.querySelector("span").textContent = title;
      });
      const end = async (e) => {
        if (e.pointerId !== id) return;
        id = null;
        if (mode !== "h") return;
        r.dataset.swiped = "1";
        setTimeout(() => delete r.dataset.swiped, 400);
        const act = actionFor(dx);
        if (act === "none" || Math.abs(dx) <= r.offsetWidth * 0.3) return reset();
        await swipeAct(r, inner, m, act, dx < 0, reset);
      };
      r.addEventListener("pointerup", end);
      r.addEventListener("pointercancel", (e) => {
        if (e.pointerId !== id) return;
        id = null;
        reset();
      });
    }

    async function swipeAct(r, inner, m, act, left, reset) {
      if (act === "read" || act === "star") {
        reset();
        if (act === "read") {
          m.unread = !m.unread;
          r.classList.toggle("unread", m.unread);
          if (!(await op([m.id], m.unread ? "unread" : "read"))) {
            m.unread = !m.unread;
            r.classList.toggle("unread", m.unread);
          }
          refreshCounts();
        } else {
          m.starred = !m.starred;
          r.querySelector(".ml-star")?.classList.toggle("on", m.starred);
          if (!(await op([m.id], m.starred ? "star" : "unstar"))) {
            m.starred = !m.starred;
            r.querySelector(".ml-star")?.classList.toggle("on", m.starred);
          }
        }
        return;
      }
      // 封存、刪除: 滑出去後收起來(5 秒內可以復原)
      inner.style.transition = "transform .2s";
      inner.style.transform = `translateX(${left ? "-" : ""}100%)`;
      if (stays(act)) {
        // 例如在「所有郵件」裡封存: 信還留著, 直接送出
        if (await op([m.id], act)) {
          m.inbox = false;
          toast("已封存");
          refreshCounts();
        }
        return reset();
      }
      const idx = items.findIndex((x) => x.id === m.id);
      r.style.height = `${r.offsetHeight}px`;
      r.style.transition = "height .2s";
      requestAnimationFrame(() => (r.style.height = "0px"));
      setTimeout(() => {
        r.remove();
        items = items.filter((x) => x.id !== m.id);
        total = Math.max(0, total - 1);
        renderFooter();
        deferOp(m, act, idx);
      }, 220);
    }

    // 封存或移到收件匣後, 信還留在目前的列表(例如在「所有郵件」裡封存)
    function stays(what) {
      return (what === "archive" && !label && folder.key !== "inbox") || (what === "inbox" && folder.key === "all");
    }

    // ---- 讀信 ----
    function showEmptyReader() {
      reader.replaceChildren(h("div", "ml-empty ml-read-empty", accounts.length ? "選一封信來看" : ""));
    }

    function closeReader() {
      current = null;
      root.classList.remove("reading");
      itemsEl.querySelectorAll(".ml-row.on").forEach((r) => r.classList.remove("on"));
      showEmptyReader();
    }

    async function openMessage(m, images = false) {
      closeAttachment();
      current = { summary: m, info: null };
      root.classList.add("reading");
      itemsEl.querySelectorAll(".ml-row").forEach((r) => r.classList.toggle("on", r.dataset.id === m.id));
      reader.replaceChildren(h("div", "ml-empty", "讀取中…"));
      busy(true);
      let info;
      try {
        info = await req(acctApi(`/msg/${m.id}`));
      } catch (e) {
        reader.replaceChildren(backBar(), h("div", "ml-empty", `讀取失敗: ${e.message}`));
        return;
      } finally {
        busy(false);
      }
      if (current?.summary !== m) return;
      current.info = info;
      // 讀信會標成已讀
      if (m.unread) {
        m.unread = false;
        itemsEl.querySelector(`.ml-row[data-id="${m.id}"]`)?.classList.remove("unread");
        refreshCounts();
      }
      renderMessage(m, info, images);
    }

    function backBar() {
      const bar = h("div", "ml-bar");
      const back = h("button", "ml-btn ml-back");
      back.innerHTML = ICON.back;
      back.title = "回到列表";
      back.dataset.edgeBack = "";
      back.dataset.edgeEl = ".ml-read";
      back.onclick = closeReader;
      bar.append(back);
      return bar;
    }

    function toolButton(icon, title, fn, cls = "") {
      const b = h("button", `ml-btn ${cls}`);
      b.innerHTML = icon;
      b.title = title;
      b.setAttribute("aria-label", title);
      b.onclick = fn;
      return b;
    }

    function renderMessage(m, info, images) {
      const bar = backBar();
      const inTrash = folder.special === "trash" && !label;
      const out = !m.inbox && !inTrash;
      bar.append(
        out || inTrash
          ? toolButton(ICON.inbox, "移到收件匣", () => act(m, "inbox"))
          : toolButton(ICON.archive, "封存", () => act(m, "archive")),
        inTrash ? h("span") : toolButton(ICON.trash, "刪除", () => act(m, "trash")),
        toolButton(ICON.unread, "標為未讀", () => act(m, "unread")),
        toolButton(ICON.star, m.starred ? "取消星號" : "加上星號", async (e) => {
          m.starred = !m.starred;
          e.currentTarget.classList.toggle("on", m.starred);
          itemsEl.querySelector(`.ml-row[data-id="${m.id}"] .ml-star`)?.classList.toggle("on", m.starred);
          await op([m.id], m.starred ? "star" : "unstar");
        }, m.starred ? "on ml-starbtn" : "ml-starbtn"),
      );
      const a = accounts.find((x) => x.name === acct);
      const gmail = h("a", "ml-btn");
      gmail.innerHTML = ICON.ext;
      gmail.title = "在 Gmail 開啟";
      gmail.target = "_blank";
      gmail.rel = "noopener";
      try {
        gmail.href = `https://mail.google.com/mail/u/?authuser=${enc(a?.email || "")}#all/${BigInt(m.id).toString(16)}`;
      } catch {}
      bar.append(h("span", "ml-spacer"), gmail);

      const head = h("div", "ml-head");
      head.append(h("h2", "ml-h-subject", info.subject || "(沒有主旨)"));
      const who = h("div", "ml-h-from");
      if (info.from) {
        who.append(h("b", "", info.from.name || info.from.addr));
        if (info.from.name) who.append(h("span", "ml-h-addr", ` <${info.from.addr}>`));
      }
      who.append(h("span", "ml-h-date", when(info.date || m.ts, true)));
      const whoRow = h("div", "ml-h-who");
      whoRow.append(avatar(api, info.from?.name, info.from?.addr, m.org, "ml-ava lg"), who);
      head.append(whoRow);
      const fmt = (l) => l.map((x) => x.name || x.addr).join(", ");
      if (info.to.length) head.append(h("div", "ml-h-to", `收件者: ${fmt(info.to)}`));
      if (info.cc.length) head.append(h("div", "ml-h-to", `副本: ${fmt(info.cc)}`));

      const parts = [bar, head];
      const trust = async (on) => {
        try {
          await req(api("/trust"), {
            method: "POST",
            headers: { "content-type": "application/json" },
            body: JSON.stringify({ addr: info.from.addr, trust: on }),
          });
          info.trusted = on;
          toast(on ? `以後 ${info.from.addr} 的圖片會直接顯示` : "已取消, 以後要按「顯示圖片」");
          renderMessage(m, info, on);
        } catch (e) {
          toast(`失敗: ${e.message}`, false);
        }
      };
      if (info.remote_images && !images && !info.trusted) {
        const note = h("div", "ml-note");
        note.append(h("span", "", "為了保護隱私, 這封信的外部圖片沒有載入"));
        const show = h("button", "ml-link", "顯示圖片");
        show.onclick = () => renderMessage(m, info, true);
        note.append(show);
        if (info.from?.addr) {
          const always = h("button", "ml-link", "一律顯示這個寄件者的圖片");
          always.onclick = () => trust(true);
          note.append(always);
        }
        parts.push(note);
      } else if (info.remote_images && info.trusted) {
        const note = h("div", "ml-note ml-note-quiet");
        note.append(h("span", "", "這個寄件者的圖片會直接顯示"));
        const off = h("button", "ml-link", "取消");
        off.onclick = () => trust(false);
        note.append(off);
        parts.push(note);
      }
      const frame = document.createElement("iframe");
      frame.className = "ml-frame";
      frame.setAttribute("sandbox", "allow-same-origin allow-popups allow-popups-to-escape-sandbox");
      frame.title = info.subject || "郵件內容";
      frame.src = acctApi(`/msg/${m.id}/body${images || info.trusted ? "?images=1" : ""}`);
      // 依內容調整高度, 由外面的窗格捲動
      // 信件比畫面寬(例如固定 600px 的表格): 像 Gmail app 一樣整封縮小到剛好的寬度, 不用左右滑
      const fit = () => {
        try {
          const d = frame.contentDocument;
          const w = d && d.getElementById("cng6-mail");
          if (!w) return;
          w.style.transform = "";
          w.style.width = "";
          const avail = d.documentElement.clientWidth;
          const need = w.scrollWidth;
          let z = 1;
          if (need > avail + 2) {
            z = avail / need;
            w.style.width = `${need}px`;
            w.style.transform = `scale(${z})`;
          }
          frame.style.height = `${Math.max(120, Math.ceil(w.offsetHeight * z) + 2)}px`;
        } catch {}
      };
      frame.onload = () => {
        fit();
        // 圖片、字型載入或視窗寬度改變時高度會變
        try {
          new ResizeObserver(fit).observe(frame.contentDocument.body);
        } catch {}
        setTimeout(fit, 300);
        setTimeout(fit, 1500);
      };
      parts.push(frame);
      if (info.attachments.length) {
        const att = h("div", "ml-atts");
        for (const x of info.attachments) {
          const link = h("button", "ml-att");
          link.onclick = () => openAttachment(m, x);
          const ic = h("span", "ml-att-icon");
          ic.innerHTML = ICON.clip;
          link.append(ic, h("span", "ml-att-name", x.name), h("span", "ml-att-size", size(x.size)));
          att.append(link);
        }
        parts.push(att);
      }
      reader.replaceChildren(...parts);
      reader.scrollTop = 0;
    }

    // ---- 附件預覽: 在信箱裡開一層, 有返回(手機加到主畫面時開新頁會回不來) ----
    let viewer = null;
    function closeAttachment() {
      if (!viewer) return;
      viewer.stop();
      viewer.el.remove();
      viewer = null;
      root.classList.remove("viewing");
    }

    function openAttachment(m, x) {
      closeAttachment();
      const url = acctApi(`/msg/${m.id}/part/${x.part}`);
      const el = h("div", "ml-viewer");
      const bar = h("div", "ml-bar ml-viewer-bar");
      const back = h("button", "ml-btn ml-viewer-back");
      back.innerHTML = `${ICON.back}<span>返回</span>`;
      back.onclick = closeAttachment;
      back.dataset.edgeBack = "";
      back.dataset.edgeEl = ".ml-viewer";
      const dl = h("a", "ml-btn ml-viewer-dl");
      dl.href = `${url}?dl=1`;
      dl.download = x.name;
      dl.innerHTML = `${svg(`<path d="M12 4v12M7 11l5 5 5-5"/><path d="M4 20h16"/>`)}<span>下載</span>`;
      bar.append(back, h("span", "ml-viewer-name", x.name), dl);
      const body = h("div", "ml-viewer-body");
      el.append(bar, body);
      root.append(el);
      root.classList.add("viewing");
      let stopped = false;
      let doc = null;
      viewer = {
        el,
        stop() {
          stopped = true;
          doc?.destroy();
        },
      };
      if (x.type.startsWith("image/")) {
        const img = h("img", "ml-viewer-img");
        img.src = url;
        img.alt = x.name;
        body.append(img);
      } else if (x.type === "application/pdf") {
        const status = h("div", "ml-empty", "讀取中…");
        body.append(status);
        renderPdf(body, url, () => stopped)
          .then((d) => {
            doc = d;
            status.remove();
            if (stopped) d.destroy();
          })
          .catch((e) => (status.textContent = `無法開啟 PDF: ${e?.message || e}`));
      } else if (x.type.startsWith("text/")) {
        const f = document.createElement("iframe");
        f.className = "ml-viewer-frame";
        f.setAttribute("sandbox", "");
        f.src = url;
        body.append(f);
      } else {
        const box = h("div", "ml-empty");
        box.append(h("div", "", "這個檔案沒辦法在這裡預覽"), h("div", "ml-h-addr", `${x.type} · ${size(x.size)}`));
        const d2 = h("a", "ml-primary ml-viewer-dl2", "下載");
        d2.href = `${url}?dl=1`;
        d2.download = x.name;
        box.append(d2);
        body.append(box);
      }
    }

    let pdfjs = null;
    async function renderPdf(body, url, stopped) {
      if (!pdfjs) {
        pdfjs = import("/pdfjs/pdf.min.mjs").then((lib) => {
          lib.GlobalWorkerOptions.workerSrc = "/pdfjs/pdf.worker.min.mjs";
          return lib;
        });
        pdfjs.catch(() => (pdfjs = null));
      }
      const lib = await pdfjs;
      const doc = await lib.getDocument({
        url,
        cMapUrl: "/pdfjs/cmaps/",
        cMapPacked: true,
        standardFontDataUrl: "/pdfjs/standard_fonts/",
        wasmUrl: "/pdfjs/wasm/",
        isEvalSupported: false,
      }).promise;
      // 一頁一頁畫, 寬度配合畫面
      const width = Math.max(200, body.clientWidth - 16);
      const dpr = Math.min(2, window.devicePixelRatio || 1);
      const max = Math.min(doc.numPages, 100);
      for (let i = 1; i <= max && !stopped(); i++) {
        const page = await doc.getPage(i);
        const vp1 = page.getViewport({ scale: 1 });
        const vp = page.getViewport({ scale: (width / vp1.width) * dpr });
        const c = document.createElement("canvas");
        c.className = "ml-viewer-page";
        c.width = vp.width;
        c.height = vp.height;
        c.style.width = `${width}px`;
        body.append(c);
        await page.render({ canvasContext: c.getContext("2d"), viewport: vp }).promise;
      }
      if (doc.numPages > max) body.append(h("div", "ml-empty", `只顯示前 ${max} 頁, 完整內容請下載`));
      return doc;
    }

    async function act(m, what) {
      const keep0 = stays(what);
      // 刪除、封存: 先從畫面拿掉, 5 秒內可以復原, 之後才真的送出
      const later = (what === "trash" || what === "archive") && !keep0;
      if (!later && !(await op([m.id], what))) return;
      if (what === "unread") {
        m.unread = true;
        itemsEl.querySelector(`.ml-row[data-id="${m.id}"]`)?.classList.add("unread");
        closeReader();
        refreshCounts();
        return;
      }
      // 封存、刪除、移到收件匣: 從目前的列表拿掉(看所有郵件時封存仍留著)
      const keep = stays(what);
      const idx = items.findIndex((x) => x.id === m.id);
      if (!keep) {
        items = items.filter((x) => x.id !== m.id);
        total = Math.max(0, total - 1);
      } else {
        m.inbox = what === "inbox";
      }
      // 自動前進: 打開下一封(較舊的); 已經是最後一封就打開上一封
      const nextIdx = keep ? idx + 1 : idx;
      const next = idx < 0 ? null : items[nextIdx] || items[nextIdx - 1];
      renderList();
      if (next && next !== m) openMessage(next);
      else closeReader();
      if (later) deferOp(m, what, idx);
      else {
        toast({ archive: "已封存", trash: "已移到垃圾桶", inbox: "已移到收件匣" }[what] || "完成");
        refreshCounts();
      }
    }

    // ---- 刪除、封存: 5 秒內可以復原 ----
    const UNDO_MS = 5000;
    let pending = null; // { m, what, idx, acct, api, timer }

    function deferOp(m, what, idx) {
      // 上一個還沒送出的先送
      if (pending) commitPending();
      pending = { m, what, idx, acct, api: acctApi("/op"), timer: setTimeout(commitPending, UNDO_MS) };
      showUndo(what === "trash" ? "已移到垃圾桶" : "已封存");
    }

    async function commitPending(keepalive = false) {
      const p = pending;
      if (!p) return;
      pending = null;
      clearTimeout(p.timer);
      hideUndo();
      try {
        const r = await fetch(p.api, {
          method: "POST",
          headers: { "content-type": "application/json" },
          body: JSON.stringify({ ids: [p.m.id], op: p.what }),
          keepalive,
        });
        if (!r.ok) throw new Error((await r.json().catch(() => null))?.error || r.statusText);
        if (!closed) refreshCounts();
      } catch (e) {
        if (closed) return;
        toast(`失敗: ${e.message}`, false);
        // 放回列表
        if (p.acct === acct) restore(p);
      }
    }

    function restore(p) {
      if (items.some((x) => x.id === p.m.id)) return;
      items.splice(Math.max(0, Math.min(p.idx, items.length)), 0, p.m);
      total++;
      renderList();
    }

    function undo() {
      const p = pending;
      if (!p) return;
      pending = null;
      clearTimeout(p.timer);
      hideUndo();
      if (p.acct === acct) restore(p);
      toast("已復原");
    }

    function showUndo(text) {
      let u = root.querySelector(".ml-undo");
      if (!u) {
        u = h("div", "ml-undo");
        u.append(h("span", "ml-undo-text"));
        const b = h("button", "ml-undo-btn", "復原");
        b.onclick = undo;
        u.append(b, h("i", "ml-undo-bar"));
        root.append(u);
      }
      u.querySelector(".ml-undo-text").textContent = text;
      // 重新開始倒數的動畫
      const bar = u.querySelector(".ml-undo-bar");
      bar.style.animation = "none";
      void bar.offsetWidth;
      bar.style.animation = "";
      u.classList.add("show");
    }

    function hideUndo() {
      root.querySelector(".ml-undo")?.classList.remove("show");
    }

    // 頁面關掉或切到背景: 馬上送出, 不要遺漏
    const flushNow = () => pending && commitPending(true);
    window.addEventListener("pagehide", flushNow);
    const onHidden = () => document.hidden && flushNow();
    document.addEventListener("visibilitychange", onHidden);

    async function op(ids, what) {
      try {
        await req(acctApi("/op"), {
          method: "POST",
          headers: { "content-type": "application/json" },
          body: JSON.stringify({ ids, op: what }),
        });
        return true;
      } catch (e) {
        toast(`失敗: ${e.message}`, false);
        return false;
      }
    }

    // 未讀數(帳號選單與收件匣旁的數字)
    async function refreshCounts() {
      try {
        const ov = await req(api(""));
        accounts = ov.accounts || [];
        renderAccounts();
        renderFolders();
      } catch {}
    }

    // ---- 帳號管理 ----
    function openAccounts() {
      overlay.hidden = false;
      const box = h("div", "ml-dialog");
      box.append(h("h3", "", "Gmail 帳號與設定"));
      if (accounts.length) {
        const list = h("div", "ml-acct-list");
        for (const a of accounts) {
          const r = h("div", "ml-acct-row");
          r.append(h("b", "", a.name), h("span", "ml-acct-mail", a.email));
          const rm = h("button", "ml-link danger", "移除");
          rm.onclick = async () => {
            if (!confirm(`移除帳號 ${a.name}(${a.email})?`)) return;
            try {
              await req(api(`/accounts/${enc(a.name)}`), { method: "DELETE" });
              overlay.hidden = true;
              acct = null;
              setTimeout(() => loadAccounts(false), 500);
            } catch (e) {
              toast(e.message, false);
            }
          };
          r.append(rm);
          list.append(r);
        }
        box.append(list);
      }
      if (mobile || matchMedia("(pointer: coarse)").matches) {
        box.append(h("h4", "", "手機列表滑動"));
        const sw = h("div", "ml-swipe-set");
        const names = { none: "不動作", archive: "封存", trash: "刪除", read: "已讀 / 未讀", star: "星號" };
        for (const [k, title] of [["left", "往左滑"], ["right", "往右滑"]]) {
          const lab = h("label", "", title);
          const sel = document.createElement("select");
          for (const [v, n] of Object.entries(names)) sel.append(new Option(n, v));
          sel.value = swipe[k];
          sel.onchange = async () => {
            const r = await Apps.exec(`/settings set mail.swipe_${k} ${sel.value}`);
            if (r.ok) {
              swipe[k] = sel.value;
              toast(`${title}: ${names[sel.value]}`);
            } else toast(r.text, false);
          };
          lab.append(sel);
          sw.append(lab);
        }
        box.append(sw);
      }
      box.append(h("h4", "", "加入帳號"));
      const help = h("div", "ml-help");
      help.innerHTML = `用 Google 的<b>應用程式密碼</b>登入(不是 Gmail 密碼):
        <ol><li>Google 帳戶要先開啟<b>兩步驟驗證</b></li>
        <li>到 <a href="https://myaccount.google.com/apppasswords" target="_blank" rel="noopener">myaccount.google.com/apppasswords</a> 建立一組, 名稱填 cng6</li>
        <li>把 16 個字母的密碼貼到下面</li></ol>
        密碼只存在${base ? "那台" : "這台"}機器(權限 600), 網頁與其他機器都看不到。`;
      box.append(help);
      const form = h("form", "ml-form");
      form.innerHTML = `
        <label>名稱<input name="name" required maxlength="40" value="${accounts.length ? "" : "個人"}" placeholder="例如 個人"></label>
        <label>Gmail<input name="email" type="email" required autocomplete="username" placeholder="you@gmail.com"></label>
        <label>應用程式密碼<input name="password" type="password" required autocomplete="new-password" placeholder="xxxx xxxx xxxx xxxx"></label>
        <div class="ml-form-err" hidden></div>
        <div class="ml-form-btns"><button type="button" class="ml-cancel">${accounts.length ? "關閉" : "取消"}</button><button type="submit" class="ml-primary">登入並加入</button></div>`;
      const errEl = form.querySelector(".ml-form-err");
      form.querySelector(".ml-cancel").onclick = () => {
        overlay.hidden = true;
      };
      form.onsubmit = async (e) => {
        e.preventDefault();
        const fd = new FormData(form);
        const submit = form.querySelector(".ml-primary");
        submit.disabled = true;
        submit.textContent = "登入中…";
        errEl.hidden = true;
        try {
          await req(api("/accounts"), {
            method: "POST",
            headers: { "content-type": "application/json" },
            body: JSON.stringify({ name: fd.get("name"), email: fd.get("email"), password: fd.get("password") }),
          });
          overlay.hidden = true;
          acct = String(fd.get("name")).trim();
          toast("已加入, 連線中…");
          // 背景重新連線需要一點時間
          setTimeout(() => loadAccounts(), 1500);
        } catch (err) {
          errEl.textContent = err.message;
          errEl.hidden = false;
        } finally {
          submit.disabled = false;
          submit.textContent = "登入並加入";
        }
      };
      box.append(form);
      overlay.replaceChildren(box);
      overlay.onclick = (e) => {
        if (e.target === overlay && accounts.length) overlay.hidden = true;
      };
    }
    $(".ml-accounts").onclick = openAccounts;

    // ---- 其他 ----
    let toastTimer = null;
    function toast(text, ok = true) {
      let t = root.querySelector(".ml-toast");
      if (!t) {
        t = h("div", "ml-toast");
        root.append(t);
      }
      t.textContent = text;
      t.dataset.ok = ok;
      t.classList.add("show");
      clearTimeout(toastTimer);
      toastTimer = setTimeout(() => t.classList.remove("show"), 2400);
    }

    searchBox.addEventListener("keydown", (e) => {
      if (e.key === "Enter") {
        search = searchBox.value.trim();
          closeReader();
        loadList();
      }
    });
    searchBox.addEventListener("search", () => {
      if (!searchBox.value && search) {
        search = "";
        loadList();
      }
    });
    $(".ml-refresh").onclick = async () => {
      await refreshCounts();
      await loadList();
    };

    // 通知被點: 打開那封信
    async function openRequested(req) {
      if (!req || base) return;
      window.MailOpenRequest = null;
      if (req.account !== acct && accounts.find((a) => a.name === req.account)) {
        acct = req.account;
        acctSel.value = acct;
        await loadLabels();
      }
      choose(FOLDERS[0], null);
      await loadList();
      const m = items.find((x) => x.id === req.id) || { id: req.id, from: "", subject: "", labels: [], inbox: true, starred: false, unread: true, ts: 0 };
      openMessage(m);
    }
    // 手機: 從左邊緣往右滑(mobile.js)拖動信件時, 底下露出列表
    reader.addEventListener("edgeback-start", () => root.classList.add("peek"));
    reader.addEventListener("edgeback-end", () => root.classList.remove("peek"));

    const onKey = (e) => e.key === "Escape" && viewer && closeAttachment();
    root.addEventListener("keydown", onKey);
    const onOpen = (e) => openRequested(e.detail);
    window.addEventListener("cng6-mail-open", onOpen);

    // 定時更新: 列表(不打擾讀信)與未讀數
    const timer = setInterval(() => {
      if (document.hidden || !acct) return;
      refreshCounts();
      if (!current && items.length <= PAGE && itemsEl.scrollTop < 50) loadList(true);
    }, 60000);

    (async () => {
      try {
        const ov = await req("/api/mail");
        if (ov.swipe) swipe = ov.swipe;
      } catch {}
      await loadNodes();
      await loadAccounts();
      if (window.MailOpenRequest) openRequested(window.MailOpenRequest);
    })();

    return () => {
      closed = true;
      clearInterval(timer);
      closeAttachment();
      commitPending(true);
      window.removeEventListener("pagehide", flushNow);
      document.removeEventListener("visibilitychange", onHidden);
      window.removeEventListener("cng6-mail-open", onOpen);
    };
  }

  return { mount };
})();

if (typeof Apps !== "undefined") Apps.registerView("mail", MailApp);

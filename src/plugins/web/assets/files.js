// cng6 檔案管理員(PC 視窗與手機共用)
//
// - 根目錄在設定 files.roots, 只能在根目錄裡面操作(伺服器檢查)
// - 刪除 = 移到資源回收桶, 可以還原
// - PC: 右鍵選單、拖放上傳、拖到資料夾搬移(按住 Ctrl 複製)、快速鍵
// - 手機: 點一下開啟, 長按進入選取, 每列的「⋯」有完整選單
"use strict";

const FilesApp = (() => {
  function h(tag, cls, text) {
    const el = document.createElement(tag);
    if (cls) el.className = cls;
    if (text != null) el.textContent = text;
    return el;
  }
  const qs = (o) => new URLSearchParams(o).toString();
  const join = (dir, name) => (dir ? `${dir}/${name}` : name);
  const parentOf = (p) => (p.includes("/") ? p.slice(0, p.lastIndexOf("/")) : "");
  const extOf = (name) => (name.includes(".") ? name.slice(name.lastIndexOf(".") + 1).toLowerCase() : "");

  const svg = (d, extra = "") =>
    `<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.9" stroke-linecap="round" stroke-linejoin="round" ${extra}>${d}</svg>`;
  const ICON = {
    back: svg(`<path d="M15 5l-7 7 7 7"/>`),
    fwd: svg(`<path d="M9 5l7 7-7 7"/>`),
    up: svg(`<path d="M12 19V5M5 12l7-7 7 7"/>`),
    newdir: svg(`<path d="M3 7a2 2 0 0 1 2-2h4l2 2h8a2 2 0 0 1 2 2v8a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2z"/><path d="M12 11v5M9.5 13.5h5"/>`),
    newfile: svg(`<path d="M14 3H7a2 2 0 0 0-2 2v14a2 2 0 0 0 2 2h10a2 2 0 0 0 2-2V8z"/><path d="M14 3v5h5M12 11v6M9 14h6"/>`),
    upload: svg(`<path d="M12 16V4M7 9l5-5 5 5"/><path d="M4 16v3a2 2 0 0 0 2 2h12a2 2 0 0 0 2-2v-3"/>`),
    paste: svg(`<rect x="6" y="4" width="12" height="17" rx="2"/><path d="M9 4V3h6v1"/>`),
    more: `<svg viewBox="0 0 24 24"><circle cx="5" cy="12" r="1.8" fill="currentColor"/><circle cx="12" cy="12" r="1.8" fill="currentColor"/><circle cx="19" cy="12" r="1.8" fill="currentColor"/></svg>`,
    trash: svg(`<path d="M4 7h16M10 11v6M14 11v6M6 7l1 13h10l1-13M9 7V4h6v3"/>`),
    home: svg(`<path d="M3 11l9-7 9 7"/><path d="M5 10v10h14V10"/>`),
    drive: svg(`<rect x="3" y="7" width="18" height="10" rx="2"/><path d="M7 12h.01M11 12h6"/>`),
    close: svg(`<path d="M6 6l12 12M18 6L6 18"/>`),
    check: svg(`<path d="M5 12l5 5 9-10"/>`),
  };
  // 檔案類型圖示
  const TYPE_ICON = {
    dir: { color: "#5ac8fa", d: `<path d="M3 7a2 2 0 0 1 2-2h4l2 2h8a2 2 0 0 1 2 2v8a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2z" fill="currentColor" stroke="none"/>` },
    image: { color: "#34c759", d: `<rect x="3" y="4" width="18" height="16" rx="2"/><circle cx="9" cy="10" r="1.6"/><path d="M21 16l-5-5-8 9"/>` },
    video: { color: "#ff6961", d: `<rect x="3" y="5" width="18" height="14" rx="2"/><path d="M10 9l5 3-5 3z" fill="currentColor"/>` },
    audio: { color: "#ff2d92", d: `<path d="M9 18V5l12-2v13"/><circle cx="6" cy="18" r="3"/><circle cx="18" cy="16" r="3"/>` },
    pdf: { color: "#e5484d", d: `<path d="M14 3H7a2 2 0 0 0-2 2v14a2 2 0 0 0 2 2h10a2 2 0 0 0 2-2V8z"/><path d="M14 3v5h5"/><path d="M8.5 16h7M8.5 12.5h7"/>` },
    text: { color: "#8e8e93", d: `<path d="M14 3H7a2 2 0 0 0-2 2v14a2 2 0 0 0 2 2h10a2 2 0 0 0 2-2V8z"/><path d="M14 3v5h5M9 13h6M9 17h4"/>` },
    code: { color: "#0a84ff", d: `<path d="M14 3H7a2 2 0 0 0-2 2v14a2 2 0 0 0 2 2h10a2 2 0 0 0 2-2V8z"/><path d="M14 3v5h5M10 12l-2 2 2 2M14 12l2 2-2 2"/>` },
    archive: { color: "#ff9f0a", d: `<rect x="4" y="3" width="16" height="18" rx="2"/><path d="M12 3v2M12 7v2M12 11v2"/><rect x="10.5" y="14" width="3" height="4" rx="1"/>` },
    book: { color: "#e07800", d: `<path d="M4 4.5A2.5 2.5 0 0 1 6.5 2H20v17H6.5A2.5 2.5 0 0 0 4 21.5z"/><path d="M4 21.5A2.5 2.5 0 0 1 6.5 19H20v3H6.5"/>` },
    file: { color: "#a1a1a6", d: `<path d="M14 3H7a2 2 0 0 0-2 2v14a2 2 0 0 0 2 2h10a2 2 0 0 0 2-2V8z"/><path d="M14 3v5h5"/>` },
  };
  const EXT = {
    image: "jpg jpeg png gif webp avif bmp ico heic",
    video: "mp4 m4v webm mov mkv avi ogv",
    audio: "mp3 m4a aac flac ogg oga opus wav",
    pdf: "pdf",
    code: "rs js mjs ts tsx jsx py sh bash zsh c h cpp hpp cc go java kt rb php lua pl swift css scss html htm xml svg json toml yaml yml ini conf cfg service sql mk cmake gradle vue",
    text: "txt md markdown log csv tsv srt lrc vtt nfo diff patch env gitignore dockerfile makefile cng",
    archive: "zip gz tgz xz bz2 zst 7z rar tar",
    book: "epub mobi azw azw3 cbz",
  };
  const KIND_OF = {};
  for (const [k, list] of Object.entries(EXT)) for (const e of list.split(" ")) KIND_OF[e] = k;
  function kindOf(e) {
    if (e.dir) return "dir";
    const lower = e.name.toLowerCase();
    if (["makefile", "dockerfile", "readme", "license", "cargo.lock"].includes(lower)) return "text";
    if (lower.startsWith(".") && !lower.slice(1).includes(".")) return "text"; // .bashrc、.gitignore…
    return KIND_OF[extOf(e.name)] || "file";
  }
  const typeIcon = (kind) => {
    const t = TYPE_ICON[kind] || TYPE_ICON.file;
    return `<span class="fm-ti" style="color:${t.color}">${svg(t.d)}</span>`;
  };
  const KIND_LABEL = { dir: "資料夾", image: "圖片", video: "影片", audio: "音訊", pdf: "PDF", code: "程式碼", text: "文字", archive: "壓縮檔", book: "電子書", file: "檔案" };

  function size(n) {
    if (n < 1024) return `${n} B`;
    const u = ["KB", "MB", "GB", "TB"];
    let v = n / 1024;
    let i = 0;
    while (v >= 1024 && i < u.length - 1) {
      v /= 1024;
      i++;
    }
    return `${v < 10 ? v.toFixed(1) : Math.round(v)} ${u[i]}`;
  }
  function when(sec) {
    if (!sec) return "";
    const d = new Date(sec * 1000);
    const now = new Date();
    const hm = `${String(d.getHours()).padStart(2, "0")}:${String(d.getMinutes()).padStart(2, "0")}`;
    if (d.toDateString() === now.toDateString()) return `今天 ${hm}`;
    const y = new Date(now);
    y.setDate(now.getDate() - 1);
    if (d.toDateString() === y.toDateString()) return `昨天 ${hm}`;
    const md = `${d.getMonth() + 1}/${d.getDate()}`;
    return d.getFullYear() === now.getFullYear() ? `${md} ${hm}` : `${d.getFullYear()}/${md}`;
  }

  async function req(url, opts = {}) {
    const r = await fetch(url, { cache: "no-store", ...opts });
    if (!r.ok) {
      const e = new Error((await r.text().catch(() => "")) || `${r.status} ${r.statusText}`);
      e.status = r.status;
      throw e;
    }
    return (r.headers.get("content-type") || "").includes("json") ? r.json() : r.text();
  }
  const post = (url, body, method = "POST") =>
    req(url, { method, headers: { "content-type": "application/json" }, body: JSON.stringify(body) });

  const PREF_KEY = "cng6.files";
  function loadPref(key = PREF_KEY) {
    try {
      return JSON.parse(localStorage.getItem(key)) || {};
    } catch {
      return {};
    }
  }
  function savePref(p, key = PREF_KEY) {
    try {
      localStorage.setItem(key, JSON.stringify(p));
    } catch {
      /* 略過 */
    }
  }

  // 複製 / 剪下的內容, 同一個頁面裡的視窗共用
  let clipboard = null; // { mode: "copy" | "cut", root, dir, names }

  // ======================================================================
  // base: 操作別台時的 API 前綴(例如 /api/cluster/node/pi4), node: 那台的名稱
  function mount(el, { device = "pc", base = "", node = null } = {}) {
    const mobile = device === "mobile";
    el.classList.add("fm-root", mobile ? "fm-mobile" : "fm-pc");
    el.tabIndex = -1;
    const prefKey = node ? `${PREF_KEY}:${node}` : PREF_KEY;
    const pref = loadPref(prefKey);
    let roots = [];
    let root = pref.root || 0;
    let path = pref.path || "";
    let entries = [];
    let mode = "files"; // files | trash
    let trashItems = [];
    let sort = pref.sort || { key: "name", asc: true };
    let showHidden = !!pref.hidden;
    let filter = "";
    const selected = new Set();
    let anchor = null; // shift 選取的起點
    let selecting = false; // 手機: 選取模式
    const history = [];
    let future = [];
    let closed = false;
    let overlay = null; // 目前開著的預覽 / 編輯器(關閉函式)

    el.innerHTML = `
      <div class="fm">
        <aside class="fm-side"></aside>
        <section class="fm-main">
          <header class="fm-bar">
            <div class="fm-nav">
              <button class="fm-ib" data-act="back" title="上一頁">${ICON.back}</button>
              <button class="fm-ib" data-act="fwd" title="下一頁">${ICON.fwd}</button>
              <button class="fm-ib" data-act="up" title="上一層">${ICON.up}</button>
            </div>
            <nav class="fm-crumbs"></nav>
            <div class="fm-tools">
              <input type="search" class="fm-filter" placeholder="篩選" spellcheck="false">
              <button class="fm-ib" data-act="paste" title="貼上" hidden>${ICON.paste}</button>
              <button class="fm-ib" data-act="mkdir" title="新增資料夾">${ICON.newdir}</button>
              <button class="fm-ib" data-act="newfile" title="新增檔案">${ICON.newfile}</button>
              <label class="fm-ib" title="上傳">${ICON.upload}<input type="file" multiple hidden></label>
              <button class="fm-ib" data-act="more" title="更多">${ICON.more}</button>
            </div>
          </header>
          <div class="fm-selbar" hidden></div>
          <div class="fm-list" tabindex="-1"></div>
          <footer class="fm-status"></footer>
        </section>
        <div class="fm-drop" hidden>放開以上傳到這個資料夾</div>
      </div>`;
    const side = el.querySelector(".fm-side");
    const crumbs = el.querySelector(".fm-crumbs");
    const list = el.querySelector(".fm-list");
    const status = el.querySelector(".fm-status");
    const selbar = el.querySelector(".fm-selbar");
    const filterEl = el.querySelector(".fm-filter");
    const fileInput = el.querySelector('input[type="file"]');
    const pasteBtn = el.querySelector('[data-act="paste"]');
    const dropEl = el.querySelector(".fm-drop");

    // ---------------------------------------------------------------- 提示
    function toast(text, ok = true) {
      let t = el.querySelector(".fm-toast");
      if (!t) {
        t = h("div", "fm-toast");
        el.appendChild(t);
      }
      t.textContent = text;
      t.dataset.ok = ok;
      t.classList.add("show");
      clearTimeout(t._timer);
      t._timer = setTimeout(() => t.classList.remove("show"), ok ? 2400 : 4200);
    }
    function report(res) {
      if (res.errors?.length) toast(`${res.message}; 失敗: ${res.errors.join("; ")}`, false);
      else toast(res.message);
    }

    // ---------------------------------------------------------------- 對話框
    function dialog({ title, text, input, okText = "確定", danger = false, select }) {
      return new Promise((resolve) => {
        const wrap = h("div", "fm-modal");
        const box = h("div", "fm-dialog");
        box.append(h("h3", null, title));
        if (text) box.append(h("p", null, text));
        let inp = null;
        if (input != null) {
          inp = h("input", "fm-input");
          inp.value = input;
          inp.spellcheck = false;
          box.append(inp);
        }
        const btns = h("div", "fm-dialog-btns");
        const cancel = h("button", "fm-btn", "取消");
        const ok = h("button", `fm-btn primary ${danger ? "danger" : ""}`, okText);
        btns.append(cancel, ok);
        box.append(btns);
        wrap.append(box);
        el.append(wrap);
        const done = (v) => {
          wrap.remove();
          el.focus();
          resolve(v);
        };
        cancel.addEventListener("click", () => done(null));
        wrap.addEventListener("click", (e) => e.target === wrap && done(null));
        ok.addEventListener("click", () => done(inp ? inp.value.trim() : true));
        box.addEventListener("keydown", (e) => {
          e.stopPropagation();
          if (e.key === "Enter" && !e.isComposing) ok.click();
          else if (e.key === "Escape") done(null);
        });
        setTimeout(() => {
          if (inp) {
            inp.focus();
            // 改名時先選檔名(不含副檔名)
            const end = select ?? inp.value.length;
            inp.setSelectionRange(0, end);
          } else ok.focus();
        }, 30);
      });
    }

    // ---------------------------------------------------------------- 選單
    function menu(items, x, y) {
      closeMenu();
      const m = h("div", mobile ? "fm-sheet" : "fm-menu");
      const wrap = h("div", "fm-menu-wrap");
      for (const it of items) {
        if (it === "-") {
          m.append(h("div", "fm-menu-sep"));
          continue;
        }
        if (!it) continue;
        const b = h("button", `fm-menu-item ${it.danger ? "danger" : ""}`, it.label);
        if (it.hint && !mobile) b.append(h("span", "fm-menu-hint", it.hint));
        b.disabled = !!it.disabled;
        b.addEventListener("click", () => {
          closeMenu();
          it.run();
        });
        m.append(b);
      }
      if (mobile) m.append(Object.assign(h("button", "fm-menu-item cancel", "取消"), { onclick: closeMenu }));
      wrap.append(m);
      wrap.addEventListener("click", (e) => e.target === wrap && closeMenu());
      wrap.addEventListener("contextmenu", (e) => {
        e.preventDefault();
        closeMenu();
      });
      el.append(wrap);
      if (!mobile) {
        const r = el.getBoundingClientRect();
        const mw = m.offsetWidth;
        const mh = m.offsetHeight;
        m.style.left = `${Math.max(4, Math.min(x - r.left, r.width - mw - 4))}px`;
        m.style.top = `${Math.max(4, Math.min(y - r.top, r.height - mh - 4))}px`;
      }
    }
    function closeMenu() {
      el.querySelector(".fm-menu-wrap")?.remove();
    }

    // ---------------------------------------------------------------- 載入
    function remember() {
      savePref({ root, path, sort, hidden: showHidden }, prefKey);
    }

    async function loadRoots() {
      try {
        roots = await req(`${base}/api/files/roots`);
      } catch (e) {
        list.replaceChildren(h("div", "fm-empty", `無法讀取根目錄: ${e.message}`));
        return;
      }
      if (root >= roots.length) {
        root = 0;
        path = "";
      }
      renderSide();
    }

    function renderSide() {
      side.replaceChildren();
      if (!mobile) side.append(h("div", "fm-side-title", "位置"));
      roots.forEach((r, i) => {
        const b = h("button", `fm-side-item ${mode === "files" && i === root ? "on" : ""}`);
        b.innerHTML = i === 0 ? ICON.home : ICON.drive;
        b.append(h("span", null, r.name));
        b.title = r.path;
        if (!r.ok) b.classList.add("off");
        b.addEventListener("click", () => go(i, ""));
        dropTarget(b, () => ({ root: i, path: "" }));
        side.append(b);
      });
      const t = h("button", `fm-side-item ${mode === "trash" ? "on" : ""}`);
      t.innerHTML = ICON.trash;
      t.append(h("span", null, "資源回收桶"));
      t.addEventListener("click", showTrash);
      // 拖到回收桶 = 刪除
      t.addEventListener("dragover", (e) => {
        if (e.dataTransfer.types.includes("application/x-cng6-files")) {
          e.preventDefault();
          t.classList.add("drop");
        }
      });
      t.addEventListener("dragleave", () => t.classList.remove("drop"));
      t.addEventListener("drop", (e) => {
        t.classList.remove("drop");
        const d = dragData(e);
        if (!d) return;
        e.preventDefault();
        trashPaths(d.root, d.names.map((n) => join(d.dir, n)));
      });
      side.append(t);
    }

    async function go(r, p, { push = true } = {}) {
      if (push && (r !== root || p !== path || mode !== "files")) {
        history.push({ root, path, mode });
        future = [];
      }
      mode = "files";
      root = r;
      path = p;
      selected.clear();
      anchor = null;
      selecting = false;
      filter = "";
      filterEl.value = "";
      remember();
      renderSide();
      await refresh();
    }

    async function refresh(keepScroll = false) {
      if (mode === "trash") return loadTrash(keepScroll);
      const top = list.scrollTop;
      try {
        const res = await req(`${base}/api/files/list?${qs({ root, path })}`);
        entries = res.entries;
      } catch (e) {
        entries = [];
        renderCrumbs();
        list.replaceChildren(h("div", "fm-empty", e.message));
        status.textContent = "";
        // 資料夾不見了(被刪除、改名): 回到上一層
        if (e.status === 404 && path) {
          toast(`${e.message}: ${path}`, false);
          return go(root, parentOf(path), { push: false });
        }
        return;
      }
      // 不在列表裡的選取拿掉
      for (const n of [...selected]) if (!entries.some((e) => e.name === n)) selected.delete(n);
      render();
      if (keepScroll) list.scrollTop = top;
    }

    // ---------------------------------------------------------------- 畫面
    function visible() {
      const f = filter.toLowerCase();
      const out = entries.filter((e) => (showHidden || !e.name.startsWith(".")) && (!f || e.name.toLowerCase().includes(f)));
      const dir = sort.asc ? 1 : -1;
      const cmpName = (a, b) => a.name.localeCompare(b.name, "zh-Hant", { numeric: true, sensitivity: "base" });
      out.sort((a, b) => {
        if (a.dir !== b.dir) return a.dir ? -1 : 1; // 資料夾在前
        let c = 0;
        if (sort.key === "size") c = a.size - b.size;
        else if (sort.key === "mtime") c = a.mtime - b.mtime;
        else if (sort.key === "kind") c = kindOf(a).localeCompare(kindOf(b)) || extOf(a.name).localeCompare(extOf(b.name));
        return (c || cmpName(a, b)) * dir;
      });
      return out;
    }

    function renderCrumbs() {
      crumbs.replaceChildren();
      const navBtns = el.querySelector(".fm-nav");
      navBtns.querySelector('[data-act="back"]').disabled = !history.length;
      navBtns.querySelector('[data-act="fwd"]').disabled = !future.length;
      navBtns.querySelector('[data-act="up"]').disabled = mode !== "files" || !path;
      if (mode === "trash") {
        crumbs.append(h("span", "fm-crumb cur", "資源回收桶"));
        return;
      }
      const parts = path ? path.split("/") : [];
      const rootBtn = h("button", `fm-crumb ${parts.length ? "" : "cur"}`, roots[root]?.name || "");
      rootBtn.addEventListener("click", () => go(root, ""));
      dropTarget(rootBtn, () => ({ root, path: "" }));
      crumbs.append(rootBtn);
      parts.forEach((p, i) => {
        crumbs.append(h("span", "fm-crumb-sep", "›"));
        const sub = parts.slice(0, i + 1).join("/");
        const b = h("button", `fm-crumb ${i === parts.length - 1 ? "cur" : ""}`, p);
        b.addEventListener("click", () => go(root, sub));
        dropTarget(b, () => ({ root, path: sub }));
        crumbs.append(b);
      });
      // 手機: 捲到最後一層
      crumbs.scrollLeft = crumbs.scrollWidth;
    }

    function render() {
      renderCrumbs();
      pasteBtn.hidden = !myClip() || mode !== "files";
      el.querySelectorAll('[data-act="mkdir"], [data-act="newfile"], .fm-tools label').forEach((b) => (b.hidden = mode !== "files"));
      filterEl.hidden = mode !== "files";
      if (mode === "trash") return renderTrash();
      const items = visible();
      list.replaceChildren();
      list.classList.toggle("selecting", selecting || selected.size > 0);
      if (!mobile) {
        const head = h("div", "fm-row fm-head");
        for (const [key, label] of [["name", "名稱"], ["mtime", "修改日期"], ["size", "大小"], ["kind", "類型"]]) {
          const c = h("button", `fm-col fm-c-${key}`, label);
          if (sort.key === key) c.append(h("span", "fm-sort", sort.asc ? "▲" : "▼"));
          c.addEventListener("click", () => {
            sort = sort.key === key ? { key, asc: !sort.asc } : { key, asc: key === "name" || key === "kind" };
            remember();
            render();
          });
          head.append(c);
        }
        list.append(head);
      }
      if (!items.length) {
        list.append(h("div", "fm-empty", filter ? "沒有符合的項目" : "這個資料夾是空的"));
      }
      for (const e of items) list.append(row(e));
      const hidden = entries.length - entries.filter((e) => showHidden || !e.name.startsWith(".")).length;
      const total = items.filter((e) => !e.dir).reduce((s, e) => s + e.size, 0);
      status.textContent =
        `${items.length} 個項目` + (total ? ` · ${size(total)}` : "") + (hidden ? ` · ${hidden} 個隱藏` : "") + (clipboard ? ` · 剪貼簿 ${clipboard.names.length} 項(${clipboard.mode === "cut" ? "剪下" : "複製"})` : "");
      renderSelbar();
    }

    function row(e) {
      const kind = kindOf(e);
      const r = h("div", `fm-row ${selected.has(e.name) ? "sel" : ""} ${e.name.startsWith(".") ? "hidden-file" : ""}`);
      r.dataset.name = e.name;
      if (myClip()?.mode === "cut" && clipboard.root === root && clipboard.dir === path && clipboard.names.includes(e.name)) r.classList.add("cut");
      const name = h("div", "fm-col fm-c-name");
      const check = h("span", "fm-check");
      check.innerHTML = ICON.check;
      name.innerHTML = typeIcon(kind);
      name.prepend(check);
      const label = h("span", "fm-name", e.name);
      if (e.link) label.append(h("span", "fm-link", "↪"));
      name.append(label);
      if (mobile) {
        const sub = h("span", "fm-sub", e.dir ? when(e.mtime) : `${size(e.size)} · ${when(e.mtime)}`);
        name.append(sub);
        const more = h("button", "fm-ib fm-row-more");
        more.innerHTML = ICON.more;
        more.addEventListener("click", (ev) => {
          ev.stopPropagation();
          if (!selected.has(e.name)) {
            selected.clear();
            selected.add(e.name);
          }
          itemMenu(e, 0, 0);
        });
        r.append(name, more);
      } else {
        r.append(name, h("div", "fm-col fm-c-mtime", when(e.mtime)), h("div", "fm-col fm-c-size", e.dir ? "—" : size(e.size)), h("div", "fm-col fm-c-kind", KIND_LABEL[kind]));
      }

      // 點選
      r.addEventListener("click", (ev) => {
        if (mobile) {
          if (selecting || ev.target.closest(".fm-check")) {
            toggle(e.name);
            if (!selected.size) selecting = false;
            return render();
          }
          return open(e);
        }
        if (ev.shiftKey && anchor != null) {
          const names = visible().map((x) => x.name);
          const a = names.indexOf(anchor);
          const b = names.indexOf(e.name);
          if (!(ev.ctrlKey || ev.metaKey)) selected.clear();
          for (let i = Math.min(a, b); i <= Math.max(a, b); i++) selected.add(names[i]);
        } else if (ev.ctrlKey || ev.metaKey) {
          toggle(e.name);
          anchor = e.name;
        } else {
          selected.clear();
          selected.add(e.name);
          anchor = e.name;
        }
        updateSelection();
      });
      if (!mobile) r.addEventListener("dblclick", () => open(e));
      r.addEventListener("contextmenu", (ev) => {
        ev.preventDefault();
        if (!selected.has(e.name)) {
          selected.clear();
          selected.add(e.name);
          anchor = e.name;
          updateSelection();
        }
        itemMenu(e, ev.clientX, ev.clientY);
      });
      // 手機: 長按進入選取模式
      if (mobile) {
        let timer = null;
        let moved = false;
        r.addEventListener("touchstart", () => {
          moved = false;
          timer = setTimeout(() => {
            if (moved) return;
            selecting = true;
            selected.add(e.name);
            navigator.vibrate?.(15);
            render();
            r.dataset.longpress = "1";
          }, 500);
        }, { passive: true });
        r.addEventListener("touchmove", () => {
          moved = true;
          clearTimeout(timer);
        }, { passive: true });
        r.addEventListener("touchend", (ev) => {
          clearTimeout(timer);
          if (r.dataset.longpress) {
            delete r.dataset.longpress;
            ev.preventDefault(); // 長按後不要觸發 click
          }
        });
      }

      // 拖放: 拖出去搬移, 資料夾可以接收
      if (!mobile) {
        r.draggable = true;
        r.addEventListener("dragstart", (ev) => {
          if (!selected.has(e.name)) {
            selected.clear();
            selected.add(e.name);
            updateSelection();
          }
          ev.dataTransfer.effectAllowed = "copyMove";
          ev.dataTransfer.setData("application/x-cng6-files", JSON.stringify({ root, dir: path, names: [...selected] }));
        });
        if (e.dir) dropTarget(r, () => ({ root, path: join(path, e.name) }), e.name);
      }
      return r;
    }

    function toggle(name) {
      if (selected.has(name)) selected.delete(name);
      else selected.add(name);
    }

    /** 只更新選取狀態(不重畫整個列表) */
    function updateSelection() {
      for (const r of list.querySelectorAll(".fm-row[data-name]")) r.classList.toggle("sel", selected.has(r.dataset.name));
      list.classList.toggle("selecting", selecting || selected.size > 0);
      if (mode === "trash") renderTrashSelbar();
      else renderSelbar();
    }

    function selectedEntries() {
      return entries.filter((e) => selected.has(e.name));
    }

    function renderSelbar() {
      const sel = selectedEntries();
      selbar.hidden = !sel.length || (!mobile && sel.length < 2 && !selecting);
      if (selbar.hidden) return;
      selbar.replaceChildren(h("span", "fm-selcount", `已選 ${sel.length} 項`));
      const add = (label, fn, cls = "") => {
        const b = h("button", `fm-btn small ${cls}`, label);
        b.addEventListener("click", fn);
        selbar.append(b);
      };
      add("下載", () => download(sel));
      add("複製", () => setClipboard("copy"));
      add("剪下", () => setClipboard("cut"));
      if (sel.length === 1) add("改名", () => rename(sel[0]));
      add("刪除", () => trashSelected(), "danger");
      add(mobile ? "取消" : "取消選取", () => {
        selected.clear();
        selecting = false;
        render();
      });
      if (mobile) {
        const all = h("button", "fm-btn small", "全選");
        all.addEventListener("click", () => {
          for (const e of visible()) selected.add(e.name);
          render();
        });
        selbar.insertBefore(all, selbar.children[1]);
      }
    }

    // ---------------------------------------------------------------- 拖放
    function dragData(e) {
      try {
        return JSON.parse(e.dataTransfer.getData("application/x-cng6-files"));
      } catch {
        return null;
      }
    }
    /** 可以放東西進去的地方(資料夾、路徑、側邊欄) */
    function dropTarget(node, where, selfName) {
      if (mobile) return;
      node.addEventListener("dragover", (e) => {
        const internal = e.dataTransfer.types.includes("application/x-cng6-files");
        const files = e.dataTransfer.types.includes("Files");
        if (!internal && !files) return;
        // 不能拖到自己身上
        if (internal && selfName && selected.has(selfName)) return;
        e.preventDefault();
        e.stopPropagation();
        e.dataTransfer.dropEffect = internal && !(e.ctrlKey || e.altKey) ? "move" : "copy";
        node.classList.add("drop");
      });
      node.addEventListener("dragleave", () => node.classList.remove("drop"));
      node.addEventListener("drop", (e) => {
        node.classList.remove("drop");
        const to = where();
        if (e.dataTransfer.files.length && !e.dataTransfer.types.includes("application/x-cng6-files")) {
          e.preventDefault();
          e.stopPropagation();
          dropEl.hidden = true;
          return uploadFiles([...e.dataTransfer.files], to.root, to.path);
        }
        const d = dragData(e);
        if (!d) return;
        e.preventDefault();
        e.stopPropagation();
        transfer(e.ctrlKey || e.altKey ? "copy" : "move", d, to);
      });
    }

    // 拖檔案進來上傳(放在目前的資料夾)
    if (!mobile) {
      let depth = 0;
      el.addEventListener("dragenter", (e) => {
        if (mode !== "files" || !e.dataTransfer.types.includes("Files") || e.dataTransfer.types.includes("application/x-cng6-files")) return;
        depth++;
        dropEl.hidden = false;
      });
      el.addEventListener("dragleave", () => {
        if (--depth <= 0) {
          depth = 0;
          dropEl.hidden = true;
        }
      });
      el.addEventListener("dragover", (e) => {
        if (mode === "files" && e.dataTransfer.types.includes("Files")) e.preventDefault();
      });
      el.addEventListener("drop", (e) => {
        depth = 0;
        dropEl.hidden = true;
        if (mode !== "files" || !e.dataTransfer.files.length || e.dataTransfer.types.includes("application/x-cng6-files")) return;
        e.preventDefault();
        uploadFiles([...e.dataTransfer.files], root, path);
      });
    }

    // ---------------------------------------------------------------- 操作
    async function op(body) {
      try {
        const res = await post(`${base}/api/files/op`, body);
        report(res);
        return res;
      } catch (e) {
        toast(e.message, false);
        return null;
      }
    }

    async function mkdir() {
      const name = await dialog({ title: "新增資料夾", input: "新資料夾", okText: "建立" });
      if (!name) return;
      const res = await op({ op: "mkdir", root, path, name });
      if (res) {
        await refresh(true);
        selectOnly(res.name);
      }
    }

    async function newFile() {
      const name = await dialog({ title: "新增檔案", input: "新檔案.txt", okText: "建立", select: 3 });
      if (!name) return;
      const res = await op({ op: "newfile", root, path, name });
      if (res) {
        await refresh(true);
        selectOnly(res.name);
        const e = entries.find((x) => x.name === res.name);
        if (e) editText(e);
      }
    }

    function selectOnly(name) {
      if (!name) return;
      selected.clear();
      selected.add(name);
      anchor = name;
      updateSelection();
      list.querySelector(`.fm-row[data-name="${CSS.escape(name)}"]`)?.scrollIntoView({ block: "nearest" });
    }

    async function rename(e) {
      const dot = e.dir ? -1 : e.name.lastIndexOf(".");
      const name = await dialog({ title: "重新命名", input: e.name, okText: "改名", select: dot > 0 ? dot : e.name.length });
      if (!name || name === e.name) return;
      const res = await op({ op: "rename", root, path: join(path, e.name), name });
      if (res) {
        await refresh(true);
        selectOnly(res.name);
      }
    }

    async function trashPaths(r, paths) {
      if (!paths.length) return;
      const what = paths.length === 1 ? `「${paths[0].split("/").pop()}」` : `${paths.length} 個項目`;
      const ok = await dialog({ title: "移到資源回收桶", text: `要把${what}移到資源回收桶嗎? 之後可以從回收桶還原。`, okText: "移到回收桶", danger: true });
      if (!ok) return;
      const res = await op({ op: "trash", root: r, paths });
      if (res) {
        selected.clear();
        refresh(true);
      }
    }
    const trashSelected = () => trashPaths(root, selectedEntries().map((e) => join(path, e.name)));

    function setClipboard(m) {
      const names = selectedEntries().map((e) => e.name);
      if (!names.length) return;
      clipboard = { mode: m, root, dir: path, names, base };
      toast(`已${m === "cut" ? "剪下" : "複製"} ${names.length} 項, 到目的地按「貼上」`);
      if (mobile) {
        selected.clear();
        selecting = false;
      }
      render();
    }

    // 剪貼簿只能在同一台機器裡貼上
    const myClip = () => (clipboard && clipboard.base === base ? clipboard : null);

    async function paste() {
      if (!myClip()) return;
      const c = clipboard;
      // 剪下再貼回同一個資料夾: 什麼都不用做
      if (c.mode === "cut" && c.root === root && c.dir === path) {
        clipboard = null;
        return render();
      }
      const res = await transfer(c.mode === "cut" ? "move" : "copy", c, { root, path });
      if (res && c.mode === "cut") clipboard = null;
      render();
    }

    async function transfer(kind, src, to) {
      if (src.root === to.root && src.dir === to.path && kind === "move") return null;
      toast(`${kind === "move" ? "搬移" : "複製"}中…`);
      const res = await op({ op: kind, root: src.root, paths: src.names.map((n) => join(src.dir, n)), to_root: to.root, to: to.path });
      if (res) refresh(true);
      return res;
    }

    function rawUrl(e, dl = false) {
      return `${base}/files/raw?${qs({ root, path: join(path, e.name), ...(dl ? { dl: 1 } : {}) })}`;
    }

    function download(sel) {
      if (!sel.length) return;
      const a = h("a");
      if (sel.length === 1 && !sel[0].dir) a.href = rawUrl(sel[0], true);
      else {
        a.href = `${base}/files/zip?${qs({ root, path, names: JSON.stringify(sel.map((e) => e.name)) })}`;
        toast("壓縮中, 下載馬上開始…");
      }
      a.download = "";
      document.body.append(a);
      a.click();
      a.remove();
    }

    async function unzip(e) {
      toast("解壓縮中…");
      const res = await op({ op: "unzip", root, path: join(path, e.name) });
      if (res) {
        await refresh(true);
        selectOnly(res.name);
      }
    }

    async function info(e) {
      const rows = [
        ["名稱", e.name],
        ["類型", KIND_LABEL[kindOf(e)] + (e.link ? "(捷徑)" : "")],
        ["位置", `${roots[root]?.path || ""}/${path}`.replace(/\/$/, "")],
        ["修改時間", e.mtime ? new Date(e.mtime * 1000).toLocaleString("zh-TW", { hour12: false }) : ""],
      ];
      if (!e.dir) rows.push(["大小", `${size(e.size)}${e.size >= 1024 ? `(${e.size.toLocaleString()} 位元組)` : ""}`]);
      const wrap = h("div", "fm-modal");
      const box = h("div", "fm-dialog fm-info");
      box.innerHTML = typeIcon(kindOf(e));
      box.append(h("h3", null, e.name));
      const tbl = h("dl");
      const usageEl = h("dd", null, "計算中…");
      for (const [k, v] of rows) tbl.append(h("dt", null, k), h("dd", null, v));
      if (e.dir) tbl.append(h("dt", null, "內容"), usageEl);
      box.append(tbl);
      const btns = h("div", "fm-dialog-btns");
      const ok = h("button", "fm-btn primary", "關閉");
      btns.append(ok);
      box.append(btns);
      wrap.append(box);
      el.append(wrap);
      const close = () => wrap.remove();
      ok.addEventListener("click", close);
      wrap.addEventListener("click", (ev) => ev.target === wrap && close());
      ok.focus();
      if (e.dir) {
        try {
          const u = await req(`${base}/api/files/usage?${qs({ root, path: join(path, e.name) })}`);
          usageEl.textContent = `${u.files.toLocaleString()} 個檔案, ${size(u.bytes)}${u.partial ? "(項目太多, 只算了一部分)" : ""}`;
        } catch (err) {
          usageEl.textContent = err.message;
        }
      }
    }

    function itemMenu(e, x, y) {
      const sel = selectedEntries();
      const many = sel.length > 1;
      const kind = kindOf(e);
      menu(
        [
          !many && { label: e.dir ? "開啟" : "開啟 / 預覽", run: () => open(e), hint: "Enter" },
          !many && kind === "file" && { label: "以文字編輯", run: () => editText(e) },
          { label: many || e.dir ? "壓縮下載" : "下載", run: () => download(sel.length ? sel : [e]) },
          !many && !e.dir && kind !== "archive" && { label: "壓縮下載", run: () => download([e]) },
          !many && extOf(e.name) === "zip" && { label: "解壓縮到這裡", run: () => unzip(e) },
          "-",
          { label: "複製", run: () => setClipboard("copy"), hint: "Ctrl+C" },
          { label: "剪下", run: () => setClipboard("cut"), hint: "Ctrl+X" },
          myClip() && e.dir && !many && { label: `貼上到「${e.name}」`, run: () => transfer(clipboard.mode === "cut" ? "move" : "copy", clipboard, { root, path: join(path, e.name) }).then((r) => r && clipboard?.mode === "cut" && (clipboard = null)) },
          !many && { label: "重新命名", run: () => rename(e), hint: "F2" },
          !many && { label: "內容", run: () => info(e) },
          !base && !sel.some((x) => x.dir) && { label: "傳送到其他機器…", run: () => sendTo(sel.length ? sel : [e]) },
          "-",
          { label: many ? `刪除 ${sel.length} 項` : "刪除", run: trashSelected, danger: true, hint: "Del" },
        ],
        x,
        y
      );
    }

    function folderMenu(x, y) {
      menu(
        [
          { label: "新增資料夾", run: mkdir },
          { label: "新增檔案", run: newFile },
          { label: "上傳檔案…", run: () => fileInput.click() },
          myClip() && { label: `貼上 ${clipboard.names.length} 項`, run: paste, hint: "Ctrl+V" },
          "-",
          { label: "全選", run: selectAll, hint: "Ctrl+A" },
          { label: showHidden ? "不顯示隱藏檔" : "顯示隱藏檔", run: toggleHidden, hint: "Ctrl+H" },
          { label: "重新整理", run: () => refresh(true) },
          path && { label: "壓縮下載這個資料夾", run: () => downloadFolder() },
        ],
        x,
        y
      );
    }

    /** 傳到 cluster 裡的另一台(放在對方的「收到的檔案」) */
    async function sendTo(sel) {
      let s;
      try {
        s = await req("/api/cluster");
      } catch {
        return toast("無法讀取 cluster", false);
      }
      const nodes = (s.nodes || []).filter((n) => !n.me && n.online && !n.stale);
      if (!s.connected || !nodes.length) return toast("cluster 沒有其他上線的機器", false);
      menu(
        nodes.map((n) => ({
          label: `傳到 ${n.name}`,
          run: async () => {
            toast(`傳送中… ${sel.length} 個檔案到 ${n.name}`);
            try {
              const r = await post("/api/cluster/send", { node: n.name, root, paths: sel.map((x) => join(path, x.name)) });
              report(r);
            } catch (err) {
              toast(err.message, false);
            }
          },
        })),
        innerWidth / 2,
        innerHeight / 3
      );
    }

    function selectAll() {
      for (const e of visible()) selected.add(e.name);
      updateSelection();
    }
    function toggleHidden() {
      showHidden = !showHidden;
      remember();
      render();
      toast(showHidden ? "顯示隱藏檔" : "不顯示隱藏檔");
    }
    function downloadFolder() {
      const a = h("a");
      a.href = `${base}/files/zip?${qs({ root, path: parentOf(path), names: JSON.stringify([path.split("/").pop()]) })}`;
      a.download = "";
      document.body.append(a);
      a.click();
      a.remove();
    }

    // ---------------------------------------------------------------- 上傳
    function uploadOne(file, r, dir, onProgress) {
      return new Promise((resolve, reject) => {
        const x = new XMLHttpRequest();
        x.open("PUT", `${base}/api/files/upload?${qs({ root: r, path: dir, name: file.name })}`);
        x.upload.onprogress = (e) => e.lengthComputable && onProgress(e.loaded / e.total);
        x.onload = () => (x.status < 300 ? resolve(JSON.parse(x.responseText)) : reject(new Error(x.responseText || x.statusText)));
        x.onerror = () => reject(new Error("連線中斷"));
        x.send(file);
      });
    }
    async function uploadFiles(files, r = root, dir = path) {
      if (!files.length) return;
      let ok = 0;
      const errors = [];
      for (let i = 0; i < files.length; i++) {
        const f = files[i];
        const label = files.length > 1 ? `(${i + 1}/${files.length}) ` : "";
        try {
          await uploadOne(f, r, dir, (p) => toast(`上傳中 ${label}${f.name} ${Math.round(p * 100)}%`));
          ok++;
        } catch (e) {
          errors.push(`${f.name}: ${e.message}`);
        }
      }
      if (errors.length) toast(`上傳 ${ok} 個成功; 失敗: ${errors.join("; ")}`, false);
      else toast(`已上傳 ${ok} 個檔案`);
      if (r === root && dir === path && mode === "files") refresh(true);
    }
    fileInput.addEventListener("change", () => {
      const files = [...fileInput.files];
      fileInput.value = "";
      uploadFiles(files);
    });

    // ---------------------------------------------------------------- 開啟 / 預覽
    function open(e) {
      if (e.dir) return go(root, join(path, e.name));
      const kind = kindOf(e);
      if (kind === "image") return viewMedia(e, "image");
      if (kind === "video" || kind === "audio") return viewMedia(e, kind);
      if (kind === "pdf") {
        // 手機的內嵌 PDF 只顯示第一頁, 直接開新分頁
        if (mobile) return window.open(rawUrl(e), "_blank", "noopener");
        return viewMedia(e, "pdf");
      }
      if (kind === "text" || kind === "code") return editText(e);
      if (kind === "archive" && extOf(e.name) === "zip") {
        return menu([{ label: "解壓縮到這裡", run: () => unzip(e) }, { label: "下載", run: () => download([e]) }, { label: "內容", run: () => info(e) }], innerWidth / 2, innerHeight / 3);
      }
      // 不知道的類型: 小檔案試著當文字開, 否則顯示資訊
      if (e.size < 512 * 1024) return editText(e, true);
      info(e);
    }

    /** 全畫面的覆蓋層(預覽、編輯器) */
    function sheet(title, onClose) {
      const wrap = h("div", "fm-viewer");
      const bar = h("header", "fm-viewer-bar");
      const closeBtn = h("button", "fm-ib", null);
      closeBtn.innerHTML = ICON.close;
      closeBtn.title = "關閉 (Esc)";
      const t = h("div", "fm-viewer-title", title);
      const actions = h("div", "fm-viewer-actions");
      bar.append(closeBtn, t, actions);
      const body = h("div", "fm-viewer-body");
      wrap.append(bar, body);
      el.append(wrap);
      const close = async () => {
        if (onClose && (await onClose()) === false) return;
        wrap.remove();
        overlay = null;
        el.focus();
      };
      closeBtn.addEventListener("click", close);
      overlay = close;
      return { wrap, body, actions, t, close };
    }

    function viewMedia(e, kind) {
      // 圖片可以在同一個資料夾裡左右切換
      const siblings = kind === "image" ? visible().filter((x) => kindOf(x) === "image") : [e];
      let idx = Math.max(0, siblings.findIndex((x) => x.name === e.name));
      const s = sheet(e.name);
      s.wrap.classList.add(`fm-v-${kind}`);
      const dl = h("button", "fm-btn small", "下載");
      dl.addEventListener("click", () => download([siblings[idx]]));
      s.actions.append(dl);
      let media;
      const show = () => {
        const cur = siblings[idx];
        s.t.textContent = siblings.length > 1 ? `${cur.name}  (${idx + 1}/${siblings.length})` : cur.name;
        s.body.replaceChildren();
        if (kind === "image") {
          media = h("img", "fm-media");
          media.alt = cur.name;
          media.src = rawUrl(cur);
          media.addEventListener("error", () => s.body.replaceChildren(h("div", "fm-empty", "這張圖片無法在瀏覽器顯示")));
        } else if (kind === "video") {
          media = h("video", "fm-media");
          media.controls = true;
          media.autoplay = true;
          media.playsInline = true;
          media.src = rawUrl(cur);
          media.addEventListener("error", () => s.body.append(h("div", "fm-empty", "瀏覽器不支援這個影片格式, 請下載後播放")));
        } else if (kind === "audio") {
          const box = h("div", "fm-audio");
          box.innerHTML = typeIcon("audio");
          box.append(h("div", "fm-audio-name", cur.name));
          media = h("audio");
          media.controls = true;
          media.autoplay = true;
          media.src = rawUrl(cur);
          box.append(media);
          s.body.append(box);
          return;
        } else {
          media = h("iframe", "fm-pdf");
          media.src = rawUrl(cur);
          media.title = cur.name;
        }
        s.body.append(media);
        if (siblings.length > 1) {
          const prev = h("button", "fm-nav-btn prev");
          prev.innerHTML = ICON.back;
          const next = h("button", "fm-nav-btn next");
          next.innerHTML = ICON.fwd;
          prev.addEventListener("click", () => step(-1));
          next.addEventListener("click", () => step(1));
          s.body.append(prev, next);
          // 預先載入下一張
          new Image().src = rawUrl(siblings[(idx + 1) % siblings.length]);
        }
      };
      const step = (d) => {
        idx = (idx + d + siblings.length) % siblings.length;
        show();
      };
      s.wrap.addEventListener("keydown", (ev) => {
        if (siblings.length > 1 && ev.key === "ArrowRight") step(1);
        else if (siblings.length > 1 && ev.key === "ArrowLeft") step(-1);
      });
      // 手機: 左右滑動
      let sx = 0;
      s.body.addEventListener("touchstart", (ev) => (sx = ev.touches[0].clientX), { passive: true });
      s.body.addEventListener("touchend", (ev) => {
        const dx = ev.changedTouches[0].clientX - sx;
        if (siblings.length > 1 && Math.abs(dx) > 60) step(dx < 0 ? 1 : -1);
      });
      s.wrap.tabIndex = -1;
      show();
      s.wrap.focus();
    }

    async function editText(e, guess = false) {
      const p = join(path, e.name);
      let file;
      try {
        file = await req(`${base}/api/files/text?${qs({ root, path: p })}`);
      } catch (err) {
        if (guess) return info(e);
        return toast(err.message, false);
      }
      let mtime = file.mtime;
      let saved = file.text;
      const s = sheet(e.name, async () => {
        if (ta.value === saved) return true;
        return !!(await dialog({ title: "還沒存檔", text: `「${e.name}」有修改還沒存檔, 要放棄修改嗎?`, okText: "放棄修改", danger: true }));
      });
      s.wrap.classList.add("fm-v-text");
      const note = h("span", "fm-viewer-note", file.encoding !== "UTF-8" ? `${file.encoding}(存檔會轉成 UTF-8)` : "");
      const saveBtn = h("button", "fm-btn small primary", "存檔");
      saveBtn.disabled = true;
      s.actions.append(note, saveBtn);
      const ta = h("textarea", "fm-editor");
      ta.value = file.text;
      ta.spellcheck = false;
      ta.setAttribute("autocapitalize", "off");
      ta.setAttribute("autocomplete", "off");
      s.body.append(ta);
      const dirty = () => {
        const d = ta.value !== saved;
        saveBtn.disabled = !d;
        s.t.textContent = d ? `● ${e.name}` : e.name;
      };
      ta.addEventListener("input", dirty);
      async function save(force = false) {
        saveBtn.disabled = true;
        try {
          const r = await post(`${base}/api/files/text?${qs({ root, path: p })}`, { text: ta.value, mtime, force }, "PUT");
          mtime = r.mtime;
          saved = ta.value;
          toast("已存檔");
          refresh(true);
        } catch (err) {
          if (err.status === 409) {
            const ok = await dialog({ title: "檔案被修改過", text: "這個檔案在別的地方被修改過, 要用目前的內容覆蓋嗎?", okText: "覆蓋", danger: true });
            if (ok) return save(true);
          } else toast(err.message, false);
        }
        dirty();
      }
      saveBtn.addEventListener("click", () => save());
      ta.addEventListener("keydown", (ev) => {
        if ((ev.ctrlKey || ev.metaKey) && ev.key.toLowerCase() === "s") {
          ev.preventDefault();
          if (ta.value !== saved) save();
        } else if (ev.key === "Tab" && !ev.ctrlKey && !ev.altKey && !ev.metaKey) {
          // Tab 插入 tab(保留復原紀錄)
          ev.preventDefault();
          document.execCommand("insertText", false, "\t");
        }
      });
      setTimeout(() => !mobile && ta.focus(), 30);
    }

    // ---------------------------------------------------------------- 回收桶
    async function showTrash() {
      if (mode !== "trash") {
        history.push({ root, path, mode });
        future = [];
      }
      mode = "trash";
      selected.clear();
      renderSide();
      await loadTrash();
    }

    async function loadTrash(keepScroll = false) {
      const top = list.scrollTop;
      try {
        trashItems = await req(`${base}/api/files/trash`);
      } catch (e) {
        trashItems = [];
        toast(e.message, false);
      }
      for (const k of [...selected]) if (!trashItems.some((t) => tkey(t) === k)) selected.delete(k);
      render();
      if (keepScroll) list.scrollTop = top;
    }
    const tkey = (t) => `${t.trash}:${t.id}`;
    /** 原本的位置: 用根目錄名稱表示, 例如「家目錄:/文件」 */
    function where(abs) {
      const r = roots
        .filter((x) => abs === x.path || abs.startsWith(`${x.path}/`))
        .sort((a, b) => b.path.length - a.path.length)[0];
      return r ? `${r.name}:/${abs.slice(r.path.length + 1)}` : abs;
    }

    function renderTrash() {
      list.replaceChildren();
      list.classList.toggle("selecting", selected.size > 0);
      const head = h("div", "fm-trash-head");
      head.append(h("span", null, "刪除的項目會留在這裡, 可以還原到原本的位置"));
      const empty = h("button", "fm-btn small danger", "清空回收桶");
      empty.disabled = !trashItems.length;
      empty.addEventListener("click", async () => {
        const ok = await dialog({ title: "清空資源回收桶", text: `要永久刪除回收桶裡的 ${trashItems.length} 個項目嗎? 這個動作無法復原。`, okText: "清空", danger: true });
        if (ok && (await op({ op: "empty" }))) loadTrash();
      });
      head.append(empty);
      list.append(head);
      if (!trashItems.length) list.append(h("div", "fm-empty", "回收桶是空的"));
      for (const t of trashItems) {
        const r = h("div", `fm-row fm-trash-row ${selected.has(tkey(t)) ? "sel" : ""}`);
        r.dataset.name = tkey(t);
        const name = h("div", "fm-col fm-c-name");
        const check = h("span", "fm-check");
        check.innerHTML = ICON.check;
        name.innerHTML = typeIcon(kindOf({ name: t.name, dir: t.dir }));
        name.prepend(check);
        const txt = h("span", "fm-name", t.name);
        name.append(txt, h("span", "fm-sub", `${where(t.path.slice(0, t.path.length - t.name.length - 1))} · 刪除於 ${t.deleted.replace("T", " ").slice(0, 16)}${t.dir ? "" : ` · ${size(t.size)}`}`));
        name.title = t.path;
        r.append(name);
        r.addEventListener("click", () => {
          toggle(tkey(t));
          updateSelection();
        });
        r.addEventListener("contextmenu", (ev) => {
          ev.preventDefault();
          if (!selected.has(tkey(t))) {
            selected.clear();
            selected.add(tkey(t));
            updateSelection();
          }
          trashMenu(ev.clientX, ev.clientY);
        });
        list.append(r);
      }
      status.textContent = `${trashItems.length} 個項目`;
      renderTrashSelbar();
    }

    function trashRefs() {
      return trashItems.filter((t) => selected.has(tkey(t))).map((t) => ({ trash: t.trash, id: t.id }));
    }
    async function restore() {
      const items = trashRefs();
      if (items.length && (await op({ op: "restore", items }))) {
        selected.clear();
        loadTrash(true);
      }
    }
    async function purge() {
      const items = trashRefs();
      if (!items.length) return;
      const ok = await dialog({ title: "永久刪除", text: `要永久刪除這 ${items.length} 個項目嗎? 這個動作無法復原。`, okText: "永久刪除", danger: true });
      if (ok && (await op({ op: "purge", items }))) {
        selected.clear();
        loadTrash(true);
      }
    }
    function trashMenu(x, y) {
      menu([{ label: "還原", run: restore }, "-", { label: "永久刪除", run: purge, danger: true }], x, y);
    }
    function renderTrashSelbar() {
      selbar.hidden = !selected.size;
      if (selbar.hidden) return;
      selbar.replaceChildren(h("span", "fm-selcount", `已選 ${selected.size} 項`));
      const add = (label, fn, cls = "") => {
        const b = h("button", `fm-btn small ${cls}`, label);
        b.addEventListener("click", fn);
        selbar.append(b);
      };
      add("還原", restore);
      add("永久刪除", purge, "danger");
      add("取消", () => {
        selected.clear();
        render();
      });
    }

    // ---------------------------------------------------------------- 按鈕與快速鍵
    el.querySelector('[data-act="back"]').addEventListener("click", back);
    el.querySelector('[data-act="fwd"]').addEventListener("click", forward);
    el.querySelector('[data-act="up"]').addEventListener("click", () => mode === "files" && path && go(root, parentOf(path)));
    el.querySelector('[data-act="mkdir"]').addEventListener("click", mkdir);
    el.querySelector('[data-act="newfile"]').addEventListener("click", newFile);
    pasteBtn.addEventListener("click", paste);
    el.querySelector('[data-act="more"]').addEventListener("click", (e) => {
      const r = e.currentTarget.getBoundingClientRect();
      if (mode === "trash") return trashMenu(r.left, r.bottom);
      folderMenu(r.right - 200, r.bottom + 4);
    });
    filterEl.addEventListener("input", () => {
      filter = filterEl.value;
      render();
    });
    filterEl.addEventListener("keydown", (e) => {
      e.stopPropagation();
      if (e.key === "Escape") {
        filterEl.value = "";
        filter = "";
        render();
        el.focus();
      }
    });
    // 空白處: 取消選取 / 右鍵資料夾選單
    list.addEventListener("click", (e) => {
      if (e.target === list || e.target.classList.contains("fm-empty")) {
        selected.clear();
        selecting = false;
        updateSelection();
      }
    });
    list.addEventListener("contextmenu", (e) => {
      if (e.target.closest(".fm-row[data-name]")) return;
      e.preventDefault();
      if (mode === "files") folderMenu(e.clientX, e.clientY);
    });

    async function back() {
      const prev = history.pop();
      if (!prev) return;
      future.push({ root, path, mode });
      if (prev.mode === "trash") {
        mode = "trash";
        renderSide();
        return loadTrash();
      }
      go(prev.root, prev.path, { push: false });
    }
    async function forward() {
      const next = future.pop();
      if (!next) return;
      history.push({ root, path, mode });
      if (next.mode === "trash") {
        mode = "trash";
        renderSide();
        return loadTrash();
      }
      mode = "files";
      root = next.root;
      path = next.path;
      selected.clear();
      remember();
      renderSide();
      refresh();
    }

    el.addEventListener("keydown", (e) => {
      if (closed || e.target.closest("input, textarea")) return;
      if (overlay) {
        if (e.key === "Escape") overlay();
        return;
      }
      if (el.querySelector(".fm-menu-wrap, .fm-modal")) {
        if (e.key === "Escape") closeMenu();
        return;
      }
      const ctrl = e.ctrlKey || e.metaKey;
      const k = e.key.toLowerCase();
      const items = mode === "files" ? visible() : [];
      const sel = selectedEntries();
      if (mode === "trash") {
        if (e.key === "Delete") purge();
        else if (e.key === "Backspace" || (e.altKey && e.key === "ArrowLeft")) back();
        return;
      }
      let handled = true;
      if (e.key === "Enter" && sel.length === 1) open(sel[0]);
      else if (e.key === "Delete" && sel.length) trashSelected();
      else if (e.key === "F2" && sel.length === 1) rename(sel[0]);
      else if (e.key === "Backspace" || (e.altKey && e.key === "ArrowUp")) path && go(root, parentOf(path));
      else if (e.altKey && e.key === "ArrowLeft") back();
      else if (e.altKey && e.key === "ArrowRight") forward();
      else if (ctrl && k === "a") selectAll();
      else if (ctrl && k === "c") setClipboard("copy");
      else if (ctrl && k === "x") setClipboard("cut");
      else if (ctrl && k === "v") paste();
      else if (ctrl && k === "h") toggleHidden();
      else if (e.key === "Escape") {
        selected.clear();
        updateSelection();
      } else if ((e.key === "ArrowDown" || e.key === "ArrowUp") && items.length) {
        const names = items.map((x) => x.name);
        const cur = anchor != null ? names.indexOf(anchor) : -1;
        const nextI = Math.max(0, Math.min(names.length - 1, cur + (e.key === "ArrowDown" ? 1 : -1)));
        if (!e.shiftKey) selected.clear();
        selected.add(names[nextI]);
        anchor = names[nextI];
        updateSelection();
        list.querySelector(`.fm-row[data-name="${CSS.escape(anchor)}"]`)?.scrollIntoView({ block: "nearest" });
      } else if (e.key.length === 1 && !ctrl && !e.altKey && items.length) {
        // 打字跳到開頭相同的項目
        const i = items.findIndex((x) => x.name.toLowerCase().startsWith(k));
        if (i >= 0) selectOnly(items[i].name);
        else handled = false;
      } else handled = false;
      if (handled) e.preventDefault();
    });

    // 視窗回到前景時更新(可能在別的地方改過)
    const onVisible = () => !document.hidden && !overlay && refresh(true);
    document.addEventListener("visibilitychange", onVisible);

    (async () => {
      await loadRoots();
      if (roots.length) await go(root, path, { push: false });
      if (!mobile) el.focus();
    })();

    return () => {
      closed = true;
      document.removeEventListener("visibilitychange", onVisible);
    };
  }

  return { mount };
})();

if (typeof Apps !== "undefined") Apps.registerView("files", FilesApp);

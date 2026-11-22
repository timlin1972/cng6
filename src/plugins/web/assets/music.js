// cng6 音樂: 網頁播放器(PC 視窗與手機共用), 下載與播放清單
//
// - 播放器只有一個(MusicPlayer), 關掉視窗或離開頁面也會繼續播
//   PC: 選單列顯示正在播放; 手機: 分頁列上方浮出迷你播放列
// - 歌詞: <名稱>.<語言>.srt|lrc|vtt, 依播放時間同步捲動
// - 下載、刪除、播放清單都執行 music 指令(和 CLI 相同, TUI 的 Message 會留紀錄)
"use strict";

const MusicPlayer = (() => {
  const audio = new Audio();
  audio.preload = "metadata";
  const STORE = "cng6.music";
  const listeners = new Set();
  const emit = () => listeners.forEach((f) => f());

  let lib = { tracks: [], playlists: [], downloads: [], version: 0 };
  const byFile = new Map();
  let queue = []; // 檔名
  let index = -1;
  let shuffle = false;
  let repeat = "all"; // off | all | one
  let source = "所有歌曲"; // 目前播放的來源(顯示用)

  try {
    const s = JSON.parse(localStorage.getItem(STORE)) || {};
    audio.volume = s.volume ?? 1;
    shuffle = !!s.shuffle;
    repeat = s.repeat || "all";
  } catch {
    /* 預設值 */
  }
  const save = () => {
    try {
      localStorage.setItem(STORE, JSON.stringify({ volume: audio.volume, shuffle, repeat }));
    } catch {
      /* 略過 */
    }
  };

  const enc = encodeURIComponent;
  const fileUrl = (f) => `/music/file/${enc(f)}`;
  const coverUrl = (t) => (t && t.cover ? `/music/cover/${enc(t.file)}` : null);

  function current() {
    return byFile.get(queue[index]) || null;
  }

  function setLibrary(data) {
    lib = data;
    byFile.clear();
    for (const t of data.tracks) byFile.set(t.file, t);
    // 刪掉的歌從佇列移除
    const cur = queue[index];
    queue = queue.filter((f) => byFile.has(f));
    index = cur && byFile.has(cur) ? queue.indexOf(cur) : Math.min(index, queue.length - 1);
    emit();
  }

  async function load() {
    const r = await fetch("/api/music", { cache: "no-store" });
    if (!r.ok) throw new Error(`HTTP ${r.status}`);
    setLibrary(await r.json());
    return lib;
  }

  function playAt(i) {
    if (i < 0 || i >= queue.length) return;
    index = i;
    const t = current();
    audio.src = fileUrl(t.file);
    audio.play().catch(() => {});
    updateSession();
    emit();
  }

  /** 播放一個清單, 從 file 開始 */
  function playList(files, file, name) {
    // 點的是正在播放的那首: 不重新播放, 只把佇列換成這個清單(之後的上一首/下一首依這個清單)
    if (file && queue[index] === file && audio.src) {
      queue = files.slice();
      source = name || "所有歌曲";
      index = queue.indexOf(file);
      if (audio.paused) audio.play().catch(() => {});
      emit();
      return;
    }
    queue = files.slice();
    source = name || "所有歌曲";
    let i = file ? queue.indexOf(file) : 0;
    if (shuffle && !file) i = Math.floor(Math.random() * queue.length);
    playAt(Math.max(0, i));
  }

  function next(auto = false) {
    if (!queue.length) return;
    if (auto && repeat === "one") return playAt(index);
    if (shuffle && queue.length > 1) {
      let i;
      do i = Math.floor(Math.random() * queue.length);
      while (i === index);
      return playAt(i);
    }
    if (index + 1 < queue.length) return playAt(index + 1);
    if (repeat === "all" || !auto) return playAt(0);
    audio.pause();
    emit();
  }

  function prev() {
    if (!queue.length) return;
    // 播超過 3 秒: 回到開頭
    if (audio.currentTime > 3) {
      audio.currentTime = 0;
      return;
    }
    playAt(index > 0 ? index - 1 : queue.length - 1);
  }

  function toggle() {
    if (!current()) {
      if (lib.tracks.length) playList(lib.tracks.map((t) => t.file), null, "所有歌曲");
      return;
    }
    if (audio.paused) audio.play().catch(() => {});
    else audio.pause();
  }

  audio.addEventListener("ended", () => next(true));
  ["play", "pause", "loadedmetadata", "volumechange"].forEach((e) => audio.addEventListener(e, emit));
  // 鎖定畫面: 播放狀態與進度
  ["play", "pause"].forEach((e) =>
    audio.addEventListener(e, () => {
      if ("mediaSession" in navigator) navigator.mediaSession.playbackState = audio.paused ? "paused" : "playing";
    }),
  );
  audio.addEventListener("loadedmetadata", () => {
    try {
      if (isFinite(audio.duration)) {
        navigator.mediaSession.setPositionState({ duration: audio.duration, playbackRate: 1, position: audio.currentTime });
      }
    } catch {
      /* 不支援 */
    }
  });
  audio.addEventListener("error", () => {
    // 檔案被刪除或格式不支援: 跳下一首
    if (current()) setTimeout(() => next(true), 800);
  });

  // ---- 鎖定畫面 / 耳機按鍵 ----
  function updateSession() {
    if (!("mediaSession" in navigator)) return;
    const t = current();
    if (!t) return;
    const art = coverUrl(t);
    navigator.mediaSession.metadata = new MediaMetadata({
      title: t.title,
      artist: t.artist || "",
      album: t.album || source,
      artwork: art ? [{ src: art, sizes: "512x512" }] : [],
    });
    setHandlers();
  }
  // 鎖定畫面的按鈕: 上一首 / 下一首
  // iOS 預設會顯示 -10 秒 / +10 秒並擋掉上一首/下一首; 把快退/快轉設成 null 才會換成上一首/下一首。
  // iOS 換歌後有時會忘記, 所以每次換歌都重新設定一次。
  function setHandlers() {
    if (!("mediaSession" in navigator)) return;
    const ms = navigator.mediaSession;
    const set = (action, fn) => {
      try {
        ms.setActionHandler(action, fn);
      } catch {
        /* 這個瀏覽器不支援這個動作 */
      }
    };
    set("play", () => audio.play());
    set("pause", () => audio.pause());
    set("previoustrack", prev);
    set("nexttrack", () => next());
    set("seekbackward", null);
    set("seekforward", null);
    set("seekto", (d) => (audio.currentTime = d.seekTime));
  }
  setHandlers();

  // ---- 指令 ----
  function quoteArg(a) {
    if (a !== "" && !/[\s"'\\]/.test(a)) return a;
    return `"${a.replace(/\\/g, "\\\\").replace(/"/g, '\\"')}"`;
  }
  async function command(line) {
    const r = await Apps.exec(line);
    await load().catch(() => {});
    return r;
  }

  /** 刪除歌曲; 正在播放的那首會先跳到下一首 */
  async function deleteTrack(file) {
    if (queue[index] === file) {
      if (queue.length > 1) next();
      else {
        audio.pause();
        audio.removeAttribute("src");
      }
    }
    return command(`/music delete ${quoteArg(file)}`);
  }

  return {
    audio,
    on: (f) => (listeners.add(f), () => listeners.delete(f)),
    get lib() {
      return lib;
    },
    track: (f) => byFile.get(f),
    current,
    get queue() {
      return queue;
    },
    get source() {
      return source;
    },
    get shuffle() {
      return shuffle;
    },
    get repeat() {
      return repeat;
    },
    setShuffle(v) {
      shuffle = v;
      save();
      emit();
    },
    cycleRepeat() {
      repeat = { off: "all", all: "one", one: "off" }[repeat];
      save();
      emit();
    },
    setVolume(v) {
      audio.volume = v;
      save();
    },
    load,
    playList,
    playAt,
    next,
    prev,
    toggle,
    command,
    deleteTrack,
    quoteArg,
    coverUrl,
  };
})();

// ==========================================================================
// 歌詞: srt / vtt / lrc → [{ t, text }]
// ==========================================================================
const Lyrics = (() => {
  const LANG_ORDER = ["zh-TW", "zh-Hant", "zh-HK", "zh", "zh-Hans", "zh-CN", "ja", "en", ""];

  function pickLang(list) {
    const pref = (() => {
      try {
        return localStorage.getItem("cng6.music.lang");
      } catch {
        return null;
      }
    })();
    const langs = list.map((l) => l.lang);
    if (pref && langs.includes(pref)) return pref;
    for (const l of LANG_ORDER) if (langs.includes(l)) return l;
    return langs[0];
  }

  const toSec = (h, m, s, ms) => +h * 3600 + +m * 60 + +s + +(ms || 0) / 10 ** String(ms || "").length;

  function parse(text) {
    const out = [];
    text = text.replace(/\r/g, "");
    if (/^\s*\[\d+:\d+/m.test(text) && !/-->/.test(text)) {
      // LRC: [mm:ss.xx] 文字
      for (const line of text.split("\n")) {
        const tags = [...line.matchAll(/\[(\d+):(\d+)(?:[.:](\d+))?\]/g)];
        const body = line.replace(/\[[^\]]*\]/g, "").trim();
        for (const m of tags) out.push({ t: toSec(0, m[1], m[2], m[3]), text: body });
      }
    } else {
      // SRT / VTT: 時間 --> 時間 + 文字
      for (const block of text.split(/\n\s*\n/)) {
        const m = block.match(/(?:(\d+):)?(\d+):(\d+)[,.](\d+)\s*-->/);
        if (!m) continue;
        const lines = block.split("\n");
        const i = lines.findIndex((l) => l.includes("-->"));
        const body = lines
          .slice(i + 1)
          .join("\n")
          .replace(/<[^>]+>/g, "")
          .replace(/\{[^}]+\}/g, "")
          .trim();
        if (body) out.push({ t: toSec(m[1] || 0, m[2], m[3], m[4]), text: body });
      }
    }
    // 相同文字連續出現(自動字幕常見)只留一次
    out.sort((a, b) => a.t - b.t);
    return out.filter((l, i) => l.text && (i === 0 || l.text !== out[i - 1].text));
  }

  const cache = new Map();
  async function load(file) {
    if (cache.has(file)) return cache.get(file);
    const r = await fetch(`/music/lyrics/${encodeURIComponent(file)}`);
    const lines = r.ok ? parse(await r.text()) : [];
    cache.set(file, lines);
    return lines;
  }

  /** 把歌詞畫進 el, 跟著播放進度捲動; 回傳停止函式 */
  function mount(el, { onLang } = {}) {
    let lines = [];
    let rows = [];
    let active = -1;
    let file = null;
    let userScroll = 0;
    el.classList.add("mu-lyrics");
    el.addEventListener("wheel", () => (userScroll = Date.now()), { passive: true });
    el.addEventListener("touchmove", () => (userScroll = Date.now()), { passive: true });

    async function setTrack() {
      const t = MusicPlayer.current();
      const key = t ? `${t.file}|${t.lyrics.map((l) => l.file).join()}` : null;
      if (key === file) return;
      file = key;
      el.replaceChildren();
      rows = [];
      active = -1;
      if (!t || !t.lyrics.length) {
        const e = document.createElement("div");
        e.className = "mu-lyrics-empty";
        e.textContent = t ? "這首歌沒有歌詞" : "還沒有播放";
        el.appendChild(e);
        onLang && onLang(t, null);
        return;
      }
      const lang = pickLang(t.lyrics);
      onLang && onLang(t, lang);
      const lf = t.lyrics.find((l) => l.lang === lang) || t.lyrics[0];
      lines = await load(lf.file);
      if (file !== key) return;
      rows = lines.map((l) => {
        const d = document.createElement("div");
        d.className = "mu-ly";
        d.textContent = l.text;
        d.addEventListener("click", () => (MusicPlayer.audio.currentTime = l.t));
        el.appendChild(d);
        return d;
      });
      if (!rows.length) {
        const e = document.createElement("div");
        e.className = "mu-lyrics-empty";
        e.textContent = "歌詞是空的";
        el.appendChild(e);
      }
      sync();
    }

    function sync() {
      const now = MusicPlayer.audio.currentTime + 0.25;
      let i = -1;
      for (let k = 0; k < lines.length && lines[k].t <= now; k++) i = k;
      if (i === active) return;
      rows[active]?.classList.remove("on");
      active = i;
      const row = rows[i];
      if (!row) return;
      row.classList.add("on");
      // 使用者手動捲動後 4 秒內不自動捲
      if (Date.now() - userScroll > 4000) {
        el.scrollTo({ top: row.offsetTop - el.clientHeight / 2 + row.offsetHeight / 2, behavior: "smooth" });
      }
    }

    const off = MusicPlayer.on(setTrack);
    const tick = () => sync();
    MusicPlayer.audio.addEventListener("timeupdate", tick);
    setTrack();
    return {
      refresh() {
        file = null;
        setTrack();
      },
      stop() {
        off();
        MusicPlayer.audio.removeEventListener("timeupdate", tick);
      },
    };
  }

  return { mount, pickLang, LANG_ORDER };
})();

// ==========================================================================
// 畫面
// ==========================================================================
const MusicApp = (() => {
  const ICON = {
    play: `<svg viewBox="0 0 24 24"><path d="M8 5.5v13l11-6.5z" fill="currentColor"/></svg>`,
    pause: `<svg viewBox="0 0 24 24"><path d="M7 5h3.5v14H7zM13.5 5H17v14h-3.5z" fill="currentColor"/></svg>`,
    prev: `<svg viewBox="0 0 24 24"><path d="M6 5h2v14H6zM20 5.5v13L9.5 12z" fill="currentColor"/></svg>`,
    next: `<svg viewBox="0 0 24 24"><path d="M16 5h2v14h-2zM4 5.5v13L14.5 12z" fill="currentColor"/></svg>`,
    shuffle: `<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><path d="M16 3h5v5M4 20L21 3M21 16v5h-5M15 15l6 6M4 4l5 5"/></svg>`,
    repeat: `<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><path d="M17 2l4 4-4 4"/><path d="M3 11V9a3 3 0 0 1 3-3h15M7 22l-4-4 4-4"/><path d="M21 13v2a3 3 0 0 1-3 3H3"/></svg>`,
    lyrics: `<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><path d="M21 12a8 8 0 0 1-11.6 7.1L4 20l1.1-4.4A8 8 0 1 1 21 12z"/><path d="M8 10h8M8 14h5"/></svg>`,
    trash: `<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><path d="M3 6h18M8 6V4h8v2M6 6l1 14h10l1-14"/></svg>`,
    more: `<svg viewBox="0 0 24 24"><circle cx="5" cy="12" r="1.8" fill="currentColor"/><circle cx="12" cy="12" r="1.8" fill="currentColor"/><circle cx="19" cy="12" r="1.8" fill="currentColor"/></svg>`,
    note: `<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><path d="M9 18V5l12-2v13"/><circle cx="6" cy="18" r="3"/><circle cx="18" cy="16" r="3"/></svg>`,
    volume: `<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><path d="M11 5L6 9H2v6h4l5 4z"/><path d="M15.5 8.5a5 5 0 0 1 0 7M19 5a10 10 0 0 1 0 14"/></svg>`,
    down: `<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.4" stroke-linecap="round" stroke-linejoin="round"><path d="M6 9l6 6 6-6"/></svg>`,
    download: `<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><path d="M12 3v12M7 10l5 5 5-5M5 21h14"/></svg>`,
    listAdd: `<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><path d="M3 6h12M3 12h12M3 18h7M18 14v7M14.5 17.5h7"/></svg>`,
  };

  function h(tag, cls, text) {
    const el = document.createElement(tag);
    if (cls) el.className = cls;
    if (text != null) el.textContent = text;
    return el;
  }
  function btn(icon, title, cls = "mu-ib") {
    const b = h("button", cls);
    b.innerHTML = ICON[icon];
    b.title = title;
    b.setAttribute("aria-label", title);
    return b;
  }
  const fmt = (s) => {
    if (!isFinite(s) || s < 0) return "0:00";
    s = Math.floor(s);
    const h_ = Math.floor(s / 3600);
    const m = Math.floor((s % 3600) / 60);
    const sec = String(s % 60).padStart(2, "0");
    return h_ ? `${h_}:${String(m).padStart(2, "0")}:${sec}` : `${m}:${sec}`;
  };

  function coverEl(t, cls) {
    const c = h("span", cls);
    const url = MusicPlayer.coverUrl(t);
    if (url) {
      const img = h("img");
      img.loading = "lazy";
      img.decoding = "async";
      img.alt = "";
      img.src = url;
      img.onerror = () => img.remove();
      c.appendChild(img);
    }
    c.insertAdjacentHTML("afterbegin", ICON.note);
    return c;
  }

  // ---- 提示 ----
  function toast(root, text, ok = true) {
    let t = root.querySelector(".mu-toast");
    if (!t) {
      t = h("div", "mu-toast");
      root.appendChild(t);
    }
    t.textContent = text;
    t.dataset.ok = ok;
    t.classList.add("show");
    clearTimeout(t._timer);
    t._timer = setTimeout(() => t.classList.remove("show"), 2600);
  }

  // ---- 選單(PC: 浮動; 手機: 底部選單) ----
  function popMenu(root, anchor, items, mobile) {
    root.querySelector(".mu-menu-wrap")?.remove();
    const wrap = h("div", `mu-menu-wrap ${mobile ? "sheet" : ""}`);
    const menu = h("div", "mu-menu");
    for (const it of items) {
      if (it === "-") {
        menu.appendChild(h("div", "mu-menu-sep"));
        continue;
      }
      if (it.header) {
        menu.appendChild(h("div", "mu-menu-head", it.header));
        continue;
      }
      const b = h("button", `mu-menu-item ${it.danger ? "danger" : ""}`, it.label);
      b.disabled = !!it.disabled;
      b.addEventListener("click", () => {
        wrap.remove();
        it.action();
      });
      menu.appendChild(b);
    }
    if (mobile) menu.appendChild(Object.assign(h("button", "mu-menu-item cancel", "取消"), { onclick: () => wrap.remove() }));
    wrap.appendChild(menu);
    wrap.addEventListener("pointerdown", (e) => e.target === wrap && wrap.remove());
    root.appendChild(wrap);
    if (!mobile) {
      const r = anchor.getBoundingClientRect();
      const rr = root.getBoundingClientRect();
      const mw = menu.offsetWidth;
      const mh = menu.offsetHeight;
      let x = r.right - rr.left - mw;
      let y = r.bottom - rr.top + 4;
      if (y + mh > rr.height - 8) y = r.top - rr.top - mh - 4;
      menu.style.left = `${Math.max(8, x)}px`;
      menu.style.top = `${Math.max(8, y)}px`;
    }
  }

  // ======================================================================
  function mount(el, { device = "pc" } = {}) {
    const mobile = device === "mobile";
    el.classList.add("mu-root", mobile ? "mu-mobile" : "mu-pc");
    el.innerHTML = `
      <aside class="mu-side">
        <div class="mu-search"><input type="search" placeholder="搜尋歌曲、歌手" spellcheck="false" enterkeyhint="search"></div>
        <nav class="mu-nav"></nav>
      </aside>
      <section class="mu-main">
        <header class="mu-head"></header>
        <div class="mu-body"></div>
      </section>
      <aside class="mu-lyrics-panel" hidden>
        <div class="mu-lyrics-head"><b>歌詞</b><select class="mu-lang" aria-label="歌詞語言"></select></div>
        <div class="mu-lyrics-box"></div>
      </aside>
      <footer class="mu-bar"></footer>`;
    const nav = el.querySelector(".mu-nav");
    const head = el.querySelector(".mu-head");
    const body = el.querySelector(".mu-body");
    const bar = el.querySelector(".mu-bar");
    const search = el.querySelector(".mu-search input");
    const lyPanel = el.querySelector(".mu-lyrics-panel");
    const langSel = el.querySelector(".mu-lang");
    let view = { kind: "all" }; // all | playlist(name) | downloads | playlists(手機)
    let closed = false;

    // ---------------------------------------------------------- 側欄
    function renderNav() {
      const lib = MusicPlayer.lib;
      nav.replaceChildren();
      const item = (label, count, v, icon) => {
        const b = h("button", `mu-nav-item ${sameView(v) ? "on" : ""}`);
        if (icon) b.insertAdjacentHTML("beforeend", ICON[icon]);
        b.append(h("span", "mu-nav-label", label));
        if (count != null) b.append(h("span", "mu-nav-count", String(count)));
        b.addEventListener("click", () => {
          view = v;
          search.value = "";
          render();
        });
        return b;
      };
      const running = lib.downloads.filter((d) => d.state === "running").length;
      nav.append(item("所有歌曲", lib.tracks.length, { kind: "all" }, "note"));
      nav.append(item("下載", running || null, { kind: "downloads" }, "download"));
      const ph = h("div", "mu-nav-head");
      ph.append(h("span", null, "播放清單"));
      const add = h("button", "mu-nav-add", "+");
      add.title = "新增播放清單";
      add.addEventListener("click", createPlaylist);
      ph.append(add);
      nav.append(ph);
      for (const p of lib.playlists) nav.append(item(p.name, p.files.length, { kind: "playlist", name: p.name }));
      if (!lib.playlists.length) nav.append(h("div", "mu-nav-empty", "按 + 建立播放清單"));
    }
    const sameView = (v) => v.kind === view.kind && v.name === view.name;

    async function createPlaylist({ stay = false } = {}) {
      const name = prompt("播放清單名稱");
      if (!name || !name.trim()) return null;
      const r = await MusicPlayer.command(`/music playlist create ${MusicPlayer.quoteArg(name.trim())}`);
      toast(el, r.text || "已建立", r.ok);
      if (!r.ok) return null;
      if (!stay) {
        view = { kind: "playlist", name: name.trim() };
        render();
      }
      return name.trim();
    }

    // ---------------------------------------------------------- 列表
    function filesOfView() {
      const lib = MusicPlayer.lib;
      if (view.kind === "playlist") {
        const p = lib.playlists.find((p) => p.name === view.name);
        return p ? p.files.filter((f) => MusicPlayer.track(f)) : [];
      }
      return lib.tracks.map((t) => t.file);
    }

    function render() {
      if (closed) return;
      renderNav();
      head.replaceChildren();
      body.replaceChildren();
      if (mobile) renderMobileTabs();
      if (view.kind === "downloads") return renderDownloads();
      if (view.kind === "playlists") return renderPlaylistIndex();
      const lib = MusicPlayer.lib;
      if (view.kind === "playlist" && !lib.playlists.some((p) => p.name === view.name)) view = { kind: "all" };

      const files = filesOfView();
      const q = search.value.trim().toLowerCase();
      const shown = files.filter((f) => {
        if (!q) return true;
        const t = MusicPlayer.track(f);
        return t.title.toLowerCase().includes(q) || t.artist.toLowerCase().includes(q) || f.toLowerCase().includes(q);
      });
      const title = view.kind === "playlist" ? view.name : "所有歌曲";

      // 標題列
      const ht = h("div", "mu-head-text");
      ht.append(h("h2", null, title), h("span", "mu-head-sub", `${files.length} 首${q ? ` · 找到 ${shown.length} 首` : ""}`));
      head.append(ht);
      const actions = h("div", "mu-head-actions");
      const playAll = h("button", "mu-btn primary", "播放");
      playAll.disabled = !shown.length;
      playAll.addEventListener("click", () => MusicPlayer.playList(shown, null, title));
      actions.append(playAll);
      if (view.kind === "playlist") {
        const more = btn("more", "播放清單選項");
        more.addEventListener("click", () =>
          popMenu(
            el,
            more,
            [
              { label: "重新命名…", action: renamePlaylist },
              { label: "刪除播放清單", danger: true, action: deletePlaylist },
            ],
            mobile,
          ),
        );
        actions.append(more);
      }
      head.append(actions);

      // 歌曲
      const list = h("div", "mu-list");
      const cur = MusicPlayer.current();
      shown.forEach((f, i) => {
        const t = MusicPlayer.track(f);
        const row = h("div", `mu-row ${cur && cur.file === f ? "playing" : ""}`);
        row.dataset.file = f;
        row.tabIndex = 0;
        row.append(coverEl(t, "mu-cover"));
        const txt = h("div", "mu-row-text");
        txt.append(h("div", "mu-row-title", t.title));
        const sub = h("div", "mu-row-sub");
        if (t.artist) sub.append(document.createTextNode(t.artist));
        if (t.lyrics.length) sub.append(h("span", "mu-tag", "歌詞"));
        txt.append(sub);
        row.append(txt, h("span", "mu-row-dur", fmt(t.duration)));
        const more = btn("more", "更多");
        more.addEventListener("click", (e) => {
          e.stopPropagation();
          trackMenu(t, more, i);
        });
        row.append(more);
        const play = () => {
          MusicPlayer.playList(shown, f, title);
          // 手機: 直接進入單曲播放頁
          if (mobile) el.classList.add("mu-full");
        };
        row.addEventListener("click", play);
        row.addEventListener("keydown", (e) => e.key === "Enter" && play());
        list.appendChild(row);
      });
      if (!shown.length) {
        list.appendChild(
          h(
            "div",
            "mu-empty",
            q ? `找不到「${search.value}」` : view.kind === "playlist" ? "這個播放清單是空的, 在歌曲的 ⋯ 選單選「加入播放清單」" : "music 資料夾裡還沒有歌曲",
          ),
        );
      }
      body.appendChild(list);
      // 保持正在播放的歌在可見範圍
      list.querySelector(".mu-row.playing")?.scrollIntoView({ block: "nearest" });
    }

    /** 加入播放清單的選單項目; skip: 不列出的清單(例如目前正在看的) */
    function playlistItems(t, skip) {
      const items = [{ header: `加入播放清單 · ${t.title}` }];
      for (const p of MusicPlayer.lib.playlists) {
        if (p.name === skip) continue;
        const has = p.files.includes(t.file);
        items.push({
          label: has ? `已在「${p.name}」` : `加入「${p.name}」`,
          disabled: has,
          action: () => addTo(p.name, t.file),
        });
      }
      items.push({
        label: "加入新的播放清單…",
        action: async () => {
          const name = await createPlaylist({ stay: true });
          if (name) addTo(name, t.file);
        },
      });
      return items;
    }

    function trackMenu(t, anchor) {
      const items = playlistItems(t, view.kind === "playlist" ? view.name : null);
      items.push("-");
      if (view.kind === "playlist") {
        items.push({ label: `從「${view.name}」移除`, action: () => removeFrom(view.name, t.file) });
      }
      items.push({ label: "刪除歌曲…", danger: true, action: () => confirmDelete(t) });
      popMenu(el, anchor, items, mobile);
    }

    async function addTo(name, file) {
      const r = await MusicPlayer.command(`/music playlist add ${MusicPlayer.quoteArg(name)} ${MusicPlayer.quoteArg(file)}`);
      toast(el, r.ok ? `已加入「${name}」` : r.text, r.ok);
    }
    async function removeFrom(name, file) {
      const r = await MusicPlayer.command(`/music playlist remove ${MusicPlayer.quoteArg(name)} ${MusicPlayer.quoteArg(file)}`);
      toast(el, r.ok ? `已從「${name}」移除` : r.text, r.ok);
    }
    async function renamePlaylist() {
      const name = prompt("新的名稱", view.name);
      if (!name || !name.trim() || name.trim() === view.name) return;
      const r = await MusicPlayer.command(`/music playlist rename ${MusicPlayer.quoteArg(view.name)} ${MusicPlayer.quoteArg(name.trim())}`);
      toast(el, r.text, r.ok);
      if (r.ok) {
        view = { kind: "playlist", name: name.trim() };
        render();
      }
    }
    async function deletePlaylist() {
      if (!confirm(`刪除播放清單「${view.name}」?\n歌曲不會被刪除。`)) return;
      const r = await MusicPlayer.command(`/music playlist delete ${MusicPlayer.quoteArg(view.name)}`);
      toast(el, r.text, r.ok);
      view = mobile ? { kind: "playlists" } : { kind: "all" };
      render();
    }
    async function confirmDelete(t) {
      const playing = MusicPlayer.current()?.file === t.file;
      const msg = `刪除「${t.title}」?\n\n檔案(含封面與歌詞)會從 music 資料夾刪除, 也會從所有播放清單移除。${playing ? "\n正在播放, 會先跳到下一首。" : ""}`;
      if (!confirm(msg)) return;
      const r = await MusicPlayer.deleteTrack(t.file);
      toast(el, r.ok ? `已刪除 ${t.title}` : r.text, r.ok);
    }

    // ---------------------------------------------------------- 下載
    function renderDownloads() {
      head.append(h("h2", null, "下載"));
      const form = h("form", "mu-dl-form");
      const inp = h("input", "mu-dl-input");
      inp.type = "url";
      inp.placeholder = "貼上 YouTube 網址";
      inp.enterKeyHint = "go";
      inp.required = true;
      const go = h("button", "mu-btn primary", "下載");
      go.type = "submit";
      form.append(inp, go);
      form.addEventListener("submit", async (e) => {
        e.preventDefault();
        const url = inp.value.trim();
        if (!url) return;
        go.disabled = true;
        const r = await MusicPlayer.command(`/music download ${MusicPlayer.quoteArg(url)}`);
        go.disabled = false;
        if (r.ok) inp.value = "";
        toast(el, r.ok ? "開始下載" : r.text, r.ok);
      });
      body.append(form, h("div", "mu-note", "下載最高音質的音訊(保留原始格式), 封面存成 .jpg, 字幕存成歌詞 .srt"));
      const list = h("div", "mu-list");
      const jobs = [...MusicPlayer.lib.downloads].reverse();
      for (const j of jobs) {
        const row = h("div", `mu-dl mu-dl-${j.state}`);
        const txt = h("div", "mu-row-text");
        txt.append(h("div", "mu-row-title", j.title));
        const sub = h("div", "mu-row-sub");
        if (j.state === "running") {
          sub.textContent = `${j.phase} ${Math.round(j.percent)}%${j.eta ? ` · 剩 ${j.eta}` : ""}`;
          const bar_ = h("div", "mu-progress");
          const fill = h("div", "mu-progress-fill");
          fill.style.width = `${j.percent}%`;
          bar_.appendChild(fill);
          txt.append(sub, bar_);
        } else {
          sub.textContent = { done: `完成 · ${j.file}`, failed: `失敗 · ${j.error}`, canceled: "已取消" }[j.state];
          txt.append(sub);
        }
        row.append(txt);
        if (j.state === "running") {
          const c = h("button", "mu-btn", "取消");
          c.addEventListener("click", async () => {
            const r = await MusicPlayer.command(`/music cancel ${j.id}`);
            toast(el, r.text, r.ok);
          });
          row.append(c);
        } else if (j.state === "done" && MusicPlayer.track(j.file)) {
          const p = h("button", "mu-btn", "播放");
          p.addEventListener("click", () => MusicPlayer.playList(MusicPlayer.lib.tracks.map((t) => t.file), j.file, "所有歌曲"));
          row.append(p);
        }
        list.append(row);
      }
      if (!jobs.length) list.append(h("div", "mu-empty", "還沒有下載紀錄"));
      body.append(list);
    }

    // ---------------------------------------------------------- 手機
    function renderMobileTabs() {
      let tabs = el.querySelector(".mu-mtabs");
      if (!tabs) {
        tabs = h("div", "mu-mtabs");
        el.querySelector(".mu-main").prepend(tabs);
      }
      tabs.replaceChildren();
      const kinds = [
        ["all", "歌曲"],
        ["playlists", "播放清單"],
        ["downloads", "下載"],
      ];
      for (const [k, label] of kinds) {
        const on = view.kind === k || (k === "playlists" && view.kind === "playlist");
        const b = h("button", on ? "on" : "", label);
        b.addEventListener("click", () => {
          view = { kind: k };
          render();
        });
        tabs.appendChild(b);
      }
    }

    function renderPlaylistIndex() {
      head.append(h("h2", null, "播放清單"));
      const add = h("button", "mu-btn", "新增");
      add.addEventListener("click", createPlaylist);
      head.append(add);
      const list = h("div", "mu-list");
      for (const p of MusicPlayer.lib.playlists) {
        const row = h("button", "mu-row mu-pl-row");
        const txt = h("div", "mu-row-text");
        txt.append(h("div", "mu-row-title", p.name), h("div", "mu-row-sub", `${p.files.length} 首`));
        row.append(txt, h("span", "mu-chev", "›"));
        row.addEventListener("click", () => {
          view = { kind: "playlist", name: p.name };
          render();
        });
        list.append(row);
      }
      if (!MusicPlayer.lib.playlists.length) list.append(h("div", "mu-empty", "還沒有播放清單"));
      body.append(list);
    }

    // ---------------------------------------------------------- 播放列
    bar.innerHTML = `
      <div class="mu-now"><span class="mu-now-cover"></span><div class="mu-now-text"><div class="mu-now-title">還沒有播放</div><div class="mu-now-sub"></div></div></div>
      <div class="mu-ctl">
        <div class="mu-buttons"></div>
        <div class="mu-seek"><span class="mu-time cur">0:00</span><input type="range" class="mu-range" min="0" max="1000" value="0" aria-label="進度"><span class="mu-time dur">0:00</span></div>
      </div>
      <div class="mu-extra"></div>`;
    const btns = bar.querySelector(".mu-buttons");
    const bShuffle = btn("shuffle", "隨機播放", "mu-ib toggle");
    const bPrev = btn("prev", "上一首");
    const bPlay = btn("play", "播放", "mu-ib play");
    const bNext = btn("next", "下一首");
    const bRepeat = btn("repeat", "重複", "mu-ib toggle");
    btns.append(bShuffle, bPrev, bPlay, bNext, bRepeat);
    bShuffle.addEventListener("click", () => MusicPlayer.setShuffle(!MusicPlayer.shuffle));
    bPrev.addEventListener("click", MusicPlayer.prev);
    bPlay.addEventListener("click", MusicPlayer.toggle);
    bNext.addEventListener("click", () => MusicPlayer.next());
    bRepeat.addEventListener("click", MusicPlayer.cycleRepeat);
    const extra = bar.querySelector(".mu-extra");
    const bLyrics = btn("lyrics", "歌詞", "mu-ib toggle");
    const bAdd = btn("listAdd", "加入播放清單");
    const bDelete = btn("trash", "刪除正在播放的歌", "mu-ib danger");
    extra.append(bLyrics, bAdd, bDelete);
    bAdd.addEventListener("click", () => {
      const t = MusicPlayer.current();
      if (t) popMenu(el, bAdd, playlistItems(t), mobile);
    });
    if (!mobile) {
      const vol = h("input", "mu-vol");
      vol.type = "range";
      vol.min = 0;
      vol.max = 1;
      vol.step = 0.01;
      vol.value = MusicPlayer.audio.volume;
      vol.setAttribute("aria-label", "音量");
      const paintVol = () => vol.style.setProperty("--p", `${vol.value * 100}%`);
      vol.addEventListener("input", () => {
        MusicPlayer.setVolume(+vol.value);
        paintVol();
      });
      paintVol();
      const vi = h("span", "mu-vol-icon");
      vi.innerHTML = ICON.volume;
      extra.append(vi, vol);
    }
    bDelete.addEventListener("click", () => {
      const t = MusicPlayer.current();
      if (t) confirmDelete(t);
    });
    // 手機: 點播放列展開全螢幕播放器
    if (mobile) {
      bar.querySelector(".mu-now").addEventListener("click", () => el.classList.toggle("mu-full"));
      const collapse = btn("down", "收起", "mu-ib mu-collapse");
      collapse.addEventListener("click", () => el.classList.remove("mu-full"));
      bar.prepend(collapse);
    }

    const range = bar.querySelector(".mu-range");
    const tCur = bar.querySelector(".mu-time.cur");
    const tDur = bar.querySelector(".mu-time.dur");
    let seeking = false;
    range.addEventListener("input", () => {
      seeking = true;
      const a = MusicPlayer.audio;
      tCur.textContent = fmt((range.value / 1000) * (a.duration || 0));
    });
    range.addEventListener("change", () => {
      const a = MusicPlayer.audio;
      if (a.duration) a.currentTime = (range.value / 1000) * a.duration;
      seeking = false;
    });
    function onTime() {
      const a = MusicPlayer.audio;
      if (!seeking) {
        range.value = a.duration ? Math.round((a.currentTime / a.duration) * 1000) : 0;
        tCur.textContent = fmt(a.currentTime);
      }
      tDur.textContent = fmt(a.duration || MusicPlayer.current()?.duration || 0);
      range.style.setProperty("--p", `${range.value / 10}%`);
    }
    MusicPlayer.audio.addEventListener("timeupdate", onTime);

    function renderBar() {
      const t = MusicPlayer.current();
      const a = MusicPlayer.audio;
      const now = bar.querySelector(".mu-now");
      // 只有換歌(或封面改變)時才換圖片, 否則每次更新都會重新載入, 畫面會跳一下
      const coverKey = t ? `${t.file}|${t.cover}` : "";
      if (now.dataset.cover !== coverKey) {
        now.dataset.cover = coverKey;
        now.querySelector(".mu-now-cover").replaceWith(coverEl(t, "mu-now-cover"));
      }
      now.querySelector(".mu-now-title").textContent = t ? t.title : "還沒有播放";
      now.querySelector(".mu-now-sub").textContent = t ? [t.artist, MusicPlayer.source].filter(Boolean).join(" · ") : "點一首歌開始播放";
      bPlay.innerHTML = a.paused ? ICON.play : ICON.pause;
      bPlay.title = a.paused ? "播放" : "暫停";
      bShuffle.classList.toggle("on", MusicPlayer.shuffle);
      bRepeat.classList.toggle("on", MusicPlayer.repeat !== "off");
      bRepeat.dataset.one = MusicPlayer.repeat === "one";
      bRepeat.title = { off: "不重複", all: "重複全部", one: "重複一首" }[MusicPlayer.repeat];
      bDelete.disabled = !t;
      bAdd.disabled = !t;
      bLyrics.classList.toggle("has", !!(t && t.lyrics.length));
      el.classList.toggle("mu-has-track", !!t);
      onTime();
    }

    // ---------------------------------------------------------- 歌詞
    const ly = Lyrics.mount(el.querySelector(".mu-lyrics-box"), {
      onLang(t, lang) {
        langSel.replaceChildren();
        langSel.hidden = !t || t.lyrics.length < 2;
        if (!t) return;
        for (const l of t.lyrics) {
          const o = h("option", null, l.lang || "預設");
          o.value = l.lang;
          o.selected = l.lang === lang;
          langSel.appendChild(o);
        }
      },
    });
    langSel.addEventListener("change", () => {
      try {
        localStorage.setItem("cng6.music.lang", langSel.value);
      } catch {
        /* 略過 */
      }
      ly.refresh();
    });
    let showLyrics = false;
    try {
      showLyrics = localStorage.getItem("cng6.music.showLyrics") === "1";
    } catch {
      /* 略過 */
    }
    function applyLyrics() {
      lyPanel.hidden = !showLyrics;
      bLyrics.classList.toggle("on", showLyrics);
      el.classList.toggle("mu-show-lyrics", showLyrics);
    }
    bLyrics.addEventListener("click", () => {
      showLyrics = !showLyrics;
      try {
        localStorage.setItem("cng6.music.showLyrics", showLyrics ? "1" : "0");
      } catch {
        /* 略過 */
      }
      applyLyrics();
    });
    applyLyrics();

    // ---------------------------------------------------------- 資料
    let lastKey = "";
    const off = MusicPlayer.on(() => {
      // 曲目或清單有變才重畫列表, 播放狀態只更新播放列與標示
      const lib = MusicPlayer.lib;
      const key = JSON.stringify([lib.version, lib.playlists, lib.downloads.map((d) => [d.id, d.state, Math.round(d.percent || 0)])]);
      if (key !== lastKey) {
        lastKey = key;
        render();
      } else {
        const cur = MusicPlayer.current();
        body.querySelectorAll(".mu-row[data-file]").forEach((r) => r.classList.toggle("playing", !!cur && r.dataset.file === cur.file));
      }
      renderBar();
    });
    search.addEventListener("input", render);

    let timer = null;
    async function poll() {
      try {
        await MusicPlayer.load();
      } catch {
        /* 下次再試 */
      }
      const running = MusicPlayer.lib.downloads.some((d) => d.state === "running");
      clearTimeout(timer);
      if (!closed) timer = setTimeout(poll, document.hidden ? 15000 : running ? 1500 : 8000);
    }
    poll();
    renderBar();

    return () => {
      closed = true;
      clearTimeout(timer);
      off();
      ly.stop();
      MusicPlayer.audio.removeEventListener("timeupdate", onTime);
    };
  }

  return { mount, ICON, coverEl, fmt };
})();

if (typeof Apps !== "undefined") Apps.registerView("music", MusicApp);

// ==========================================================================
// 關掉視窗後也看得到正在播放: PC 在選單列, 手機在分頁列上方
// ==========================================================================
(() => {
  const menubar = document.querySelector("#menubar");
  const mobileApp = document.querySelector("#app .m-tabs");
  if (!menubar && !mobileApp) return;
  const el = document.createElement("div");
  el.className = menubar ? "mu-mini-pc" : "mu-mini-m";
  el.hidden = true;
  el.innerHTML = `<button class="mu-mini-open"><span class="mu-mini-cover"></span><span class="mu-mini-title"></span></button><button class="mu-mini-play" aria-label="播放/暫停"></button><button class="mu-mini-next" aria-label="下一首">${MusicApp.ICON.next}</button>`;
  if (menubar) menubar.insertBefore(el, menubar.querySelector(".theme-wrap"));
  else mobileApp.parentElement.insertBefore(el, mobileApp);
  el.querySelector(".mu-mini-play").addEventListener("click", MusicPlayer.toggle);
  el.querySelector(".mu-mini-next").addEventListener("click", () => MusicPlayer.next());
  el.querySelector(".mu-mini-open").addEventListener("click", () => {
    if (window.Desktop) Desktop.open("music");
    else if (window.MobileNav) MobileNav.openApp("music");
  });
  function update() {
    const t = MusicPlayer.current();
    // 音樂畫面開著時不重複顯示(手機)
    const inView = !menubar && document.querySelector(".mu-root:not([hidden])")?.offsetParent;
    el.hidden = !t || !!inView;
    if (!t) return;
    const coverKey = `${t.file}|${t.cover}`;
    if (el.dataset.cover !== coverKey) {
      el.dataset.cover = coverKey;
      el.querySelector(".mu-mini-cover").replaceWith(MusicApp.coverEl(t, "mu-mini-cover"));
    }
    el.querySelector(".mu-mini-title").textContent = t.title;
    el.querySelector(".mu-mini-play").innerHTML = MusicPlayer.audio.paused ? MusicApp.ICON.play : MusicApp.ICON.pause;
    document.body.classList.toggle("mu-mini-on", !el.hidden);
  }
  MusicPlayer.on(update);
  // 手機切換頁面時重新判斷
  setInterval(update, 1000);
})();

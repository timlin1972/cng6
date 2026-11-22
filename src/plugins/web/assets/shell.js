// cng6 網頁 CLI: 這個瀏覽器分頁自己的 CLI 連線(輸入與輸出在同一個視窗)
//
// - 指令送到 /api/cli, 由 cng6 用和 TUI 相同的指令樹執行
// - 層次、指令歷史、訊息都是這個分頁自己的, 和 TUI 分開
"use strict";

const Shell = (() => {
  const HISTORY_KEY = "cng6.cli.history";
  const HISTORY_MAX = 500;
  const MESSAGE_MAX = 2000;

  // 每個分頁一個 session(重新整理會沿用, 新分頁是新的)
  let session = null;
  try {
    session = sessionStorage.getItem("cng6.cli.session");
  } catch {
    /* 無法使用 sessionStorage */
  }
  if (!session) {
    session = (crypto.randomUUID && crypto.randomUUID()) || `s${Date.now()}${Math.random().toString(16).slice(2)}`;
    try {
      sessionStorage.setItem("cng6.cli.session", session);
    } catch {
      /* 略過 */
    }
  }

  let history = [];
  try {
    history = JSON.parse(localStorage.getItem(HISTORY_KEY)) || [];
  } catch {
    history = [];
  }

  const state = {
    path: [],
    messages: [], // {t, level, text}
    busy: false,
  };
  const listeners = { message: new Set(), path: new Set(), clear: new Set() };
  const emit = (kind, arg) => listeners[kind].forEach((f) => f(arg));

  function on(kind, fn) {
    listeners[kind].add(fn);
    return () => listeners[kind].delete(fn);
  }

  function addMessages(lines) {
    const t = Date.now();
    const added = lines.map((l) => ({ t, level: l.level, text: l.text }));
    state.messages.push(...added);
    if (state.messages.length > MESSAGE_MAX) state.messages.splice(0, state.messages.length - MESSAGE_MAX);
    emit("message", added);
  }

  function clearMessages() {
    state.messages = [];
    emit("clear");
  }

  function addHistory(line) {
    if (!line.trim()) return;
    if (history[history.length - 1] !== line) history.push(line);
    if (history.length > HISTORY_MAX) history.splice(0, history.length - HISTORY_MAX);
    try {
      localStorage.setItem(HISTORY_KEY, JSON.stringify(history));
    } catch {
      /* 略過 */
    }
  }

  async function request(op, line) {
    let res;
    try {
      const r = await fetch("/api/cli", {
        method: "POST",
        headers: { "content-type": "application/json" },
        body: JSON.stringify({ session, op, line }),
      });
      if (!r.ok) throw new Error((await r.text()) || `HTTP ${r.status}`);
      res = await r.json();
    } catch (e) {
      addMessages([{ level: "error", text: `無法連線到 cng6: ${e.message}` }]);
      return null;
    }
    if (res.path.join(" ") !== state.path.join(" ")) {
      state.path = res.path;
      emit("path", state.path);
    }
    if (res.output.length) addMessages(res.output);
    return res;
  }

  return {
    on,
    get path() {
      return state.path;
    },
    get messages() {
      return state.messages;
    },
    get history() {
      return history;
    },
    prompt: () => `${state.path.join(" ")}>`,
    /** 執行指令; history: false 時不記到歷史(例如 Esc 回上一層) */
    exec(line, { history = true } = {}) {
      if (history) addHistory(line);
      return request("exec", line);
    },
    complete: (before) => request("complete", before),
    describe: (before) => request("describe", before),
    clearMessages,
    // 進入頁面時取得目前層次(空指令不會產生輸出)
    sync: () => request("exec", ""),
  };
})();

/** 文字結尾是否還在引號內(和 src/parse.rs 的 in_quote 相同規則) */
function inQuote(text) {
  let q = null;
  for (let i = 0; i < text.length; i++) {
    const c = text[i];
    if (q === null) {
      if (c === "'" || c === '"') q = c;
      else if (c === "\\") i++;
    } else if (q === '"' && c === "\\") {
      i++;
    } else if (c === q) {
      q = null;
    }
  }
  return q !== null;
}

// ------------------------------------------------- CLI 視窗(輸入 + 輸出)
const CliApp = {
  mount(el) {
    el.classList.add("term-root");
    el.innerHTML = `
      <div class="term-toolbar">
        <div class="term-filters" role="radiogroup" aria-label="篩選">
          <button data-f="all" class="on">全部</button>
          <button data-f="warn">警告</button>
          <button data-f="error">錯誤</button>
        </div>
        <span class="term-count"></span>
        <label class="term-opt"><input type="checkbox" class="term-time"> 時間</label>
        <button class="term-clear" title="清除輸出">清除</button>
      </div>
      <div class="term-list" role="log" aria-live="polite"></div>
      <button class="term-jump" hidden>↓ 最新訊息</button>
      <div class="term-input-row">
        <span class="term-prompt"></span>
        <input class="term-input" spellcheck="false" autocomplete="off" autocapitalize="off" aria-label="指令">
      </div>
      <div class="term-hint">Enter 執行 · Tab 補完 · ? 說明 · ↑↓ 歷史 · Esc 清除/上一層</div>`;
    const list = el.querySelector(".term-list");
    const count = el.querySelector(".term-count");
    const jump = el.querySelector(".term-jump");
    const timeBox = el.querySelector(".term-time");
    const promptEl = el.querySelector(".term-prompt");
    const input = el.querySelector(".term-input");

    // ---- 輸出 ----
    let filter = "all";
    try {
      timeBox.checked = localStorage.getItem("cng6.msg.time") === "1";
    } catch {
      /* 略過 */
    }
    list.classList.toggle("show-time", timeBox.checked);

    const ICON = { warn: "▲", error: "●" };
    const fmtTime = (t) => new Date(t).toLocaleTimeString("zh-TW", { hour12: false });
    const visible = (lv) => filter === "all" || lv === filter || (filter === "warn" && lv === "error");
    const nearBottom = () => list.scrollHeight - list.scrollTop - list.clientHeight < 40;

    function row(m) {
      const r = document.createElement("div");
      r.className = `term-line lv-${m.level}`;
      r.dataset.level = m.level;
      const time = document.createElement("span");
      time.className = "term-t";
      time.textContent = fmtTime(m.t);
      const icon = document.createElement("span");
      icon.className = "term-icon";
      icon.textContent = ICON[m.level] || "";
      const text = document.createElement("span");
      text.className = "term-text";
      text.textContent = m.text;
      r.append(time, icon, text);
      r.hidden = !visible(m.level);
      return r;
    }

    function updateCount() {
      const ms = Shell.messages;
      const w = ms.filter((m) => m.level === "warn").length;
      const e = ms.filter((m) => m.level === "error").length;
      count.textContent = ms.length ? `${ms.length} 行${w ? ` · ${w} 警告` : ""}${e ? ` · ${e} 錯誤` : ""}` : "";
    }

    function renderAll() {
      list.replaceChildren(...Shell.messages.map(row));
      if (!Shell.messages.length) {
        const empty = document.createElement("div");
        empty.className = "term-empty";
        empty.textContent = "輸入指令開始, 按 ? 查看可用的指令";
        list.appendChild(empty);
      }
      list.scrollTop = list.scrollHeight;
      updateCount();
    }

    const offMsg = Shell.on("message", (added) => {
      const stick = nearBottom();
      list.querySelector(".term-empty")?.remove();
      for (const m of added) list.appendChild(row(m));
      // 超過上限時移除最舊的
      while (list.children.length > Shell.messages.length) list.firstChild.remove();
      if (stick) list.scrollTop = list.scrollHeight;
      else jump.hidden = false;
      updateCount();
    });
    const offClear = Shell.on("clear", renderAll);

    list.addEventListener("scroll", () => {
      if (nearBottom()) jump.hidden = true;
    });
    jump.addEventListener("click", () => {
      list.scrollTop = list.scrollHeight;
      jump.hidden = true;
      input.focus();
    });
    el.querySelector(".term-clear").addEventListener("click", () => {
      Shell.clearMessages();
      input.focus();
    });
    timeBox.addEventListener("change", () => {
      list.classList.toggle("show-time", timeBox.checked);
      try {
        localStorage.setItem("cng6.msg.time", timeBox.checked ? "1" : "0");
      } catch {
        /* 略過 */
      }
    });
    el.querySelectorAll(".term-filters button").forEach((b) =>
      b.addEventListener("click", () => {
        filter = b.dataset.f;
        el.querySelectorAll(".term-filters button").forEach((x) => x.classList.toggle("on", x === b));
        for (const r of list.querySelectorAll(".term-line")) r.hidden = !visible(r.dataset.level);
        list.scrollTop = list.scrollHeight;
      }),
    );

    // ---- 輸入 ----
    let hIndex = null; // 瀏覽歷史時的位置
    let draft = "";
    const showPrompt = () => (promptEl.textContent = Shell.prompt());
    const offPath = Shell.on("path", showPrompt);
    showPrompt();
    Shell.sync();

    // 點輸出區把游標放回輸入列(選取文字複製時除外)
    list.addEventListener("mouseup", () => {
      if (!window.getSelection().toString()) input.focus();
    });
    setTimeout(() => input.focus(), 50);

    // 送出指令時跳到最新的輸出
    const toBottom = () => {
      list.scrollTop = list.scrollHeight;
      jump.hidden = true;
    };

    input.addEventListener("keydown", async (e) => {
      const pos = input.selectionStart;
      const before = input.value.slice(0, pos);
      const after = input.value.slice(pos);

      if (e.key === "Enter") {
        e.preventDefault();
        const line = input.value;
        input.value = "";
        hIndex = null;
        toBottom();
        await Shell.exec(line);
      } else if (e.key === "Tab") {
        e.preventDefault();
        toBottom();
        const res = await Shell.complete(before);
        if (res && res.replacement != null && input.value.slice(0, pos) === before) {
          input.value = res.replacement + after;
          input.setSelectionRange(res.replacement.length, res.replacement.length);
        }
      } else if (e.key === "?" && !inQuote(before)) {
        e.preventDefault();
        toBottom();
        await Shell.describe(before);
      } else if (e.key === "ArrowUp" || e.key === "ArrowDown") {
        e.preventDefault();
        const hist = Shell.history;
        if (!hist.length) return;
        if (hIndex === null) {
          if (e.key === "ArrowDown") return;
          draft = input.value;
          hIndex = hist.length;
        }
        hIndex += e.key === "ArrowUp" ? -1 : 1;
        if (hIndex < 0) hIndex = 0;
        if (hIndex >= hist.length) {
          hIndex = null;
          input.value = draft;
        } else {
          input.value = hist[hIndex];
        }
        const end = input.value.length;
        input.setSelectionRange(end, end);
      } else if (e.key === "Escape") {
        e.preventDefault();
        if (input.value) input.value = "";
        else if (Shell.path.length) await Shell.exec("..", { history: false });
        hIndex = null;
      } else if (e.ctrlKey && !e.altKey && !e.metaKey) {
        // 和 TUI 相同的編輯快速鍵; Ctrl-L 清除輸出
        const k = e.key.toLowerCase();
        if (k === "u") {
          e.preventDefault();
          input.value = after;
          input.setSelectionRange(0, 0);
        } else if (k === "k") {
          e.preventDefault();
          input.value = before;
        } else if (k === "w") {
          e.preventDefault();
          const cut = before.replace(/\S+\s*$/, "");
          input.value = cut + after;
          input.setSelectionRange(cut.length, cut.length);
        } else if (k === "a") {
          e.preventDefault();
          input.setSelectionRange(0, 0);
        } else if (k === "e") {
          e.preventDefault();
          input.setSelectionRange(input.value.length, input.value.length);
        } else if (k === "l") {
          e.preventDefault();
          Shell.clearMessages();
        }
      }
    });

    renderAll();
    return () => {
      offMsg();
      offClear();
      offPath();
    };
  },
};

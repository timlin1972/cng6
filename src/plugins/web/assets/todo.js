// cng6 待辦清單(PC 視窗與手機共用; 整個 cluster 共用一份)
//
// - 上面輸入新的待辦: 內容、優先順序(高 / 中 / 低)、到期日(選填)
// - 未完成依優先順序排, 點圓圈完成; 點內容展開編輯(內容、優先順序、到期日、備註、刪除)
// - 已完成的收在最下面, 可以一次清掉
// - 別台改的會同步過來: 定時重新讀取(編輯中不打斷)
"use strict";

const TodoApp = (() => {
  function h(tag, cls, text) {
    const el = document.createElement(tag);
    if (cls) el.className = cls;
    if (text != null) el.textContent = text;
    return el;
  }
  const PRI = { 1: "高", 2: "中", 3: "低" };

  function today(offset = 0) {
    const d = new Date();
    d.setDate(d.getDate() + offset);
    return `${d.getFullYear()}-${String(d.getMonth() + 1).padStart(2, "0")}-${String(d.getDate()).padStart(2, "0")}`;
  }

  function dueText(due) {
    if (!due) return "";
    if (due === today()) return "今天";
    if (due === today(1)) return "明天";
    if (due === today(-1)) return "昨天";
    const [y, m, d] = due.split("-").map(Number);
    return y === new Date().getFullYear() ? `${m}/${d}` : `${y}/${m}/${d}`;
  }

  function mount(el, { device = "pc" } = {}) {
    let items = [];
    let editing = null; // 展開編輯中的 id
    let showDone = false;
    let newPri = 2;
    let closed = false;

    const root = h("div", "td-root");
    root.dataset.device = device;
    root.innerHTML = `
      <form class="td-add">
        <input class="td-title" placeholder="新增待辦…" maxlength="500" enterkeyhint="done">
        <div class="td-add-row">
          <div class="td-pri" role="radiogroup" aria-label="優先順序"></div>
          <input class="td-due" type="date" title="到期日(選填)">
          <button class="td-add-btn" type="submit">新增</button>
        </div>
      </form>
      <div class="td-list"></div>
      <div class="td-toast" role="status"></div>`;
    el.replaceChildren(root);
    const $ = (s) => root.querySelector(s);
    const form = $(".td-add");
    const titleIn = $(".td-title");
    const dueIn = $(".td-due");
    const listEl = $(".td-list");

    async function req(url, opts) {
      const r = await fetch(url, opts);
      let d = null;
      try {
        d = await r.json();
      } catch {}
      if (!r.ok) throw new Error((d && d.error) || `${r.status} ${r.statusText}`);
      return d;
    }
    const send = (method, url, body) =>
      req(url, { method, headers: { "content-type": "application/json" }, body: body ? JSON.stringify(body) : undefined });

    let toastTimer = null;
    function toast(text, ok = true) {
      const t = $(".td-toast");
      t.textContent = text;
      t.dataset.ok = ok;
      t.classList.add("show");
      clearTimeout(toastTimer);
      toastTimer = setTimeout(() => t.classList.remove("show"), 2200);
    }

    // ---- 新增 ----
    function priButtons(container, value, onPick) {
      container.replaceChildren();
      for (const p of [1, 2, 3]) {
        const b = h("button", `td-pri-btn p${p}`, PRI[p]);
        b.type = "button";
        b.setAttribute("role", "radio");
        b.setAttribute("aria-checked", String(p === value));
        b.onclick = () => onPick(p);
        container.append(b);
      }
    }
    const renderNewPri = () =>
      priButtons($(".td-add .td-pri"), newPri, (p) => {
        newPri = p;
        renderNewPri();
      });
    renderNewPri();

    form.onsubmit = async (e) => {
      e.preventDefault();
      const title = titleIn.value.trim();
      if (!title) return titleIn.focus();
      try {
        const it = await send("POST", "/api/todo", { title, priority: newPri, due: dueIn.value || null });
        items.push(it);
        titleIn.value = "";
        dueIn.value = "";
        render();
        titleIn.focus();
      } catch (err) {
        toast(err.message, false);
      }
    };

    // ---- 清單 ----
    function sortItems() {
      items.sort((a, b) => {
        if (a.done !== b.done) return a.done ? 1 : -1;
        if (a.done) return (b.done_at || b.updated) - (a.done_at || a.updated);
        if (a.priority !== b.priority) return a.priority - b.priority;
        if (a.due !== b.due) return !a.due ? 1 : !b.due ? -1 : a.due < b.due ? -1 : 1;
        return a.created - b.created;
      });
    }

    function render() {
      sortItems();
      const open = items.filter((i) => !i.done);
      const done = items.filter((i) => i.done);
      listEl.replaceChildren();
      if (!items.length) {
        listEl.append(h("div", "td-empty", "沒有待辦事項 🎉"));
        return;
      }
      for (const it of open) listEl.append(row(it));
      if (!open.length) listEl.append(h("div", "td-empty small", "全部完成了"));
      if (done.length) {
        const head = h("div", "td-done-head");
        const tog = h("button", "td-link", `${showDone ? "▾" : "▸"} 已完成 (${done.length})`);
        tog.onclick = () => {
          showDone = !showDone;
          render();
        };
        const clear = h("button", "td-link danger", "清除已完成");
        clear.onclick = async () => {
          if (!confirm(`清掉 ${done.length} 項已完成的待辦?(所有機器都會清掉)`)) return;
          try {
            await send("POST", "/api/todo/clear");
            items = items.filter((i) => !i.done);
            render();
          } catch (err) {
            toast(err.message, false);
          }
        };
        head.append(tog, clear);
        listEl.append(head);
        if (showDone) for (const it of done) listEl.append(row(it));
      }
    }

    async function patch(it, p) {
      const old = { ...it };
      Object.assign(it, p);
      render();
      try {
        const n = await send("PUT", `/api/todo/${it.id}`, p);
        Object.assign(it, n);
        render();
      } catch (err) {
        Object.assign(it, old);
        render();
        toast(err.message, false);
      }
    }

    function row(it) {
      const r = h("div", `td-item p${it.priority}`);
      r.classList.toggle("done", it.done);
      r.classList.toggle("open", editing === it.id);
      const check = h("button", "td-check");
      check.title = it.done ? "標成未完成" : "完成";
      check.setAttribute("aria-label", check.title);
      check.onclick = () => patch(it, { done: !it.done });
      const main = h("div", "td-main");
      const title = h("div", "td-item-title", it.title);
      const meta = h("div", "td-meta");
      meta.append(h("span", `td-badge p${it.priority}`, PRI[it.priority]));
      if (it.due) {
        const overdue = !it.done && it.due < today();
        const d = h("span", `td-due-text${overdue ? " overdue" : it.due === today() ? " soon" : ""}`, `${overdue ? "已過期 · " : ""}${dueText(it.due)}`);
        meta.append(d);
      }
      if (it.notes) {
        const n = h("span", "td-note-mark");
        n.title = "有備註";
        n.innerHTML = `<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round"><path d="M14 3H6a2 2 0 0 0-2 2v14a2 2 0 0 0 2 2h12a2 2 0 0 0 2-2V9z"/><path d="M14 3v6h6M8 13h8M8 17h5"/></svg>`;
        meta.append(n);
      }
      main.append(title, meta);
      main.onclick = () => {
        editing = editing === it.id ? null : it.id;
        render();
      };
      r.append(check, main);
      if (editing === it.id) r.append(editor(it));
      return r;
    }

    function editor(it) {
      const box = h("div", "td-edit");
      const t = h("textarea", "td-edit-title");
      t.value = it.title;
      t.rows = 2;
      t.maxLength = 500;
      t.onchange = () => t.value.trim() && t.value.trim() !== it.title && patch(it, { title: t.value.trim() });
      const row1 = h("div", "td-edit-row");
      const pri = h("div", "td-pri");
      priButtons(pri, it.priority, (p) => patch(it, { priority: p }));
      const due = h("input", "td-due");
      due.type = "date";
      due.value = it.due || "";
      due.onchange = () => patch(it, { due: due.value || "" });
      row1.append(pri, due);
      const notes = h("textarea", "td-edit-notes");
      notes.placeholder = "備註";
      notes.rows = 3;
      notes.value = it.notes || "";
      notes.onchange = () => notes.value !== (it.notes || "") && patch(it, { notes: notes.value });
      const row2 = h("div", "td-edit-row");
      const info = h("span", "td-edit-info", it.by ? `最後修改: ${it.by}` : "");
      const del = h("button", "td-link danger", "刪除");
      del.onclick = async () => {
        try {
          await send("DELETE", `/api/todo/${it.id}`);
          items = items.filter((x) => x.id !== it.id);
          editing = null;
          render();
          toast(`已刪除: ${it.title}`);
        } catch (err) {
          toast(err.message, false);
        }
      };
      const doneBtn = h("button", "td-link", "收起");
      doneBtn.onclick = () => {
        editing = null;
        render();
      };
      row2.append(info, del, doneBtn);
      box.append(t, row1, notes, row2);
      box.onclick = (e) => e.stopPropagation();
      return box;
    }

    async function load() {
      try {
        const d = await req("/api/todo");
        if (closed) return;
        items = d.items;
        render();
      } catch (err) {
        listEl.replaceChildren(h("div", "td-empty", `讀取失敗: ${err.message}`));
      }
    }

    // 別台改的會同步過來: 定時更新(編輯或輸入中不打斷)
    const timer = setInterval(() => {
      if (document.hidden || editing || root.contains(document.activeElement) && document.activeElement.tagName !== "BUTTON") return;
      load();
    }, 10000);
    const onVisible = () => !document.hidden && !editing && load();
    document.addEventListener("visibilitychange", onVisible);

    load();
    if (device !== "mobile") titleIn.focus();

    return () => {
      closed = true;
      clearInterval(timer);
      document.removeEventListener("visibilitychange", onVisible);
    };
  }

  return { mount };
})();

if (typeof Apps !== "undefined") Apps.registerView("todo", TodoApp);

// cng6 Settings 視窗: 查看與修改系統設定(和 TUI 的 settings 指令共用同一份設定)
"use strict";

const SettingsApp = (() => {
  // 修改透過 /settings set 指令, 和 CLI 相同的檢查與寫檔; 用獨立的 session, 不影響網頁 CLI 的層次
  const SESSION = `settings-${Math.random().toString(36).slice(2, 10)}`;
  const POLL_MS = 3000;

  const GROUPS = {
    appearance: { name: "外觀", icon: "appearance" },
    wallpaper: { name: "桌布", icon: "wallpaper" },
    cli: { name: "CLI", icon: "cli" },
    message: { name: "Message", icon: "message" },
    system: { name: "System", icon: "system" },
    web: { name: "Web", icon: "web" },
    panel: { name: "Panel 位置", icon: "panel" },
  };
  // 外觀是這個瀏覽器自己的偏好, 不在 settings.conf 裡
  const LOCAL = "appearance";
  const ORDER = [LOCAL, "wallpaper", "cli", "message", "system", "web", "panel"];
  const ICONS = {
    wallpaper: `<rect x="3" y="5" width="18" height="14" rx="2"/><circle cx="9" cy="10" r="1.6"/><path d="M21 16l-5-5-8 8"/>`,
    appearance: `<circle cx="12" cy="12" r="8"/><path d="M12 4a8 8 0 0 1 0 16z" fill="#fff"/>`,
    cli: `<path d="M5 7l5 5-5 5"/><path d="M12 18h7"/>`,
    message: `<path d="M21 12a8 8 0 0 1-11.6 7.1L4 20l1.1-4.4A8 8 0 1 1 21 12z"/>`,
    system: `<rect x="6" y="6" width="12" height="12" rx="2"/><path d="M9 2v4M15 2v4M9 18v4M15 18v4M2 9h4M2 15h4M18 9h4M18 15h4"/>`,
    web: `<circle cx="12" cy="12" r="9"/><path d="M3 12h18M12 3a14 14 0 0 1 0 18M12 3a14 14 0 0 0 0 18"/>`,
    panel: `<rect x="3" y="4" width="18" height="16" rx="2"/><path d="M12 4v16M3 12h9"/>`,
    other: `<circle cx="12" cy="12" r="3"/><path d="M12 2v3M12 19v3M2 12h3M19 12h3"/>`,
  };
  const svg = (name) =>
    `<svg viewBox="0 0 24 24" fill="none" stroke="#fff" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">${ICONS[name] || ICONS.other}</svg>`;

  const groupOf = (key) => key.split(".")[0];

  // 和 src/parse.rs 的 quote_arg 相同: 需要時加上引號
  function quoteArg(a) {
    if (a !== "" && !/[\s"'\\]/.test(a)) return a;
    return `"${a.replace(/\\/g, "\\\\").replace(/"/g, '\\"')}"`;
  }

  async function api(op, line) {
    const r = await fetch("/api/cli", {
      method: "POST",
      headers: { "content-type": "application/json" },
      body: JSON.stringify({ session: SESSION, op, line }),
    });
    if (!r.ok) throw new Error((await r.text()) || `HTTP ${r.status}`);
    return r.json();
  }

  function h(tag, cls, text) {
    const el = document.createElement(tag);
    if (cls) el.className = cls;
    if (text != null) el.textContent = text;
    return el;
  }

  // 設定的控制項種類: 依預設值判斷
  function kindOf(row) {
    if (row.key.endsWith(".rect")) return "rect";
    if (row.default === "true" || row.default === "false") return "bool";
    if (/^\d+(\.\d+)?$/.test(row.default)) return "number";
    return "text";
  }

  // 說明裡的 "(true/false)"、"1~65535" 等提示不必重複顯示在標題
  function splitDesc(desc) {
    const m = desc.match(/^(.*?)(?:[,，]\s*|\s+)?\(([^()]*)\)\s*$/);
    return m ? [m[1], m[2]] : [desc, ""];
  }

  function mount(el) {
    el.classList.add("set-root");
    el.innerHTML = `
      <aside class="set-side">
        <div class="set-search"><svg viewBox="0 0 24 24" aria-hidden="true"><circle cx="11" cy="11" r="7"/><path d="M20 20l-4-4"/></svg><input placeholder="搜尋設定" spellcheck="false"></div>
        <nav class="set-groups"></nav>
        <div class="set-file">設定檔 settings.conf</div>
      </aside>
      <main class="set-main"><div class="set-pane"></div></main>
      <div class="set-toast" role="status"></div>`;
    const nav = el.querySelector(".set-groups");
    const pane = el.querySelector(".set-pane");
    const search = el.querySelector(".set-search input");
    const toast = el.querySelector(".set-toast");

    let rows = [];
    let current = LOCAL;
    let editing = false; // 輸入中不要被定時更新蓋掉
    let toastTimer = null;
    let closed = false;

    function showToast(text, kind = "ok") {
      toast.textContent = text;
      toast.dataset.kind = kind;
      toast.classList.add("show");
      clearTimeout(toastTimer);
      toastTimer = setTimeout(() => toast.classList.remove("show"), 2200);
    }

    async function refresh() {
      try {
        const res = await api("settings", "");
        const changed = JSON.stringify(res.settings) !== JSON.stringify(rows);
        rows = res.settings;
        if (changed && !editing) render();
      } catch (e) {
        if (!rows.length) {
          pane.replaceChildren(h("div", "set-empty", `無法讀取設定: ${e.message}`));
        }
      }
    }

    /** 執行 settings 指令, 回傳錯誤訊息(成功時為 null) */
    async function run(line) {
      try {
        const res = await api("exec", line);
        const err = res.output.find((o) => o.level === "error" || o.level === "warn");
        await refresh();
        return err ? err.text.replace(/^settings:\s*/, "") : null;
      } catch (e) {
        return e.message;
      }
    }

    async function apply(row, value, errEl) {
      if (value === row.value) return;
      const err = await run(`/settings set ${row.key} ${quoteArg(value)}`);
      if (err) {
        errEl.textContent = err;
        errEl.hidden = false;
        render(); // 還原成目前的值
        const again = pane.querySelector(`[data-key="${CSS.escape(row.key)}"] .set-err`);
        if (again) {
          again.textContent = err;
          again.hidden = false;
        }
      } else {
        showToast(`已儲存 ${row.key}`);
      }
    }

    async function reset(row) {
      const err = await run(`/settings reset ${row.key}`);
      showToast(err ? err : `${row.key} 已恢復預設`, err ? "error" : "ok");
    }

    // ---- 控制項 ----
    function control(row, errEl) {
      const kind = kindOf(row);
      if (kind === "bool") {
        const label = h("label", "set-switch");
        const box = h("input");
        box.type = "checkbox";
        box.checked = row.value === "true";
        box.setAttribute("aria-label", row.key);
        box.addEventListener("change", () => apply(row, box.checked ? "true" : "false", errEl));
        label.append(box, h("span", "track"));
        return label;
      }
      if (kind === "rect") return rectControl(row, errEl);
      const input = h("input", `set-input ${kind === "number" ? "num" : ""}`);
      input.value = row.value;
      input.spellcheck = false;
      input.inputMode = kind === "number" ? "decimal" : "text";
      input.setAttribute("aria-label", row.key);
      wireInput(input, () => apply(row, input.value.trim(), errEl), row.value);
      return input;
    }

    // Enter 或離開輸入框時套用, Esc 還原
    function wireInput(input, commit, original) {
      input.addEventListener("focus", () => (editing = true));
      input.addEventListener("blur", async () => {
        editing = false;
        await commit();
      });
      input.addEventListener("keydown", (e) => {
        if (e.key === "Enter") input.blur();
        else if (e.key === "Escape") {
          input.value = original;
          input.blur();
        }
      });
    }

    function rectControl(row, errEl) {
      const wrap = h("div", "set-rect");
      const preview = h("div", "set-rect-preview");
      const fill = h("div", "set-rect-fill");
      preview.appendChild(fill);
      const inputs = h("div", "set-rect-inputs");
      const names = ["x1", "y1", "x2", "y2"];
      const vals = row.value.split(/\s+/).map(Number);
      const fields = names.map((n, i) => {
        const f = h("label", "set-rect-field");
        const input = h("input", "set-input num");
        input.value = vals[i] ?? "";
        input.inputMode = "numeric";
        input.setAttribute("aria-label", `${row.key} ${n}`);
        f.append(h("span", null, n), input);
        inputs.appendChild(f);
        return input;
      });
      const draw = () => {
        const [x1, y1, x2, y2] = fields.map((f) => Math.max(0, Math.min(100, Number(f.value) || 0)));
        fill.style.left = `${x1}%`;
        fill.style.top = `${y1}%`;
        fill.style.width = `${Math.max(0, x2 - x1)}%`;
        fill.style.height = `${Math.max(0, y2 - y1)}%`;
        fill.classList.toggle("bad", !(x1 < x2 && y1 < y2));
      };
      draw();
      fields.forEach((input) => {
        input.addEventListener("input", draw);
        wireInput(
          input,
          async () => {
            // 四個欄位都離開後才套用(在欄位之間移動時不送出)
            await new Promise((r) => setTimeout(r, 0));
            if (fields.includes(document.activeElement)) return;
            await apply(row, fields.map((f) => f.value.trim()).join(" "), errEl);
          },
          input.value,
        );
      });
      wrap.append(preview, inputs);
      return wrap;
    }

    // ---- 版面 ----
    function groupsPresent() {
      const keys = [LOCAL, ...new Set(rows.map((r) => groupOf(r.key)))];
      return keys.sort((a, b) => {
        const ia = ORDER.indexOf(a);
        const ib = ORDER.indexOf(b);
        return (ia < 0 ? 99 : ia) - (ib < 0 ? 99 : ib) || a.localeCompare(b);
      });
    }

    function renderNav() {
      nav.replaceChildren();
      for (const g of groupsPresent()) {
        const meta = GROUPS[g] || { name: g, icon: "other" };
        const btn = h("button", `set-nav ${g === current && !search.value ? "on" : ""}`);
        const icon = h("span", `set-icon i-${meta.icon}`);
        icon.innerHTML = svg(meta.icon);
        const count = rows.filter((r) => groupOf(r.key) === g && r.value !== r.default).length;
        btn.append(icon, h("span", "set-nav-name", meta.name));
        if (count) btn.appendChild(h("span", "set-nav-count", String(count)));
        btn.addEventListener("click", () => {
          current = g;
          search.value = "";
          render();
        });
        nav.appendChild(btn);
      }
    }

    function rowEl(row) {
      const r = h("div", "set-row");
      r.dataset.key = row.key;
      const modified = row.value !== row.default;
      const [title, hint] = splitDesc(row.desc);
      const text = h("div", "set-text");
      const t = h("div", "set-title", title);
      if (modified) t.appendChild(h("span", "set-dot"));
      const sub = h("div", "set-sub");
      sub.append(h("code", null, row.key));
      if (hint) sub.append(h("span", "set-hint", hint));
      if (row.key.startsWith("web.") && /restart/.test(row.desc)) sub.append(h("span", "set-tag", "需 restart"));
      const err = h("div", "set-err");
      err.hidden = true;
      text.append(t, sub, err);

      const right = h("div", "set-ctl");
      if (modified) {
        const rb = h("button", "set-reset");
        rb.title = `恢復預設 (${row.default})`;
        rb.setAttribute("aria-label", rb.title);
        rb.innerHTML = `<svg viewBox="0 0 24 24"><path d="M4 12a8 8 0 1 0 2.3-5.6"/><path d="M4 4v4h4"/></svg>`;
        rb.addEventListener("click", () => reset(row));
        right.appendChild(rb);
      }
      right.appendChild(control(row, err));
      r.append(text, right);
      return r;
    }

    function section(title, list, extra) {
      const s = h("section", "set-section");
      const head = h("div", "set-section-head");
      head.appendChild(h("h2", null, title));
      if (extra) head.appendChild(extra);
      s.appendChild(head);
      const card = h("div", "set-card");
      for (const row of list) card.appendChild(rowEl(row));
      s.appendChild(card);
      return s;
    }

    function render() {
      renderNav();
      pane.replaceChildren();
      const q = search.value.trim().toLowerCase();
      if (q) {
        const hits = rows.filter((r) => r.key.toLowerCase().includes(q) || r.desc.toLowerCase().includes(q));
        pane.appendChild(h("h1", null, "搜尋結果"));
        if (!hits.length) pane.appendChild(h("div", "set-empty", `找不到「${search.value}」`));
        for (const g of groupsPresent()) {
          const list = hits.filter((r) => groupOf(r.key) === g);
          if (list.length) pane.appendChild(section((GROUPS[g] || { name: g }).name, list));
        }
        return;
      }
      if (!groupsPresent().includes(current)) current = groupsPresent()[0];
      if (current === LOCAL) return renderAppearance();
      const meta = GROUPS[current] || { name: current, icon: "other" };
      const head = h("header", "set-head");
      const icon = h("span", `set-icon big i-${meta.icon}`);
      icon.innerHTML = svg(meta.icon);
      head.append(icon, h("h1", null, meta.name));
      pane.appendChild(head);

      let extra = null;
      if (current === "web") {
        extra = h("button", "set-action", "重新啟動 web");
        extra.title = "套用 port / https / 憑證設定; 網頁會中斷約一秒後自動恢復";
        extra.addEventListener("click", async () => {
          extra.disabled = true;
          extra.textContent = "重新啟動中…";
          try {
            await api("exec", "/web restart");
          } catch {
            /* 連線會在重新啟動時中斷 */
          }
          setTimeout(() => {
            extra.disabled = false;
            extra.textContent = "重新啟動 web";
            showToast("web 已重新啟動");
          }, 1500);
        });
      }
      let list = rows.filter((r) => groupOf(r.key) === current);
      if (current === "wallpaper") {
        pane.appendChild(wallpaperGallery());
        // 目前桌布用上面的縮圖選, 不另外列出
        list = list.filter((r) => r.key !== "wallpaper.current");
      }
      pane.appendChild(section("設定", list, extra));
      const modified = list.filter((r) => r.value !== r.default);
      if (modified.length) {
        const all = h("button", "set-link", `全部恢復預設 (${modified.length})`);
        all.addEventListener("click", async () => {
          for (const r of modified) await run(`/settings reset ${r.key}`);
          showToast(`${meta.name} 已全部恢復預設`);
        });
        pane.appendChild(all);
      }
    }

    // ---- 桌布 ----
    const IMAGE_RE = /\.(jpe?g|png|webp|gif|avif)$/i;
    let uploading = 0;

    async function setWallpaper(name) {
      const err = await run(`/wallpaper set ${quoteArg(name)}`);
      if (err) showToast(err, "error");
      else showToast(name === "default" ? "已換回預設桌布" : `桌布已換成 ${name}`);
      if (window.Desktop) Desktop.refreshWallpaper();
    }

    async function uploadFiles(files) {
      const list = [...files];
      const bad = list.filter((f) => !IMAGE_RE.test(f.name));
      if (bad.length) showToast(`只接受 jpg / png / webp / gif / avif: ${bad.map((f) => f.name).join(", ")}`, "error");
      const good = list.filter((f) => IMAGE_RE.test(f.name));
      if (!good.length) return;
      uploading += good.length;
      render();
      let ok = 0;
      let lastErr = null;
      for (const f of good) {
        try {
          const r = await fetch(`/api/wallpaper/${encodeURIComponent(f.name)}`, { method: "PUT", body: f });
          if (!r.ok) throw new Error((await r.text()) || `HTTP ${r.status}`);
          ok++;
        } catch (e) {
          lastErr = `${f.name}: ${e.message}`;
        }
        uploading--;
      }
      render();
      if (lastErr) showToast(lastErr, "error");
      else showToast(`已上傳 ${ok} 張桌布`);
    }

    async function deleteWallpaper(name, showing) {
      if (!confirm(`刪除桌布「${name}」?\n檔案會從資料夾移除。`)) return;
      try {
        const r = await fetch(`/api/wallpaper/${encodeURIComponent(name)}`, { method: "DELETE" });
        if (!r.ok) throw new Error((await r.text()) || `HTTP ${r.status}`);
      } catch (e) {
        return showToast(e.message, "error");
      }
      // 刪掉的是目前的桌布: 換回預設
      if (name === showing) await run("/wallpaper set default");
      if (window.Desktop) Desktop.refreshWallpaper();
      showToast(`已刪除 ${name}`);
      render();
    }

    function wallpaperGallery() {
      const s = h("section", "set-section wp-section");
      const head = h("div", "set-section-head");
      head.appendChild(h("h2", null, "桌布"));
      const pick = h("label", "set-action wp-upload");
      pick.textContent = "上傳圖片…";
      const file = h("input");
      file.type = "file";
      file.accept = "image/jpeg,image/png,image/webp,image/gif,image/avif";
      file.multiple = true;
      file.hidden = true;
      file.addEventListener("change", () => {
        uploadFiles(file.files);
        file.value = "";
      });
      pick.appendChild(file);
      head.appendChild(pick);
      s.appendChild(head);

      const card = h("div", "set-card wp-card");
      const grid = h("div", "wp-grid");
      card.appendChild(grid);
      const hint = h("div", "wp-hint", "把圖片拖到這裡也可以上傳");
      card.appendChild(hint);
      s.appendChild(card);

      // 拖放上傳
      card.addEventListener("dragover", (e) => {
        e.preventDefault();
        card.classList.add("drop");
      });
      card.addEventListener("dragleave", (e) => {
        if (!card.contains(e.relatedTarget)) card.classList.remove("drop");
      });
      card.addEventListener("drop", (e) => {
        e.preventDefault();
        card.classList.remove("drop");
        if (e.dataTransfer.files.length) uploadFiles(e.dataTransfer.files);
      });

      fetch("/api/wallpaper", { cache: "no-store" })
        .then((r) => r.json())
        .then((info) => {
          const tile = (name, label, imgUrl) => {
            const b = h("div", `wp-tile ${info.showing === name ? "on" : ""}`);
            b.tabIndex = 0;
            b.setAttribute("role", "radio");
            b.setAttribute("aria-checked", String(info.showing === name));
            b.title = label;
            const thumb = h("span", `wp-thumb ${imgUrl ? "" : "wp-default"}`);
            if (imgUrl) {
              const img = h("img");
              img.loading = "lazy";
              img.decoding = "async";
              img.alt = "";
              img.src = imgUrl;
              thumb.appendChild(img);
            }
            b.append(thumb, h("span", "wp-name", label));
            const choose = () => info.showing !== name && setWallpaper(name);
            b.addEventListener("click", choose);
            b.addEventListener("keydown", (e) => {
              if (e.key === "Enter" || e.key === " ") {
                e.preventDefault();
                choose();
              }
            });
            if (imgUrl) {
              const del = h("button", "wp-del", "×");
              del.title = `刪除 ${name}`;
              del.setAttribute("aria-label", del.title);
              del.addEventListener("click", (e) => {
                e.stopPropagation();
                deleteWallpaper(name, info.showing);
              });
              b.appendChild(del);
            }
            return b;
          };
          grid.appendChild(tile("default", "預設", null));
          for (const it of info.list) grid.appendChild(tile(it.name, it.name, it.url));
          for (let i = 0; i < uploading; i++) {
            const u = h("div", "wp-tile uploading");
            u.append(h("span", "wp-thumb", "上傳中…"), h("span", "wp-name", " "));
            grid.appendChild(u);
          }
          if (!info.list.length && !uploading) hint.textContent = `資料夾 ${info.dir}/ 裡還沒有圖片, 按「上傳圖片…」或把圖片拖到這裡`;
        })
        .catch((e) => grid.replaceChildren(h("div", "set-empty", `無法讀取桌布: ${e.message}`)));
      return s;
    }

    // ---- 外觀 ----
    const THEMES = [
      { id: "system", name: "系統", note: "跟著作業系統" },
      { id: "light", name: "淺色" },
      { id: "dark", name: "深色" },
    ];

    function renderAppearance() {
      const meta = GROUPS[LOCAL];
      const head = h("header", "set-head");
      const icon = h("span", `set-icon big i-${meta.icon}`);
      icon.innerHTML = svg(meta.icon);
      head.append(icon, h("h1", null, meta.name));
      pane.appendChild(head);

      const s = h("section", "set-section");
      const sh = h("div", "set-section-head");
      sh.appendChild(h("h2", null, "外觀"));
      s.appendChild(sh);
      const card = h("div", "set-card set-theme-card");
      const now = window.Desktop ? Desktop.getTheme() : "system";
      const opts = h("div", "set-themes");
      opts.setAttribute("role", "radiogroup");
      opts.setAttribute("aria-label", "外觀");
      for (const t of THEMES) {
        const b = h("button", `set-theme ${t.id === now ? "on" : ""}`);
        b.setAttribute("role", "radio");
        b.setAttribute("aria-checked", String(t.id === now));
        const thumb = h("span", `set-thumb t-${t.id}`);
        // 縮圖: 桌面 + 一個小視窗
        thumb.innerHTML = `<span class="mini-win"><span class="mini-bar"></span><span class="mini-line"></span><span class="mini-line short"></span></span>`;
        b.append(thumb, h("span", "set-theme-name", t.name));
        b.addEventListener("click", () => window.Desktop && Desktop.setTheme(t.id));
        opts.appendChild(b);
      }
      card.appendChild(opts);
      const note = h("div", "set-theme-note", "只套用在這個瀏覽器, 不會寫入 settings.conf。「系統」會跟著作業系統的淺色/深色設定自動切換。");
      card.appendChild(note);
      s.appendChild(card);
      pane.appendChild(s);
    }

    // 從選單列切換外觀時, 同步更新
    const onTheme = () => {
      if (current === LOCAL && !search.value) render();
    };
    window.addEventListener("cng6-theme", onTheme);

    search.addEventListener("input", render);
    render();
    refresh();
    const timer = setInterval(() => {
      if (!closed && !document.hidden) refresh();
    }, POLL_MS);

    return () => {
      closed = true;
      clearInterval(timer);
      window.removeEventListener("cng6-theme", onTheme);
    };
  }

  return { mount };
})();

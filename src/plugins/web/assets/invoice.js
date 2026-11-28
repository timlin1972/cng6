// cng6 電子發票(PC 視窗與手機共用)
//
// - 依期別分組, 每張發票顯示: 未開獎 / 中獎 / 沒中獎 / 已過期
// - 點寄件者或主旨打開那封信(郵件 App); 沒中獎、已過期的可以把信刪掉(移到 Gmail 垃圾桶)
// - 每期可以一次刪掉全部沒中獎的
// - 找不到號碼的信另外列出: 打開來看, 手動加入號碼, 或標成「不是發票」
"use strict";

const InvoiceApp = (() => {
  function h(tag, cls, text) {
    const el = document.createElement(tag);
    if (cls) el.className = cls;
    if (text != null) el.textContent = text;
    return el;
  }
  const money = (n) => Number(n || 0).toLocaleString("en-US");
  const fmtNo = (n) => `${n.slice(0, 2)}-${n.slice(2)}`;
  const md = (s) => {
    if (!s) return "";
    const [y, m, d] = s.split("-").map(Number);
    return y === new Date().getFullYear() ? `${m}/${d}` : `${y}/${m}/${d}`;
  };
  function periodLabel(key) {
    const roc = key.slice(0, -2);
    const m = Number(key.slice(-2));
    return `${roc}年 ${String(m).padStart(2, "0")}-${String(m + 1).padStart(2, "0")}月`;
  }
  const STATUS = {
    won: "中獎",
    lost: "沒中獎",
    pending: "未開獎",
    waiting: "等號碼",
    expired: "已過期",
  };
  const FILTERS = [
    ["all", "全部"],
    ["won", "中獎"],
    ["pending", "未開獎"],
    ["lost", "沒中獎"],
    ["expired", "已過期"],
    ["unknown", "找不到號碼"],
  ];

  function openMail(account, msg) {
    if (!msg) return;
    window.MailOpenRequest = { account, id: msg };
    window.dispatchEvent(new CustomEvent("cng6-mail-open", { detail: window.MailOpenRequest }));
    if (window.DeskNav) window.DeskNav.openApp("mail");
    else if (window.MobileNav) window.MobileNav.openApp("mail");
  }

  function mount(el, { device = "pc" } = {}) {
    let data = null;
    let filter = "all";
    let closed = false;
    let busy = false;
    // 手動開始的掃描: 掃完要提示(記下開始前的狀態, 比較差多少)
    let watching = null;

    const root = h("div", "iv-root");
    root.dataset.device = device;
    root.innerHTML = `
      <div class="iv-top">
        <div class="iv-sum"></div>
        <div class="iv-actions">
          <button class="iv-btn iv-scan" title="檢查新的信">檢查新信</button>
          <button class="iv-btn iv-full" title="全部重新掃描(找到的發票會保留)">全部重新掃描</button>
          <button class="iv-btn primary iv-add">＋ 手動加入</button>
        </div>
      </div>
      <div class="iv-filters"></div>
      <div class="iv-list"></div>
      <div class="iv-overlay" hidden></div>
      <div class="iv-toast" role="status"></div>`;
    el.replaceChildren(root);
    const $ = (s) => root.querySelector(s);
    const listEl = $(".iv-list");
    const overlay = $(".iv-overlay");

    async function req(url, body) {
      const r = await fetch(url, body ? { method: "POST", headers: { "content-type": "application/json" }, body: JSON.stringify(body) } : undefined);
      let d = null;
      try {
        d = await r.json();
      } catch {}
      if (!r.ok) throw new Error((d && d.error) || `${r.status} ${r.statusText}`);
      return d;
    }

    let toastTimer = null;
    function toast(text, ok = true, ms = 2600) {
      const t = $(".iv-toast");
      t.textContent = text;
      t.dataset.ok = ok;
      t.classList.add("show");
      clearTimeout(toastTimer);
      toastTimer = setTimeout(() => t.classList.remove("show"), ms);
    }

    async function startScan(full) {
      try {
        await req("/api/invoice/scan", { full });
      } catch (e) {
        return toast(e.message, false);
      }
      watching = {
        full,
        last: data?.progress.last ?? null,
        items: data?.items.length ?? 0,
        unknown: data?.unknown.length ?? 0,
        since: Date.now(),
      };
      toast(full ? "開始重新掃描…" : "檢查新的信…");
      tick(1500);
    }
    $(".iv-scan").onclick = () => startScan(false);
    $(".iv-full").onclick = () => {
      if (!confirm("全部重新掃描?(找到的發票會保留; 信已經刪除的會從清單移除)")) return;
      startScan(true);
    };

    // 手動開始的掃描結束了: 提示結果
    function checkDone() {
      if (!watching || !data) return;
      const p = data.progress;
      const finished = !p.running && p.last !== watching.last;
      // 等太久(例如 Gmail 沒有回應)也不要一直等
      const tooLong = Date.now() - watching.since > 10 * 60 * 1000;
      if (!finished && !tooLong) return;
      const w = watching;
      watching = null;
      if (p.error) return toast(`掃描失敗: ${p.error}`, false, 6000);
      if (!finished) return;
      const diff = data.items.length - w.items;
      const parts = [`${w.full ? "重新掃描" : "檢查"}完成`];
      if (diff > 0) parts.push(`新增 ${diff} 張發票`);
      else if (diff < 0) parts.push(`移除 ${-diff} 張(信已刪除)`);
      else parts.push("沒有新的發票");
      const du = data.unknown.length - w.unknown;
      if (du > 0) parts.push(`${du} 封找不到號碼`);
      toast(parts.join(", "), true, 5000);
    }
    $(".iv-add").onclick = () => manualDialog();

    // ---- 摘要與篩選 ----
    function renderTop() {
      const sum = $(".iv-sum");
      const p = data.progress;
      const won = data.items.filter((i) => i.status === "won" && !i.claimed);
      const parts = [];
      if (p.running) parts.push(`掃描中 ${p.done}/${p.total}…`);
      if (won.length) parts.push(`🎉 中獎 ${won.length} 張, 共 ${money(won.reduce((s, i) => s + (i.prize_amount || 0), 0))} 元還沒領`);
      parts.push(`共 ${data.items.length} 張發票`);
      if (data.unknown.length) parts.push(`${data.unknown.length} 封找不到號碼`);
      if (!p.running && p.last) parts.push(`上次檢查 ${new Date(p.last * 1000).toLocaleString("zh-TW", { hour12: false, month: "numeric", day: "numeric", hour: "2-digit", minute: "2-digit" })}`);
      sum.textContent = parts.join(" · ");
      sum.classList.toggle("bad", !!p.error);
      if (p.error) sum.textContent += ` · 錯誤: ${p.error}`;
      const f = $(".iv-filters");
      f.replaceChildren(
        ...FILTERS.map(([k, label]) => {
          const n = k === "all" ? data.items.length : k === "unknown" ? data.unknown.length : data.items.filter((i) => i.status === k || (k === "pending" && i.status === "waiting")).length;
          const b = h("button", "iv-chip", `${label}${n ? ` ${n}` : ""}`);
          b.classList.toggle("on", filter === k);
          b.onclick = () => {
            filter = k;
            render();
          };
          return b;
        }),
      );
    }

    // ---- 清單 ----
    function render() {
      if (!data) return;
      renderTop();
      listEl.replaceChildren();
      if (filter === "unknown") return renderUnknown();
      const items = data.items.filter((i) => filter === "all" || i.status === filter || (filter === "pending" && i.status === "waiting"));
      if (!items.length) {
        listEl.append(h("div", "iv-empty", data.progress.running ? "掃描中, 找到的發票會陸續出現…" : "沒有符合的發票"));
      }
      // 依期別分組
      const groups = new Map();
      for (const it of items) {
        if (!groups.has(it.period)) groups.set(it.period, []);
        groups.get(it.period).push(it);
      }
      for (const [period, list] of groups) {
        const g = h("section", "iv-group");
        const head = h("div", "iv-ghead");
        const first = list[0];
        const title = h("div", "iv-gtitle");
        title.append(h("b", "", periodLabel(period)));
        let info = "";
        if (first.status === "pending") info = `${md(first.draw)} 開獎`;
        else if (first.status === "expired") info = "領獎期限已過";
        else info = `領獎 ${md(first.claim_from)} ~ ${md(first.claim_to)}`;
        title.append(h("span", "iv-ginfo", info));
        head.append(title);
        // 這期全部可以刪的信(沒中獎、已過期)
        const del = [...new Map(list.filter((i) => i.deletable).map((i) => [`${i.account}:${i.msg}`, [i.account, i.msg]])).values()];
        if (del.length) {
          const b = h("button", "iv-link danger", `刪除沒中獎的信 (${del.length})`);
          b.onclick = () => deleteMails(del, `刪除 ${periodLabel(period)} 沒中獎(或已過期)的 ${del.length} 封信?\n會移到 Gmail 的垃圾桶, 30 天內可以救回。`);
          head.append(b);
        }
        g.append(head);
        for (const it of list) g.append(row(it));
        listEl.append(g);
      }
    }

    function row(it) {
      const r = h("div", `iv-item s-${it.status}`);
      const st = h("div", "iv-status");
      if (it.status === "won") {
        st.append(h("b", "", `${it.prize}`), h("span", "", `${money(it.prize_amount)} 元`));
      } else if (it.status === "expired" && it.prize) {
        st.append(h("b", "", "已過期"), h("span", "", `曾中 ${money(it.prize_amount)} 元`));
      } else {
        st.append(h("b", "", STATUS[it.status] || it.status));
        if (it.status === "pending") st.append(h("span", "", `${md(it.draw)} 開獎`));
      }
      const main = h("div", "iv-main");
      const no = h("div", "iv-no", fmtNo(it.number));
      const who = h(it.msg && !it.mail_gone ? "button" : "div", "iv-who", it.seller || it.subject || (it.manual ? "手動加入" : ""));
      if (it.msg && !it.mail_gone) {
        who.title = "打開這封信";
        who.onclick = () => openMail(it.account, it.msg);
      }
      const meta = h("div", "iv-meta");
      const bits = [];
      if (it.date) bits.push(md(it.date));
      if (it.amount) bits.push(`$${money(it.amount)}`);
      if (it.seller && it.subject) bits.push(it.subject);
      if (it.manual) bits.push("手動加入");
      if (it.mail_gone) bits.push("信已刪除");
      meta.textContent = bits.join(" · ");
      main.append(no, who, meta);
      const acts = h("div", "iv-acts");
      if (it.status === "won") {
        const c = h("label", "iv-claim");
        const box = document.createElement("input");
        box.type = "checkbox";
        box.checked = it.claimed;
        box.onchange = async () => {
          try {
            await req("/api/invoice/claimed", { key: it.key, claimed: box.checked });
            it.claimed = box.checked;
            renderTop();
          } catch (e) {
            box.checked = !box.checked;
            toast(e.message, false);
          }
        };
        c.append(box, h("span", "", "已領取"));
        acts.append(c);
        if (it.claim_to) acts.append(h("span", "iv-deadline", `${md(it.claim_to)} 截止`));
      }
      if (it.deletable) {
        const d = h("button", "iv-link danger", "刪除信件");
        d.onclick = () => deleteMails([[it.account, it.msg]], `把這封信移到 Gmail 的垃圾桶?\n${it.seller || ""} ${it.subject || ""}`);
        acts.append(d);
      }
      if (it.manual) {
        const d = h("button", "iv-link", "移除");
        d.title = "從清單移除(不會動到信件)";
        d.onclick = async () => {
          if (!confirm(`從清單移除 ${fmtNo(it.number)}?`)) return;
          try {
            await req("/api/invoice/remove", { key: it.key });
            load();
          } catch (e) {
            toast(e.message, false);
          }
        };
        acts.append(d);
      }
      r.append(st, main, acts);
      return r;
    }

    function renderUnknown() {
      if (!data.unknown.length) {
        listEl.append(h("div", "iv-empty", "沒有找不到號碼的信"));
        return;
      }
      listEl.append(h("div", "iv-hint", "這些信看起來和發票有關, 但找不到發票號碼。打開來看, 是發票就手動加入號碼, 不是的話按「不是發票」。"));
      for (const u of data.unknown) {
        const r = h("div", "iv-item s-unknown");
        const main = h("div", "iv-main");
        const who = h("button", "iv-who", u.from || "(沒有寄件者)");
        who.title = "打開這封信";
        who.onclick = () => openMail(u.account, u.msg);
        main.append(who, h("div", "iv-meta", `${new Date(u.received * 1000).toLocaleDateString("zh-TW")} · ${u.subject || "(沒有主旨)"}`));
        if (u.locked_pdf) main.append(h("div", "iv-locked", "🔒 附件 PDF 有密碼, 輸入密碼後會自動再找一次"));
        const acts = h("div", "iv-acts");
        if (u.locked_pdf && u.from_addr) {
          const pw = h("button", "iv-link", "輸入 PDF 密碼");
          pw.onclick = () => pdfPassword(u);
          acts.append(pw);
        }
        const open = h("button", "iv-link", "打開");
        open.onclick = () => openMail(u.account, u.msg);
        const add = h("button", "iv-link", "加入號碼");
        add.onclick = () => manualDialog(u);
        const no = h("button", "iv-link", "不是發票");
        no.onclick = async () => {
          try {
            await req("/api/invoice/dismiss", { account: u.account, msg: u.msg });
            data.unknown = data.unknown.filter((x) => x !== u);
            render();
          } catch (e) {
            toast(e.message, false);
          }
        };
        const del = h("button", "iv-link danger", "刪除信件");
        del.onclick = () => deleteMails([[u.account, u.msg]], `把這封信移到 Gmail 的垃圾桶?\n${u.from} ${u.subject}`);
        acts.append(open, add, no, del);
        r.append(main, acts);
        listEl.append(r);
      }
    }

    // 記住這個寄件者的 PDF 密碼(存在這台, 權限 600), 發票會自動再找一次
    function pdfPassword(u) {
      overlay.hidden = false;
      const box = h("form", "iv-dialog");
      box.innerHTML = `
        <h3>PDF 密碼</h3>
        <div class="iv-from"></div>
        <label>密碼<input name="pw" type="password" required autocomplete="off"></label>
        <div class="iv-hint2">記住後, 這個寄件者以後的 PDF 都會用這個密碼打開(郵件的附件預覽也是)。常見的是身分證字號。</div>
        <div class="iv-err" hidden></div>
        <div class="iv-dbtns"><button type="button" class="iv-btn iv-cancel">取消</button><button type="submit" class="iv-btn primary">記住並重新找</button></div>`;
      box.querySelector(".iv-from").textContent = `${u.from} <${u.from_addr}>`;
      box.querySelector(".iv-cancel").onclick = () => (overlay.hidden = true);
      box.onsubmit = async (e) => {
        e.preventDefault();
        try {
          await req("/api/mail/pdfpw", { addr: u.from_addr, password: String(new FormData(box).get("pw")) });
          overlay.hidden = true;
          toast("已記住, 重新找發票中…");
          setTimeout(load, 4000);
          setTimeout(load, 12000);
        } catch (ex) {
          const err = box.querySelector(".iv-err");
          err.textContent = ex.message;
          err.hidden = false;
        }
      };
      overlay.replaceChildren(box);
      box.querySelector("input").focus();
    }

    async function deleteMails(mails, question) {
      if (busy || !confirm(question)) return;
      busy = true;
      try {
        const r = await req("/api/invoice/delete", { mails });
        toast(`已刪除 ${r.deleted} 封信`);
        await load();
      } catch (e) {
        toast(e.message, false);
      } finally {
        busy = false;
      }
    }

    // ---- 手動加入 ----
    function manualDialog(from = null) {
      overlay.hidden = false;
      const box = h("form", "iv-dialog");
      const d0 = from ? new Date(from.received * 1000) : new Date();
      const date = `${d0.getFullYear()}-${String(d0.getMonth() + 1).padStart(2, "0")}-${String(d0.getDate()).padStart(2, "0")}`;
      box.innerHTML = `
        <h3>手動加入發票</h3>
        <div class="iv-from"></div>
        <label>發票號碼<input name="number" required placeholder="AB-12345678" autocapitalize="characters" autocomplete="off"></label>
        <label>發票日期<input name="date" type="date" required value="${date}"></label>
        <label>金額(選填)<input name="amount" type="number" min="0" inputmode="numeric"></label>
        <label>備註(選填)<input name="note" maxlength="200" placeholder="例如 商店名稱"></label>
        <div class="iv-err" hidden></div>
        <div class="iv-dbtns"><button type="button" class="iv-btn iv-cancel">取消</button><button type="submit" class="iv-btn primary">加入</button></div>`;
      if (from) box.querySelector(".iv-from").textContent = `${from.from} · ${from.subject}`;
      box.querySelector(".iv-cancel").onclick = () => (overlay.hidden = true);
      box.onsubmit = async (e) => {
        e.preventDefault();
        const f = new FormData(box);
        const err = box.querySelector(".iv-err");
        try {
          await req("/api/invoice/manual", {
            number: String(f.get("number")),
            date: String(f.get("date")),
            amount: f.get("amount") ? Number(f.get("amount")) : null,
            note: String(f.get("note") || ""),
            account: from?.account || null,
            msg: from?.msg || null,
          });
          overlay.hidden = true;
          toast("已加入");
          load();
        } catch (ex) {
          err.textContent = ex.message;
          err.hidden = false;
        }
      };
      overlay.replaceChildren(box);
      // 注意: onclick 回傳 false 會取消預設動作(「加入」送不出去), 所以要用大括號
      overlay.onclick = (e) => {
        if (e.target === overlay) overlay.hidden = true;
      };
      box.querySelector('[name="number"]').focus();
    }

    async function load() {
      try {
        data = await req("/api/invoice");
        if (!closed) {
          render();
          checkDone();
        }
      } catch (e) {
        listEl.replaceChildren(h("div", "iv-empty", `讀取失敗: ${e.message}`));
      }
    }

    // 掃描中(或等手動開始的掃描)常常更新, 平常每分鐘
    let timer = null;
    function tick(ms) {
      clearTimeout(timer);
      const wait = ms ?? (watching || data?.progress.running ? 1500 : 60000);
      timer = setTimeout(async () => {
        if (!document.hidden && overlay.hidden) await load();
        if (!closed) tick();
      }, wait);
    }
    load().then(() => tick());

    return () => {
      closed = true;
      clearTimeout(timer);
    };
  }

  return { mount };
})();

if (typeof Apps !== "undefined") Apps.registerView("invoice", InvoiceApp);

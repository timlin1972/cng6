// cng6 service worker
// - 推播通知: 網頁關著也收得到(新郵件、發票中獎), 點通知打開那封信
// - 離線: 連不到機器時, 顯示上次的畫面(資料要等連上才會更新)
// 只在瀏覽器信任的 HTTPS 下才會註冊(自簽憑證不行)。

const PAGE_CACHE = "cng6-page-v1";
// 頁面依裝置不同(電腦/手機), 但同一個瀏覽器一直是同一種, 存一份就好
const PAGE_KEY = "/";

self.addEventListener("install", () => self.skipWaiting());
self.addEventListener("activate", (e) => {
  e.waitUntil(
    (async () => {
      for (const k of await caches.keys()) if (k !== PAGE_CACHE) await caches.delete(k);
      await self.clients.claim();
    })(),
  );
});

// 主頁面: 先上網拿最新的, 拿不到才用上次存的
self.addEventListener("fetch", (e) => {
  const req = e.request;
  if (req.method !== "GET" || req.mode !== "navigate") return;
  const url = new URL(req.url);
  if (url.origin !== self.location.origin || url.pathname !== "/") return;
  e.respondWith(
    (async () => {
      const cache = await caches.open(PAGE_CACHE);
      try {
        const res = await fetch(req);
        if (res.ok) await cache.put(PAGE_KEY, res.clone());
        return res;
      } catch {
        const old = await cache.match(PAGE_KEY);
        if (old) return old;
        return new Response(
          `<!doctype html><meta charset="utf-8"><meta name="viewport" content="width=device-width, initial-scale=1">
<title>CNG6 · 離線</title>
<body style="font:16px system-ui,sans-serif;display:grid;place-items:center;height:90vh;margin:0;text-align:center;color:#888">
<div><p>連不到這台機器</p><button onclick="location.reload()" style="font:inherit;padding:.5em 1.2em">重新連線</button></div>`,
          { headers: { "content-type": "text/html; charset=utf-8" } },
        );
      }
    })(),
  );
});

self.addEventListener("push", (e) => {
  let d = {};
  try {
    d = e.data ? e.data.json() : {};
  } catch {
    d = { body: e.data ? e.data.text() : "" };
  }
  e.waitUntil(
    self.registration.showNotification(d.title || "CNG6", {
      body: d.body || "",
      tag: d.tag || undefined,
      icon: "/icons/icon-192.png",
      badge: "/icons/icon-192.png",
      data: { mail: d.mail || null },
    }),
  );
});

// 點通知: 已經開著的網頁就切過去, 沒有就開一個新的; 有信件就打開那封
self.addEventListener("notificationclick", (e) => {
  e.notification.close();
  const mail = e.notification.data && e.notification.data.mail;
  e.waitUntil(
    (async () => {
      const list = await self.clients.matchAll({ type: "window", includeUncontrolled: true });
      const c = list.find((w) => w.focused) || list[0];
      if (c) {
        await c.focus().catch(() => {});
        if (mail) c.postMessage({ type: "cng6-open-mail", account: mail[0], id: mail[1] });
        return;
      }
      const q = mail ? `?mail=${encodeURIComponent(mail[0])}&id=${encodeURIComponent(mail[1])}` : "";
      await self.clients.openWindow("/" + q);
    })(),
  );
});

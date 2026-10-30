// Kiwi service worker. The build stamps CACHE with a build hash so every deploy gets a fresh cache.
const CACHE = "kiwi-__BUILD_ID__";
const SHELL = "/";

self.addEventListener("install", (event) => {
  event.waitUntil(
    caches
      .open(CACHE)
      .then((cache) => cache.add(new Request(SHELL, { cache: "reload" })))
      .catch(() => {})
      .then(() => self.skipWaiting()),
  );
});

self.addEventListener("activate", (event) => {
  event.waitUntil(
    caches
      .keys()
      .then((keys) => Promise.all(keys.filter((key) => key.startsWith("kiwi-") && key !== CACHE).map((key) => caches.delete(key))))
      .then(() => self.clients.claim()),
  );
});

function isBypassed(url) {
  const path = url.pathname;
  // The server answers these itself: the API, OTA installs and enrollment profiles.
  return ["/api", "/ota", "/enroll"].some((p) => path === p || path.startsWith(`${p}/`)) || path === "/sw.js";
}

function storeResponse(event, cache, key, response) {
  // Keep the worker alive for the write without making rendering wait for disk. A full
  // cache must not turn a successful network response into a failed page or asset.
  event.waitUntil(cache.put(key, response.clone()).catch(() => {}));
}

async function networkFirst(request, event) {
  const cache = await caches.open(CACHE);
  try {
    const response = await fetch(request);
    const type = response.headers.get("Content-Type") || "";
    if (response.ok && type.includes("text/html")) storeResponse(event, cache, SHELL, response);
    // While Kiwi restarts, `tailscale serve` answers 502 with an empty page. The cached app shows
    // its own error state with "Try Again" instead (a Home Screen app has no reload button).
    if (response.status >= 500) {
      const cached = await cache.match(SHELL);
      if (cached) return cached;
    }
    return response;
  } catch (error) {
    const cached = await cache.match(SHELL);
    if (cached) return cached;
    throw error;
  }
}

async function cacheFirst(request, event) {
  const cache = await caches.open(CACHE);
  const cached = await cache.match(request);
  if (cached) return cached;
  const response = await fetch(request);
  if (response.ok) storeResponse(event, cache, request, response);
  return response;
}

self.addEventListener("fetch", (event) => {
  const { request } = event;
  if (request.method !== "GET") return;
  const url = new URL(request.url);
  if (url.origin !== self.location.origin || isBypassed(url)) return;

  if (request.mode === "navigate") {
    event.respondWith(networkFirst(request, event));
  } else if (url.pathname.startsWith("/assets/")) {
    event.respondWith(cacheFirst(request, event));
  }
});

self.addEventListener("push", (event) => {
  let data = {};
  try {
    data = event.data ? event.data.json() : {};
  } catch {
    data = { body: event.data ? event.data.text() : "" };
  }

  const tasks = [
    // Always show a notification: iOS revokes push for silent pushes.
    self.registration.showNotification(data.title || "Kiwi", {
      body: data.body || "",
      tag: data.tag || undefined,
      icon: "/icons/icon-192.png",
      data: { url: typeof data.url === "string" ? data.url : "/" },
    }),
  ];
  if (typeof data.badge === "number" && "setAppBadge" in self.navigator) {
    tasks.push(data.badge > 0 ? self.navigator.setAppBadge(data.badge) : self.navigator.clearAppBadge());
  }
  event.waitUntil(Promise.allSettled(tasks));
});

self.addEventListener("notificationclick", (event) => {
  event.notification.close();
  let target = new URL((event.notification.data && event.notification.data.url) || "/", self.location.origin);
  if (target.origin !== self.location.origin) target = new URL("/", self.location.origin);

  event.waitUntil(
    (async () => {
      const windows = await self.clients.matchAll({ type: "window", includeUncontrolled: true });
      const existing = windows.find((client) => new URL(client.url).origin === self.location.origin);
      if (existing) {
        await existing.focus();
        // The open app routes in place (see startRouter).
        if (existing.url !== target.href) {
          existing.postMessage({ type: "navigate", url: target.pathname + target.search });
        }
        return;
      }
      await self.clients.openWindow(target.href);
    })(),
  );
});

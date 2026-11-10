import { readFileSync } from "node:fs";
import { runInNewContext } from "node:vm";
import { describe, expect, it, vi } from "vitest";

const source = readFileSync(new URL("../public/sw.js", import.meta.url), "utf8");
const origin = "https://kiwi.example";

interface FetchEvent {
  request: Request;
  respondWith: (response: Promise<Response>) => void;
  waitUntil: (task: Promise<unknown>) => void;
}

function worker(options: { response?: Response; cached?: Response; write?: () => Promise<void> } = {}) {
  const listeners = new Map<string, (event: FetchEvent) => void>();
  const cache = {
    match: vi.fn(async () => options.cached),
    put: vi.fn(options.write ?? (async () => {})),
  };
  const fetch = vi.fn(async () => options.response ?? new Response("new"));
  runInNewContext(source, {
    self: {
      location: { origin },
      addEventListener: (type: string, listener: (event: FetchEvent) => void) => listeners.set(type, listener),
    },
    caches: { open: async () => cache },
    fetch,
    URL,
    Request,
  });
  return {
    fetch,
    cache,
    request(path: string, navigate = false) {
      const request = new Request(`${origin}${path}`);
      if (navigate) Object.defineProperty(request, "mode", { value: "navigate" });
      let response: Promise<Response> | undefined;
      const writes: Promise<unknown>[] = [];
      listeners.get("fetch")?.({
        request,
        respondWith: (value) => { response = value; },
        waitUntil: (value) => { writes.push(value); },
      });
      return { response, writes };
    },
  };
}

describe("service worker response caching", () => {
  it.each([false, true])("delivers the network response without waiting for disk (navigation: %s)", async (navigate) => {
    let finish = () => {};
    const write = new Promise<void>((resolve) => { finish = resolve; });
    const network = new Response("fresh", { headers: { "Content-Type": navigate ? "text/html" : "text/javascript" } });
    const sw = worker({ response: network, write: () => write });
    const event = sw.request(navigate ? "/apps" : "/assets/app-hash.js", navigate);
    expect(await event.response).toBe(network);
    expect(event.writes).toHaveLength(1);
    finish();
    await Promise.all(event.writes);
  });

  it.each([false, true])("keeps a successful response when the cache is full (navigation: %s)", async (navigate) => {
    const network = new Response("fresh", { headers: { "Content-Type": navigate ? "text/html" : "text/javascript" } });
    const sw = worker({ response: network, write: async () => { throw new Error("Quota exceeded"); } });
    const event = sw.request(navigate ? "/apps" : "/assets/app-hash.js", navigate);
    expect(await event.response).toBe(network);
    await expect(Promise.all(event.writes)).resolves.toEqual([undefined]);
  });

  it("serves cached assets without another network request", async () => {
    const cached = new Response("cached");
    const sw = worker({ cached });
    expect(await sw.request("/assets/app-hash.js").response).toBe(cached);
    expect(sw.fetch).not.toHaveBeenCalled();
    expect(sw.cache.put).not.toHaveBeenCalled();
  });

  it("still serves the cached shell during a server restart", async () => {
    const cached = new Response("shell");
    const sw = worker({ cached, response: new Response(null, { status: 502 }) });
    expect(await sw.request("/apps", true).response).toBe(cached);
    expect(sw.cache.put).not.toHaveBeenCalled();
  });

  it.each(["/api/apps", "/ota/token/manifest.plist", "/android/kiwi.apk", "/sw.js"])("bypasses %s", (path) => {
    const sw = worker();
    expect(sw.request(path).response).toBeUndefined();
    expect(sw.fetch).not.toHaveBeenCalled();
  });
});

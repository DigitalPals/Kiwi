import { describe, expect, it } from "vitest";
import { ApiError, NETWORK_ERROR_MESSAGE, TIMEOUT_MESSAGE, createClient } from "./client";

interface Call {
  input: string;
  init: RequestInit;
}

function fakeFetch(respond: (call: Call) => Response | Promise<Response>) {
  const calls: Call[] = [];
  const fetchImpl = async (input: string, init: RequestInit) => {
    const call = { input, init };
    calls.push(call);
    return respond(call);
  };
  return { calls, client: createClient(fetchImpl) };
}

/** A client whose first `/api/me` (the session gate) is already done. */
async function primed(respond: (call: Call) => Response | Promise<Response>) {
  const fake = fakeFetch(respond);
  await fake.client.me().catch(() => {});
  fake.calls.length = 0;
  return fake;
}

const json = (body: unknown, status = 200) =>
  new Response(JSON.stringify(body), { status, headers: { "Content-Type": "application/json" } });

const headers = (call: Call | undefined) => (call?.init.headers ?? {}) as Record<string, string>;

describe("api client", () => {
  it("sends GET requests without CSRF headers or body", async () => {
    const { calls, client } = await primed(() => json([]));
    await client.sources();
    expect(calls[0]?.input).toBe("/api/sources");
    expect(calls[0]?.init.method).toBe("GET");
    expect(calls[0]?.init.credentials).toBe("same-origin");
    expect(calls[0]?.init.body).toBeUndefined();
    expect(headers(calls[0])["X-Kiwi"]).toBeUndefined();
  });

  it("sends X-Kiwi and JSON content type on mutations", async () => {
    const { calls, client } = await primed(() => json({ id: "x" }, 201));
    await client.addSource("https://example.com/apps.json");
    expect(calls[0]?.init.method).toBe("POST");
    expect(headers(calls[0])["X-Kiwi"]).toBe("1");
    expect(headers(calls[0])["Content-Type"]).toBe("application/json");
    expect(calls[0]?.init.body).toBe(JSON.stringify({ url: "https://example.com/apps.json" }));
    expect(calls[0]?.init.credentials).toBe("same-origin");
  });

  it("sends an empty JSON object for body-less POSTs and no body for DELETE", async () => {
    const { calls, client } = await primed((call) => (call.init.method === "DELETE" ? new Response(null, { status: 204 }) : json([])));
    await client.prepareUpdates();
    await client.removeSource("abc");
    expect(calls[0]?.init.body).toBe("{}");
    expect(calls[1]?.init.method).toBe("DELETE");
    expect(calls[1]?.init.body).toBeUndefined();
    expect(headers(calls[1])["X-Kiwi"]).toBe("1");
  });

  it("encodes path segments and query parameters", async () => {
    const { calls, client } = await primed(() => json({}));
    await client.app("src/1", "com.example.app");
    await client.prepare("s", "com.example.app", { version: "1.0", build: "7" });
    await client.apps({ q: "you tube", source: "a&b" });
    await client.apps();
    await client.forget("com.example.app.TEAM");
    expect(calls[0]?.input).toBe("/api/apps/src%2F1/com.example.app");
    expect(calls[1]?.input).toBe("/api/apps/s/com.example.app/prepare");
    expect(calls[1]?.init.body).toBe(JSON.stringify({ version: "1.0", build: "7" }));
    expect(calls[2]?.input).toBe("/api/apps?q=you+tube&source=a%26b");
    expect(calls[3]?.input).toBe("/api/apps");
    expect(calls[4]?.input).toBe("/api/library/com.example.app.TEAM");
  });

  it("surfaces the server's error message and code", async () => {
    const { client } = fakeFetch(() =>
      json({ error: { code: "conflict", message: "This source is already added." } }, 409),
    );
    const error = await client.addSource("https://x").catch((e: unknown) => e);
    expect(error).toBeInstanceOf(ApiError);
    expect(error).toMatchObject({ status: 409, code: "conflict", message: "This source is already added." });
  });

  it("falls back to the status line for non-JSON errors", async () => {
    const { client } = fakeFetch(() => new Response("<html>bad gateway</html>", { status: 502, statusText: "Bad Gateway" }));
    await expect(client.me()).rejects.toMatchObject({ status: 502, code: "http", message: "502 Bad Gateway" });
  });

  it("reports network failures", async () => {
    const { client } = fakeFetch(() => {
      throw new TypeError("Load failed");
    });
    await expect(client.me()).rejects.toMatchObject({ status: 0, code: "network", message: NETWORK_ERROR_MESSAGE });
  });

  it("gives up on requests that hang and gives slow operations longer", async () => {
    const calls: string[] = [];
    // Never answers; rejects like fetch when the request is aborted.
    const hang = (input: string, init: RequestInit) => {
      calls.push(input);
      return new Promise<Response>((_, reject) => {
        init.signal?.addEventListener("abort", () => reject(new DOMException("Aborted", "AbortError")));
      });
    };
    const client = createClient(hang, { normal: 10, slow: 60, download: 60 });
    const started = Date.now();
    await expect(client.me()).rejects.toMatchObject({ status: 0, code: "timeout", message: TIMEOUT_MESSAGE });
    await expect(client.refreshSources()).rejects.toMatchObject({ code: "timeout" });
    expect(Date.now() - started).toBeGreaterThanOrEqual(60);
    expect(calls).toContain("/api/sources/refresh");
  });

  it("rejects non-JSON success responses (e.g. the SPA fallback)", async () => {
    const { client } = fakeFetch(() => new Response("<!doctype html>", { status: 200, headers: { "Content-Type": "text/html" } }));
    await expect(client.me()).rejects.toMatchObject({ code: "bad_response" });
  });

  it("completes the first /api/me before any other request", async () => {
    let release = () => {};
    const { calls, client } = fakeFetch((call) => {
      if (call.input === "/api/me" && calls.length === 1) {
        return new Promise<Response>((resolve) => {
          release = () => resolve(json({ device: { id: "local:x" } }));
        });
      }
      return json([]);
    });
    const apps = client.apps();
    const library = client.library();
    const me = client.me();
    await new Promise((r) => setTimeout(r, 10));
    expect(calls.map((c) => c.input)).toEqual(["/api/me"]);
    release();
    await Promise.all([apps, library, me]);
    expect(calls.map((c) => c.input).sort()).toEqual(["/api/apps", "/api/library", "/api/me"]);
    expect(calls[0]?.input).toBe("/api/me");
    // Afterwards requests go out directly.
    await client.sources();
    expect(calls.at(-1)?.input).toBe("/api/sources");
    expect(calls).toHaveLength(4);
  });

  it("shares concurrent reads, then fetches fresh data after completion", async () => {
    const { calls, client } = await primed(() => json([]));
    await Promise.all([client.signing(), client.signing(), client.signing()]);
    expect(calls.map((c) => c.input)).toEqual(["/api/signing"]);
    await client.signing();
    expect(calls).toHaveLength(2);
    await Promise.all([client.me(), client.me()]);
    expect(calls.filter((c) => c.input === "/api/me")).toHaveLength(1);
  });

  it("does not keep a failed read and allows the next attempt", async () => {
    const { calls, client } = await primed((call) => call.input === "/api/me"
      ? json({})
      : json({ error: { message: "Offline" } }, 503));
    const results = await Promise.allSettled([client.signing(), client.signing()]);
    expect(results.every((r) => r.status === "rejected")).toBe(true);
    expect(calls).toHaveLength(1);
    await expect(client.signing()).rejects.toMatchObject({ status: 503 });
    expect(calls).toHaveLength(2);
  });

  it("does not reuse reads from before or during a mutation", async () => {
    const pendingReads: ((response: Response) => void)[] = [];
    let finishWrite = (_response: Response) => {};
    const { calls, client } = await primed((call) => {
      if (call.input === "/api/me") return json({});
      return new Promise<Response>((resolve) => {
        if (call.init.method === "GET") pendingReads.push(resolve);
        else finishWrite = resolve;
      });
    });
    const before = client.sources();
    await Promise.resolve();
    const writing = client.updateSource("s", { enabled: false });
    await Promise.resolve();
    const during = client.sources();
    await Promise.resolve();
    expect(pendingReads).toHaveLength(2);
    finishWrite(json({ id: "s", enabled: false }));
    await writing;
    const after = client.sources();
    await Promise.resolve();
    expect(pendingReads).toHaveLength(3);
    // An old request completing must not remove the newest request from the shared map.
    pendingReads[0]?.(json(["old"]));
    await before;
    const joined = client.sources();
    await Promise.resolve();
    expect(pendingReads).toHaveLength(3);
    pendingReads[1]?.(json(["during"]));
    pendingReads[2]?.(json(["new"]));
    expect(await Promise.all([during, after, joined])).toEqual([["during"], ["new"], ["new"]]);
    expect(calls.filter((c) => c.init.method === "GET")).toHaveLength(3);
  });

  it("starts the session over after a failed first /api/me", async () => {
    let fail = true;
    const { calls, client } = fakeFetch((call) => {
      if (call.input === "/api/me" && fail) throw new TypeError("Load failed");
      return json([]);
    });
    await expect(client.sources()).resolves.toEqual([]);
    expect(calls.map((c) => c.input)).toEqual(["/api/me", "/api/sources"]);
    fail = false;
    await client.sources();
    expect(calls.map((c) => c.input)).toEqual(["/api/me", "/api/sources", "/api/me", "/api/sources"]);
    await client.sources();
    expect(calls).toHaveLength(5);
  });

  it("returns undefined for 204", async () => {
    const { client } = fakeFetch(() => new Response(null, { status: 204 }));
    await expect(client.forget("x")).resolves.toBeUndefined();
  });
});

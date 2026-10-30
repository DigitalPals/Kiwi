import type { IncomingMessage, ServerResponse } from "node:http";
import type { Plugin } from "vite";
import { VIEWER_COOKIE, createMockApi, type MockApi } from "./api.ts";

function readBody(req: IncomingMessage): Promise<string> {
  return new Promise((resolve, reject) => {
    const chunks: Buffer[] = [];
    req.on("data", (chunk: Buffer) => chunks.push(chunk));
    req.on("end", () => resolve(Buffer.concat(chunks).toString("utf8")));
    req.on("error", reject);
  });
}

function escapeXml(text: string): string {
  return text.replace(/[<>&"]/g, (c) => `&#${c.charCodeAt(0)};`);
}

function iconSvg(name: string, tint: string): string {
  return `<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 120 120"><defs><linearGradient id="g" x1="0" y1="0" x2="0" y2="1"><stop offset="0" stop-color="${tint}" stop-opacity="0.75"/><stop offset="1" stop-color="${tint}"/></linearGradient></defs><rect width="120" height="120" fill="url(#g)"/><text x="60" y="79" font-family="-apple-system,Helvetica,sans-serif" font-size="56" font-weight="700" fill="#fff" text-anchor="middle">${escapeXml(name.charAt(0))}</text></svg>`;
}

function screenshotSvg(name: string, tint: string, n: number): string {
  const rows = Array.from(
    { length: 6 },
    (_, i) => `<rect x="60" y="${560 + i * 260}" width="1059" height="210" rx="36" fill="#fff" fill-opacity="${0.14 + (i % 2) * 0.06}"/>`,
  ).join("");
  return `<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 1179 2556"><defs><linearGradient id="g" x1="0" y1="0" x2="1" y2="1"><stop offset="0" stop-color="${tint}"/><stop offset="1" stop-color="#111"/></linearGradient></defs><rect width="1179" height="2556" fill="url(#g)"/><text x="80" y="330" font-family="-apple-system,Helvetica,sans-serif" font-size="120" font-weight="800" fill="#fff">${escapeXml(name)}</text><text x="80" y="450" font-family="-apple-system,Helvetica,sans-serif" font-size="64" fill="#fff" fill-opacity="0.75">Screenshot ${n}</text>${rows}</svg>`;
}

const delay = (ms: number) => new Promise((resolve) => setTimeout(resolve, ms));

function latency(method: string, path: string): number {
  if (method === "POST" && path === "/api/sources") return 900;
  if (path.includes("refresh") || path.endsWith("/sync") || path.endsWith("/rescan")) return 700;
  return 120 + Math.round(Math.random() * 200);
}

function send(res: ServerResponse, status: number, contentType: string, body: string) {
  res.statusCode = status;
  res.setHeader("Content-Type", contentType);
  res.setHeader("Cache-Control", "no-store");
  res.end(body);
}

/** Serves /api and /ota from in-memory fixtures (dev server only). */
export function mockApi(): Plugin {
  let api: MockApi;
  return {
    name: "kiwi-mock-api",
    apply: "serve",
    configureServer(server) {
      api = createMockApi();
      server.config.logger.info("  Kiwi mock API enabled (VITE_MOCK=1)");
      server.middlewares.use((req, res, next) => {
        const url = new URL(req.url ?? "/", "http://localhost");
        const art = /^\/__mock\/(icon|shot)\/([a-z0-9-]+?)(?:-(\d+))?\.svg$/.exec(url.pathname);
        if (art) {
          const info = api.artwork(art[2] ?? "");
          if (!info) return send(res, 404, "text/plain", "not found");
          const svg = art[1] === "icon" ? iconSvg(info.name, info.tint) : screenshotSvg(info.name, info.tint, Number(art[3] ?? 1));
          return send(res, 200, "image/svg+xml", svg);
        }
        // `?as=emma` views the app as a family member's iPhone (`?as=alex` is the owner again).
        const as = url.searchParams.get("as");
        if (as !== null && req.method === "GET" && !url.pathname.startsWith("/api/")) {
          url.searchParams.delete("as");
          res.statusCode = 302;
          res.setHeader("Set-Cookie", `${VIEWER_COOKIE}=${/^[a-z]+$/.test(as) ? as : "alex"}; Path=/; SameSite=Lax`);
          res.setHeader("Location", url.pathname + url.search);
          res.end();
          return;
        }
        if (!["/api/", "/ota/", "/enroll/"].some((p) => url.pathname.startsWith(p))) return next();

        void (async () => {
          const body = await readBody(req);
          const method = req.method ?? "GET";
          await delay(latency(method, url.pathname));
          const headers: Record<string, string | undefined> = {};
          for (const [k, v] of Object.entries(req.headers)) headers[k] = Array.isArray(v) ? v.join(",") : v;
          const result = api.handle({
            method,
            path: url.pathname,
            query: url.searchParams,
            headers,
            body,
            origin: `http://${req.headers.host ?? "localhost:5173"}`,
          });
          for (const [name, value] of Object.entries(result.headers ?? {})) res.setHeader(name, value);
          if (result.status === 204 || result.status === 302) {
            res.statusCode = result.status;
            res.end();
          } else if (result.text !== undefined) {
            send(res, result.status, result.contentType ?? "text/plain", result.text);
          } else {
            send(res, result.status, "application/json", JSON.stringify(result.json));
          }
        })().catch((error: unknown) => send(res, 500, "text/plain", String(error)));
      });
    },
  };
}

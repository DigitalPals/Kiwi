import { describe, expect, it } from "vitest";
import type {
  AppDetail,
  AppSummary,
  AutomationOverview,
  AutomationState,
  Build,
  Me,
  Settings,
  Source,
  StoreApp,
} from "../src/api/types.ts";
import { compareVersions, createMockApi, shortName } from "./api.ts";

function setup() {
  let now = Date.parse("2026-10-29T12:00:00Z");
  const api = createMockApi(() => now);
  const request = (method: string, path: string, body?: unknown) => {
    const url = new URL(path, "http://localhost:5173");
    return api.handle({
      method,
      path: url.pathname,
      query: url.searchParams,
      headers: method === "GET" ? {} : { "x-kiwi": "1", "content-type": "application/json" },
      body: body === undefined ? "" : JSON.stringify(body),
      origin: "http://localhost:5173",
    });
  };
  return { api, request, advance: (ms: number) => (now += ms) };
}

const actionOf = (apps: unknown, bundleId: string) =>
  (apps as AppSummary[]).find((a) => a.bundle_id === bundleId)?.state.action;

describe("mock api", () => {
  it("answers builds_off while Kiwi Builds is turned off", () => {
    const { request } = setup();
    const off = request("PATCH", "/api/settings", { kiwi_builds: false });
    expect((off.json as Settings).kiwi_builds).toBe(false);
    const r = request("GET", "/api/automation");
    expect(r.status).toBe(409);
    expect((r.json as { error: { code: string } }).error.code).toBe("builds_off");
    request("PATCH", "/api/settings", { kiwi_builds: true });
    expect(request("GET", "/api/automation").status).toBe(200);
  });

  it("lists the apps Kiwi builds and saves each app's settings", () => {
    const { request } = setup();
    const overview = request("GET", "/api/automation").json as AutomationOverview;
    expect(overview.apps.map((a) => a.id)).toEqual(["youtube", "youtube-music", "twitch", "x", "reddit", "rambler", "gboard"]);
    expect(overview.apps.map((a) => a.added)).toEqual([true, false, false, false, false, false, false]);
    const initial = request("GET", "/api/automation/youtube").json as AutomationState;
    expect(initial.available_tweaks.map((t) => t.id)).toContain("youpip");
    expect(initial.included.map((t) => t.name)).toEqual(["YouMod"]);
    expect(initial.published).toBeNull();
    const preferences = { enabled: false, interval_minutes: 150, tweaks: ["youpip"] };
    const saved = request("PATCH", "/api/automation/youtube", preferences).json as AutomationState;
    expect(saved.preferences).toEqual(preferences);
    expect(saved.next_check_at).toBeNull();
    expect(saved.jobs).toHaveLength(0);
    const checked = request("POST", "/api/automation/youtube/check", {}).json as AutomationState;
    expect(checked.jobs[0]?.preferences).toEqual(preferences);
    expect(checked.jobs[0]?.status).toBe("blocked");
    expect(checked.published).toBeNull();
    expect((request("GET", "/api/automation/youtube").json as AutomationState).preferences).toEqual(preferences);
  });

  it("adds an app with a name choice, checks it and removes it", () => {
    const { request } = setup();
    const x = request("GET", "/api/automation/x").json as AutomationState;
    expect(x.names).toEqual(["Twitter", "X"]);
    expect(x.preferences.name).toBe("Twitter");
    expect(x.warning).toBeTruthy();
    expect(request("POST", "/api/automation/x/check", {}).status).toBe(400);
    expect(request("POST", "/api/automation/x/add", { ...x.preferences, name: "Bird" }).status).toBe(400);
    const added = request("POST", "/api/automation/x/add", { ...x.preferences, name: "X" });
    expect(added.status).toBe(202);
    expect((added.json as AutomationState).added).toBe(true);
    expect((added.json as AutomationState).jobs[0]?.app).toBe("x");
    expect((request("DELETE", "/api/automation/x").json as AutomationState).added).toBe(false);
    expect(request("GET", "/api/automation/instagram").status).toBe(404);
  });

  it("simulates builds that publish Kiwi revisions in the Kiwi Builds source", () => {
    let now = Date.parse("2026-10-29T12:00:00Z");
    const api = createMockApi(() => now, { builds: "simulated" });
    const request = (method: string, path: string, body?: unknown) =>
      api.handle({
        method,
        path,
        query: new URLSearchParams(),
        headers: method === "GET" ? {} : { "x-kiwi": "1", "content-type": "application/json" },
        body: body === undefined ? "" : JSON.stringify(body),
        origin: "http://localhost:5173",
      });
    const youtube = () => request("GET", "/api/automation/youtube").json as AutomationState;
    const music = request("GET", "/api/automation/youtube-music").json as AutomationState;

    request("POST", "/api/automation/youtube/check", { force: false });
    expect(youtube().jobs[0]).toMatchObject({ status: "running", stage: "Checking build setup" });
    request("POST", "/api/automation/youtube-music/add", music.preferences);
    expect((request("GET", "/api/automation/youtube-music").json as AutomationState).jobs[0]?.stage).toBe("Waiting for YouTube");
    expect(request("POST", "/api/automation/youtube/check", {}).status).toBe(409);
    now += 5 * 2600;
    expect(youtube().jobs[0]).toMatchObject({ status: "running", stage: "Injecting tweaks", version: "21.44.0" });
    expect((request("GET", "/api/automation").json as AutomationOverview).checks.every((c) => c.ready)).toBe(true);
    now += 3 * 2600;
    const built = youtube();
    expect(built.jobs[0]).toMatchObject({ status: "ready", stage: "Signed and available in Kiwi" });
    expect(built.published).toMatchObject({ version: "21.44.0", revision: 1 });
    expect((request("GET", "/api/automation/youtube-music").json as AutomationState).jobs[0]?.status).toBe("running");
    const sources = request("GET", "/api/sources").json as Source[];
    expect(sources.find((s) => s.id === "kiwi-youtube")).toMatchObject({ kind: "local_build", name: "Kiwi Builds" });
    const app = request("GET", "/api/apps/kiwi-youtube/com.google.ios.youtube").json as AppDetail;
    expect(app.name).toBe("YouTube");
    expect(app.state.action).toBe("install");

    // Unchanged recipes are skipped; changed patches make a new revision of the same version.
    now += 60_000;
    request("POST", "/api/automation/youtube/check", {});
    now += 3 * 2600;
    expect(youtube().jobs[0]).toMatchObject({ status: "unchanged", stage: "Already up to date" });
    request("PATCH", "/api/automation/youtube", { ...built.preferences, tweaks: ["youpip"] });
    expect(youtube().configuration_changed).toBe(true);
    request("POST", "/api/automation/youtube/check", {});
    now += 8 * 2600;
    expect(youtube()).toMatchObject({ configuration_changed: false, published: { revision: 3, tweaks: ["youpip"] } });
    const detail = request("GET", "/api/apps/kiwi-youtube/com.google.ios.youtube").json as AppDetail;
    expect(detail.releases.map((r) => r.revision)).toEqual([3, 1]);

    // A new App Store version is built on the next check.
    expect(api.releaseOnAppStore("com.google.ios.youtube", "21.45.0")).toBe(true);
    request("POST", "/api/automation/youtube/check", {});
    now += 8 * 2600;
    expect(youtube().published).toMatchObject({ version: "21.45.0", revision: 4 });

    request("POST", "/api/automation/youtube/check", { force: true });
    request("POST", "/api/automation/youtube/cancel");
    expect(youtube().jobs[0]).toMatchObject({ status: "cancelled", stage: "Cancelled" });
  });

  it("publishes the native Rambler app without claiming App Store setup is ready", () => {
    let now = Date.parse("2026-10-29T12:00:00Z");
    const api = createMockApi(() => now, { builds: "simulated" });
    const request = (method: string, path: string, body?: unknown) => api.handle({
      method, path, query: new URLSearchParams(),
      headers: method === "GET" ? {} : { "x-kiwi": "1", "content-type": "application/json" },
      body: body === undefined ? "" : JSON.stringify(body), origin: "http://localhost:5173",
    });
    const initial = request("GET", "/api/automation/rambler").json as AutomationState;
    expect(initial).toMatchObject({ added: false, experimental: true, included: [], available_tweaks: [] });
    expect(initial.preferences.tweaks).toEqual([]);
    expect(request("POST", "/api/automation/rambler/add", initial.preferences).status).toBe(202);
    now += 9 * 2600;
    const ready = request("GET", "/api/automation/rambler").json as AutomationState;
    expect(ready.jobs[0]).toMatchObject({ status: "ready", external_id: expect.stringMatching(/^native-/) });
    expect(ready.published).toMatchObject({ version: "0.1.0", build: "1", revision: 1 });
    expect(ready.checks).toEqual([{ name: "Native iOS build tools", ready: true }]);
    expect((request("GET", "/api/automation").json as AutomationOverview).checks).toEqual([]);
    const app = request("GET", "/api/apps/kiwi-youtube/com.kiwi.ramblerpoc").json as AppDetail;
    expect(app.state.action).toBe("install");
    expect(app.releases[0]).toMatchObject({ version: "0.1.0", build: "1", min_os: "16.0" });
    expect(app.state.install_url).toBeTruthy();
    expect(request("POST", "/api/automation/rambler/check", {}).status).toBe(202);
    now += 3 * 2600;
    expect((request("GET", "/api/automation/rambler").json as AutomationState).jobs[0]?.status).toBe("unchanged");
  });

  it("publishes experimental Gboard with its original version and separate Apple build number", () => {
    let now = Date.parse("2026-10-29T12:00:00Z");
    const api = createMockApi(() => now, { builds: "simulated" });
    const request = (method: string, path: string, body?: unknown) => api.handle({
      method, path, query: new URLSearchParams(),
      headers: method === "GET" ? {} : { "x-kiwi": "1", "content-type": "application/json" },
      body: body === undefined ? "" : JSON.stringify(body), origin: "http://localhost:5173",
    });
    const initial = request("GET", "/api/automation/gboard").json as AutomationState;
    expect(initial).toMatchObject({ added: false, experimental: true, app_version: "2.3.19", available_tweaks: [] });
    expect(request("POST", "/api/automation/gboard/add", initial.preferences).status).toBe(202);
    now += 12 * 2600;
    const ready = request("GET", "/api/automation/gboard").json as AutomationState;
    expect(ready.jobs[0]?.status).toBe("ready");
    expect(ready.published).toMatchObject({ version: "2.3.19", build: "2.3.19.444584732", revision: 1 });
    const app = request("GET", "/api/apps/kiwi-youtube/com.google.keyboard").json as AppDetail;
    expect(app.name).toBe("Gboard Rambler");
    expect(app.releases[0]?.min_os).toBe("16.0");
  });

  it("covers every action in the fixtures", () => {
    const { request } = setup();
    const apps = request("GET", "/api/apps").json as AppSummary[];
    const actions = new Set(apps.map((a) => a.state.action));
    for (const action of ["get", "preparing", "install", "installing", "open", "update", "retry", "unavailable"]) {
      expect(actions).toContain(action);
    }
    expect(apps.find((a) => a.state.action === "open" && a.state.open_url)).toBeTruthy();
    expect(apps.find((a) => a.state.action === "open" && !a.state.open_url)).toBeTruthy();
  });

  it("simulates prepare, ready, manifest fetch and install", () => {
    const { request, advance } = setup();
    const prepared = request("POST", "/api/apps/ytlite/com.atebits.Tweetie2/prepare", {});
    expect(prepared.status).toBe(202);
    const build = prepared.json as Build;
    expect(actionOf(request("GET", "/api/apps").json, "com.atebits.Tweetie2")).toBe("preparing");

    advance(15_000);
    const ready = request("GET", `/api/builds/${build.id}`).json as Build;
    expect(ready.status).toBe("ready");
    const detail = request("GET", "/api/apps/ytlite/com.atebits.Tweetie2").json as AppDetail;
    expect(detail.state.action).toBe("install");
    expect(detail.state.install_url).toMatch(/^itms-services:\/\/\?action=download-manifest&url=/);

    const manifest = decodeURIComponent(detail.state.install_url?.split("url=")[1] ?? "");
    const plist = request("GET", new URL(manifest).pathname);
    expect(plist.contentType).toBe("application/xml");
    expect(actionOf(request("GET", "/api/apps").json, "com.atebits.Tweetie2")).toBe("installing");

    advance(7_000);
    const done = request("GET", "/api/apps/ytlite/com.atebits.Tweetie2").json as AppDetail;
    expect(done.state.action).toBe("open");
    expect(done.state.open_url).toMatch(/^kiwi-[0-9a-f]{12}:\/\/$/);
  });

  it("offers another source's app with the same bundle id as a replacement", () => {
    const { request, advance } = setup();
    const youmod = request("GET", "/api/apps/youmod/com.google.ios.youtube").json as AppDetail;
    expect(youmod.state.action).toBe("get");
    expect(youmod.state.installed).toBeNull();
    expect(youmod.state.replaces).toEqual({ name: "YouTubePlus", source_name: "YTLite Repository", version: "21.43.5" });
    const updates = request("GET", "/api/updates").json as AppSummary[];
    expect(updates.some((a) => a.source_id === "youmod")).toBe(false);

    const build = request("POST", "/api/apps/youmod/com.google.ios.youtube/prepare", {}).json as Build;
    advance(15_000);
    const ready = request("GET", `/api/builds/${build.id}`).json as Build;
    const manifest = decodeURIComponent(ready.install_url?.split("url=")[1] ?? "");
    request("GET", new URL(manifest).pathname);
    advance(7_000);
    const done = request("GET", "/api/apps/youmod/com.google.ios.youtube").json as AppDetail;
    expect(done.state.action).toBe("open");
    expect(done.state.replaces).toBeNull();
    const ytplus = request("GET", "/api/apps/ytlite/com.google.ios.youtube").json as AppDetail;
    expect(ytplus.state.installed).toBeNull();
    expect(ytplus.state.replaces).toMatchObject({ name: "YouMod", source_name: "YouMod Repo" });
  });

  it("enforces the CSRF header on mutations", () => {
    const { api } = setup();
    const res = api.handle({
      method: "POST",
      path: "/api/sources/refresh",
      query: new URLSearchParams(),
      headers: {},
      body: "",
      origin: "http://localhost",
    });
    expect(res.status).toBe(403);
    expect(res.json).toMatchObject({ error: { code: "csrf" } });
  });

  it("validates new sources", () => {
    const { request } = setup();
    expect(request("POST", "/api/sources", { url: "http://insecure.example/apps.json" }).status).toBe(400);
    expect(request("POST", "/api/sources", { url: "https://apps.altstore.io/" }).status).toBe(409);
    expect(request("POST", "/api/sources", { url: "https://fail.example/apps.json" }).json).toMatchObject({
      error: { code: "source_fetch" },
    });
    expect(request("POST", "/api/sources", { url: "https://new.example/apps.json" }).status).toBe(201);
  });

  it("counts updates and prepares them all", () => {
    const { request } = setup();
    const me = request("GET", "/api/me").json as Me;
    const updates = request("GET", "/api/updates").json as AppSummary[];
    expect(me.counts.updates).toBe(updates.length);
    expect(updates.length).toBeGreaterThan(0);
    const builds = request("POST", "/api/updates/prepare").json as Build[];
    expect(builds).toHaveLength(updates.length);
  });

  it("hides apps and picks them from a source", () => {
    const { request } = setup();
    const source = () => (request("GET", "/api/sources").json as Source[]).find((s) => s.id === "driftywinds");
    expect(source()).toMatchObject({ browse: "picked", app_count: 10, shown_count: 4, new_count: 1 });
    const names = (apps: unknown) => (apps as AppSummary[]).map((a) => a.name);
    expect(names(request("GET", "/api/apps?source=driftywinds").json)).toEqual([
      "Feather",
      "Spotify (EeveeSpotifyReincarnated)",
      "YouMod + Tweaks (driftywinds)",
      "YT Music Ultimate",
    ]);
    const all = request("GET", "/api/apps?source=driftywinds&hidden=1").json as AppSummary[];
    expect(all).toHaveLength(10);
    expect(all.find((a) => a.name === "LiveContainer")).toMatchObject({ hidden: true, is_new: true });

    const picked = request("PATCH", "/api/apps/driftywinds/kh.crysalis.feather", { hidden: true }).json as AppDetail;
    expect(picked.hidden).toBe(true);
    expect(source()?.shown_count).toBe(3);
    request("PATCH", "/api/sources/driftywinds", { browse: "all" });
    expect(source()).toMatchObject({ browse: "all", shown_count: 9, new_count: 0 });
    expect(request("PATCH", "/api/sources/driftywinds", { browse: "some" }).status).toBe(400);
  });

  it("compares apps with their App Store original", () => {
    const { request } = setup();
    const spotify = request("GET", "/api/apps/driftywinds/com.spotify.client").json as AppDetail;
    expect(spotify.original_match).toBe("bundle_id");
    expect(spotify.original).toMatchObject({
      short_name: "Spotify",
      version: "9.1.94",
      app_version: "9.1.50",
      status: "behind",
    });
    expect(spotify.other_versions.map((o) => o.name)).toEqual(["EeveeSpotify"]);
    const ytplus = request("GET", "/api/apps/ytlite/com.google.ios.youtube").json as AppDetail;
    expect(ytplus.original?.status).toBe("same");
    expect(ytplus.other_versions.map((o) => o.source_id).sort()).toEqual(["driftywinds", "youmod"]);
    const feather = request("GET", "/api/apps/driftywinds/kh.crysalis.feather").json as AppDetail;
    expect([feather.original, feather.original_match]).toEqual([null, "not_found"]);

    const found = request("GET", "/api/store/search?term=youtube").json as StoreApp[];
    expect(found.map((a) => a.short_name)).toEqual(["YouTube", "YouTube Music", "YouTube Kids"]);
    const linked = request("PATCH", "/api/apps/driftywinds/kh.crysalis.feather", {
      original: { bundle_id: "com.google.ios.youtubekids" },
    }).json as AppDetail;
    expect([linked.original_match, linked.original?.short_name]).toEqual(["linked", "YouTube Kids"]);
    const none = request("PATCH", "/api/apps/driftywinds/kh.crysalis.feather", { original: { bundle_id: null } });
    expect((none.json as AppDetail).original_match).toBe("none");
    expect(request("PATCH", "/api/apps/driftywinds/kh.crysalis.feather", { original: { bundle_id: "x.y" } }).status).toBe(400);

    const settings = request("PATCH", "/api/settings", { app_store_country: "US" }).json as Settings;
    expect(settings.app_store_country).toBe("us");
    const us = request("GET", "/api/apps/driftywinds/com.spotify.client").json as AppDetail;
    expect(us.original?.name).toBe("Spotify: Music and Podcasts");
    expect(request("PATCH", "/api/settings", { app_store_country: "usa" }).status).toBe(400);
  });

  it("shortens App Store names like the server", () => {
    expect(shortName("Spotify: Muziek en podcasts")).toBe("Spotify");
    expect(shortName("Delta - Game Emulator")).toBe("Delta");
    expect(shortName("X")).toBe("X");
  });

  it("compares versions numerically", () => {
    expect(compareVersions("21.44.0", "21.43.5")).toBe(1);
    expect(compareVersions("1.10", "1.9")).toBe(1);
    expect(compareVersions("2.0", "2.0.0")).toBe(0);
    expect(compareVersions("1.0", "1.0.1")).toBe(-1);
  });
});

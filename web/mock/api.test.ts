import { describe, expect, it } from "vitest";
import type {
  AppDetail,
  AppSummary,
  AutomationOverview,
  AutomationState,
  Build,
  Me,
  Settings,
  Signing,
  Source,
  StoreApp,
} from "../src/api/types.ts";
import { MOCK_ANDROID_SIGNER, compareVersions, createMockApi, shortName } from "./api.ts";

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
    expect(overview.apps.map((a) => a.id)).toEqual([
      "youtube", "youtube-music", "twitch", "x", "reddit", "android-youtube", "android-youtube-music", "android-reddit",
    ]);
    expect(overview.apps.map((a) => a.platform)).toEqual(["ios", "ios", "ios", "ios", "ios", "android", "android", "android"]);
    expect(overview.apps.map((a) => a.added)).toEqual([true, false, false, false, false, false, false, false]);
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

  it("builds Android apps that an Android phone installs as APK downloads", () => {
    let now = Date.parse("2026-10-29T12:00:00Z");
    const api = createMockApi(() => now, { builds: "simulated", device: "android" });
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
    const me = request("GET", "/api/me").json as Me;
    expect(me.device).toMatchObject({ platform: "android", model: "Android", os_version: null });
    expect(me.counts.installed).toBe(0);
    // AltStore sources list iPhone apps, which Browse leaves out on Android.
    expect(request("GET", "/api/apps").json).toEqual([]);
    const other = request("GET", "/api/apps/ytlite/com.google.ios.youtube").json as AppDetail;
    expect(other).toMatchObject({ platform: "ios", unavailable: "Made for iPhone and iPad", state: { action: "unavailable" } });

    const recipe = request("GET", "/api/automation/android-youtube").json as AutomationState;
    expect(recipe).toMatchObject({ platform: "android", bundle_id: "app.morphe.android.youtube", added: false });
    request("POST", "/api/automation/android-youtube/add", recipe.preferences);
    now += 3 * 2600;
    const running = request("GET", "/api/automation/android-youtube").json as AutomationState;
    expect(running.jobs[0]).toMatchObject({ status: "running", stage: "Patching YouTube", version: "21.30.216" });
    // No Apple account or decryption: the Mac's setup checks stay untouched.
    expect(running.checks).toEqual([]);
    now += 2 * 2600;
    const built = request("GET", "/api/automation/android-youtube").json as AutomationState;
    expect(built.jobs[0]).toMatchObject({ status: "ready", stage: "Available in Kiwi" });

    const apps = request("GET", "/api/apps").json as AppSummary[];
    expect(apps.map((a) => a.bundle_id)).toEqual(["app.morphe.android.youtube"]);
    const app = request("GET", "/api/apps/kiwi-youtube/app.morphe.android.youtube").json as AppDetail;
    expect(app).toMatchObject({
      platform: "android", name: "YouTube", original: null, original_match: "none",
      signed_bundle_id: "app.morphe.android.youtube", dropped_entitlements: [],
    });
    expect(app.releases[0]).toMatchObject({ min_os: null, compatible: true });
    // Published builds are ready: the button is the APK itself.
    expect(app.state.action).toBe("install");
    const link = new URL(app.state.install_url ?? "");
    expect(link.pathname).toMatch(/^\/ota\/[^/]+\/app\.apk$/);
    expect(request("GET", link.pathname.replace("app.apk", "manifest.plist")).status).toBe(404);
    const download = request("GET", link.pathname);
    expect(download).toMatchObject({ status: 200, contentType: "application/vnd.android.package-archive" });
    const installed = request("GET", "/api/apps/kiwi-youtube/app.morphe.android.youtube").json as AppDetail;
    expect(installed.state).toMatchObject({
      action: "open", open_url: null,
      installed: { version: "21.30.216", signed_bundle_id: "app.morphe.android.youtube", expires_at: null },
    });
    expect((request("GET", "/api/me").json as Me).warnings.filter((w) => w.code === "expiring")).toEqual([]);
  });

  it("shows an iPhone the Android apps as made for Android", () => {
    let now = Date.parse("2026-10-29T12:00:00Z");
    const api = createMockApi(() => now, { builds: "simulated" });
    const request = (method: string, path: string, body?: unknown) =>
      api.handle({
        method, path, query: new URLSearchParams(),
        headers: method === "GET" ? {} : { "x-kiwi": "1", "content-type": "application/json" },
        body: body === undefined ? "" : JSON.stringify(body),
        origin: "http://localhost:5173",
      });
    const recipe = request("GET", "/api/automation/android-reddit").json as AutomationState;
    request("POST", "/api/automation/android-reddit/add", recipe.preferences);
    now += 5 * 2600;
    expect((request("GET", "/api/apps").json as AppSummary[]).some((a) => a.platform === "android")).toBe(false);
    const app = request("GET", "/api/apps/kiwi-youtube/com.reddit.frontpage").json as AppDetail;
    expect(app).toMatchObject({ platform: "android", unavailable: "Made for Android devices", state: { action: "unavailable" } });
    expect((request("GET", "/api/me").json as Me).device.platform).toBe("ios");
  });

  it("reports the device's platform, features and the first-run setup", () => {
    const { request } = setup();
    const me = request("GET", "/api/me").json as Me;
    expect(me.device).toMatchObject({ platform: "ios", platform_source: "detected", detected_platform: "ios", client: "web" });
    expect(me.onboarded).toBe(true);
    expect(me.host).toEqual({ os: "macos", ios_signing: "keychain", devicectl: true });
    expect(me.features).toEqual(["ios_apps", "home_screen_app", "web_push", "devicectl"]);
    expect(me.android_app).toMatchObject({ package: "io.github.digitalpals.kiwi", install_url: "http://localhost:5173/android/kiwi.apk" });
    expect(request("GET", "/android/kiwi.apk")).toMatchObject({ status: 200, contentType: "application/vnd.android.package-archive" });
    // The owner's choice wins; null goes back to detection.
    const chosen = request("PATCH", "/api/me", { platform: "android" }).json as Me;
    expect(chosen.device).toMatchObject({ platform: "android", platform_source: "chosen", detected_platform: "ios" });
    expect(chosen.features).toEqual(["android_apps", "web_push", "android_app_offer"]);
    expect(request("GET", "/api/apps").json).toEqual([]);
    expect((request("PATCH", "/api/me", { platform: null }).json as Me).device.platform_source).toBe("detected");
    expect(request("PATCH", "/api/me", { platform: "windows" }).status).toBe(400);
    expect((request("PATCH", "/api/me", { onboarded: false }).json as Me).onboarded).toBe(false);
    expect(request("PUT", "/api/me/packages", { packages: [] }).status).toBe(400);
  });

  it("answers as a desktop browser that has not been set up, on Linux", () => {
    const now = Date.parse("2026-10-29T12:00:00Z");
    const api = createMockApi(() => now, { device: "desktop", onboarded: false, host: "linux", androidApp: false });
    const request = (method: string, path: string, body?: unknown) =>
      api.handle({
        method, path, query: new URLSearchParams(),
        headers: method === "GET" ? {} : { "x-kiwi": "1", "content-type": "application/json" },
        body: body === undefined ? "" : JSON.stringify(body),
        origin: "http://localhost:5173",
      });
    const me = request("GET", "/api/me").json as Me;
    expect(me.device).toMatchObject({ kind: "browser", platform: "ios", platform_source: "assumed", detected_platform: null });
    expect(me).toMatchObject({ onboarded: false, android_app: null, host: { os: "linux", ios_signing: "files", devicectl: false } });
    expect(me.features).toEqual(["ios_apps", "home_screen_app", "web_push"]);
    expect(me.counts.installed).toBe(0);
    expect(request("GET", "/android/kiwi.apk").status).toBe(404);
    const android = request("PATCH", "/api/me", { platform: "android", onboarded: true }).json as Me;
    expect(android).toMatchObject({ onboarded: true, device: { platform: "android", platform_source: "chosen" } });
    expect(android.features).toEqual(["android_apps", "web_push"]);
  });

  it("marks Android apps installed with another signature", () => {
    let now = Date.parse("2026-10-29T12:00:00Z");
    const api = createMockApi(() => now, { builds: "simulated", device: "android", foreign: ["app.morphe.android.youtube"] });
    const request = (method: string, path: string, body?: unknown) =>
      api.handle({
        method, path, query: new URLSearchParams(),
        headers: method === "GET" ? {} : { "x-kiwi": "1", "content-type": "application/json" },
        body: body === undefined ? "" : JSON.stringify(body),
        origin: "http://localhost:5173",
      });
    const recipe = request("GET", "/api/automation/android-youtube").json as AutomationState;
    request("POST", "/api/automation/android-youtube/add", recipe.preferences);
    now += 5 * 2600;
    const path = "/api/apps/kiwi-youtube/app.morphe.android.youtube";
    expect((request("GET", path).json as AppDetail).state.foreign_install).toBe(true);
    const report = (signer: string) => ({
      packages: [{ package: "app.morphe.android.youtube", version_name: "21.30.216", version_code: 1, signer_sha256: [signer], installer: null }],
    });
    expect(request("PUT", "/api/me/packages", report(MOCK_ANDROID_SIGNER)).status).toBe(204);
    expect((request("GET", path).json as AppDetail).state.foreign_install).toBe(false);
    request("PUT", "/api/me/packages", report("d".repeat(64)));
    expect((request("GET", path).json as AppDetail).state.foreign_install).toBe(true);
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

describe("mock signing", () => {
  function signingSetup(signing: "keychain" | "files", device: "ios" | "android" = "ios") {
    const api = createMockApi(() => Date.parse("2026-10-29T12:00:00Z"), { signing, device });
    return (method: string, path: string, body?: unknown) =>
      api.handle({
        method,
        path,
        query: new URLSearchParams(),
        headers: method === "GET" ? {} : { "x-kiwi": "1", "content-type": "application/json" },
        body: body === undefined ? "" : JSON.stringify(body),
        origin: "http://localhost:5173",
      });
  }

  it("signs with the keychain on a Mac and refuses identity uploads", () => {
    const request = signingSetup("keychain");
    const signing = request("GET", "/api/signing").json as Signing;
    expect(signing.backend).toBe("keychain");
    expect(signing.profiles.every((p) => !p.uploaded)).toBe(true);
    expect(request("POST", "/api/signing/identity", { p12: "AAAA", password: "x" }).status).toBe(409);
    const uploaded = request("POST", "/api/signing/profiles", { profile: "AAAA" }).json as Signing;
    expect(uploaded.profiles[0]?.uploaded).toBe(true);
    expect(request("DELETE", `/api/signing/profiles/${signing.profiles[0]?.uuid}`).status).toBe(404);
  });

  it("uploads and removes the identity and profiles on Linux", () => {
    const request = signingSetup("files");
    expect((request("GET", "/api/signing").json as Signing).backend).toBe("files");
    const removed = request("DELETE", "/api/signing/identity").json as Signing;
    expect(removed.identities).toEqual([]);
    expect(removed.active).toBeNull();
    expect(removed.error).toMatch(/No signing identity was uploaded/);
    expect(request("DELETE", "/api/signing/identity").status).toBe(404);
    expect(request("POST", "/api/signing/identity", { p12: "AAAA", password: "wrong" }).status).toBe(400);
    expect(request("POST", "/api/signing/identity", { p12: "not base64!", password: "" }).status).toBe(400);
    const added = request("POST", "/api/signing/identity", { p12: "AAAA", password: "secret" }).json as Signing;
    expect(added.identities).toHaveLength(1);
    expect(JSON.stringify(added)).not.toContain("secret");
    const profiles = (request("GET", "/api/signing").json as Signing).profiles;
    for (const p of profiles) request("DELETE", `/api/signing/profiles/${p.uuid}`);
    const empty = request("GET", "/api/signing").json as Signing;
    expect(empty.profiles).toEqual([]);
    expect(empty.error).toMatch(/wildcard provisioning profile/);
    const uploaded = request("POST", "/api/signing/profiles", { profile: "AAAA" }).json as Signing;
    expect(uploaded.profiles[0]?.uploaded).toBe(true);
    expect(uploaded.active?.profile_uuid).toBe(uploaded.profiles[0]?.uuid);
    expect(uploaded.error).toBeNull();
  });

  it("warns about signing problems like the server, except on Android", () => {
    const signingWarnings = (request: ReturnType<typeof signingSetup>) =>
      (request("GET", "/api/me").json as Me).warnings.filter((w) => w.link === "/settings").map((w) => [w.level, w.code]);
    const request = signingSetup("files");
    expect(signingWarnings(request)).toEqual([]);
    for (const p of (request("GET", "/api/signing").json as Signing).profiles) request("DELETE", `/api/signing/profiles/${p.uuid}`);
    expect(signingWarnings(request)).toEqual([["error", "no_profile"]]);
    request("DELETE", "/api/signing/identity");
    expect(signingWarnings(request)).toEqual([["error", "no_identity"]]);
    const android = signingSetup("files", "android");
    android("DELETE", "/api/signing/identity");
    expect(signingWarnings(android)).toEqual([]);
  });
});

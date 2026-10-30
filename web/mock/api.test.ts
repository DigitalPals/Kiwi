import { describe, expect, it } from "vitest";
import type {
  AppDetail,
  AppleAccount,
  AppSummary,
  AutomationOverview,
  AutomationState,
  Build,
  Family,
  FamilyDevice,
  Me,
  PersonDetail,
  Settings,
  Source,
  StoreApp,
} from "../src/api/types.ts";
import { compareVersions, createMockApi, shortName } from "./api.ts";

function setup() {
  let now = Date.parse("2026-10-29T12:00:00Z");
  const api = createMockApi(() => now);
  /** `as`: the viewer (family member) asking; the owner by default. */
  const request = (method: string, path: string, body?: unknown, as?: string) => {
    const url = new URL(path, "http://localhost:5173");
    const cookie = as ? { cookie: `kiwi_mock_as=${as}` } : {};
    return api.handle({
      method,
      path: url.pathname,
      query: url.searchParams,
      headers: method === "GET" ? cookie : { ...cookie, "x-kiwi": "1", "content-type": "application/json" },
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
    expect(overview.apps.map((a) => a.id)).toEqual(["youtube", "youtube-music", "twitch", "x", "reddit"]);
    expect(overview.apps.map((a) => a.added)).toEqual([true, false, false, false, false]);
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

describe("mock family", () => {
  it("shows members only their apps and keeps the owner's pages to the owner", () => {
    const { request } = setup();
    const me = request("GET", "/api/me", undefined, "emma").json as Me;
    expect(me.access).toEqual({ role: "member", person: { id: "emma@example.com", name: "Emma" }, owner_name: "Alex" });
    expect(me.registration.ready).toBe(true);
    expect(me.counts.sources).toBe(0);
    const apps = request("GET", "/api/apps", undefined, "emma").json as AppSummary[];
    expect(apps.map((a) => a.name)).toEqual(["Delta", "EeveeSpotify", "YouTubePlus", "YT Music Ultimate"]);
    expect(apps.every((a) => !a.hidden && !a.is_new)).toBe(true);
    for (const path of ["/api/sources", "/api/settings", "/api/family", "/api/automation"]) {
      const r = request("GET", path, undefined, "emma");
      expect(r.status, path).toBe(403);
      expect((r.json as { error: { code: string; message: string } }).error).toEqual({ code: "forbidden", message: "Only Alex can do this." });
    }
    expect(request("GET", "/api/apps/ytlite/com.atebits.Tweetie2", undefined, "emma").status).toBe(404);
    const library = request("GET", "/api/library", undefined, "emma").json as AppSummary[];
    expect(library.find((a) => a.name === "BHTwitter")?.orphaned).toBe(true);
    expect(library.find((a) => a.name === "YouTubePlus")?.orphaned).toBe(false);
    // The owner's own view is unchanged, with who can install each app.
    expect((request("GET", "/api/me").json as Me).access.role).toBe("owner");
    const detail = request("GET", "/api/apps/ytlite/com.google.ios.youtube").json as AppDetail;
    expect(detail.access?.map((a) => [a.name, a.allowed, a.installed_version])).toEqual([
      ["Alex Morgan", true, "21.43.5"],
      ["Emma", true, "21.43.5"],
      ["Kim de Vries", false, null],
      ["Noah", false, null],
    ]);
    expect((request("GET", "/api/apps/ytlite/com.google.ios.youtube", undefined, "emma").json as AppDetail).access).toBeNull();
  });

  it("registers a new iPhone: enroll, approve, add to a profile, pick apps", () => {
    const { request } = setup();
    const kim = (method: string, path: string, body?: unknown) => request(method, path, body, "kim");
    expect((kim("GET", "/api/me").json as Me).registration.status).toBe("none");
    expect(kim("GET", "/api/apps").json).toEqual([]);
    const prepare = kim("POST", "/api/apps/ytlite/com.google.ios.youtube/prepare", {});
    expect(prepare.status).toBe(403);
    expect((prepare.json as { error: { code: string } }).error.code).toBe("not_registered");

    const { url } = kim("POST", "/api/me/enroll", {}).json as { url: string };
    expect(url).toMatch(/^\/enroll\/e-[\w-]+\/kiwi\.mobileconfig$/);
    const enrolled = kim("GET", url);
    expect(enrolled.status).toBe(302);
    expect(enrolled.headers?.Location).toBe("/?enrolled=1");
    expect(kim("GET", url).status).toBe(404);
    const me = kim("GET", "/api/me").json as Me;
    expect(me.registration.status).toBe("pending");
    expect(me.device.name).toBe("Kim's iPhone");

    const family = request("GET", "/api/family").json as Family;
    expect(family.pending).toBe(2);
    expect(family.people.map((p) => p.name)).toEqual(["Alex Morgan", "Emma", "Kim de Vries", "Noah"]);
    const approved = request("POST", "/api/family/devices/ts%3AnKIM1CNTRL/approve", {}).json as FamilyDevice;
    expect(approved.registration).toMatchObject({ status: "approved", in_profile: false });
    expect((kim("GET", "/api/me").json as Me).registration.ready).toBe(false);
    // Added to a profile by hand, then a rescan.
    request("POST", "/api/signing/rescan", {});
    expect((kim("GET", "/api/me").json as Me).registration.ready).toBe(true);

    const person = request("PATCH", "/api/family/people/kim%40example.com/apps", {
      apps: [{ source_id: "altstore", bundle_id: "com.rileytestut.Delta", allowed: true }],
    }).json as PersonDetail;
    expect(person.app_count).toBe(1);
    expect(person.apps.find((a) => a.name === "Delta")).toMatchObject({ allowed: true, explicit: true });
    expect((kim("GET", "/api/apps").json as AppSummary[]).map((a) => a.name)).toEqual(["Delta"]);
    expect(request("PATCH", "/api/family/people/alex%40example.com/apps", { apps: [] }).status).toBe(400);

    // Removing Kim: the iPhone has to register again.
    expect(request("DELETE", "/api/family/people/kim%40example.com").status).toBe(204);
    expect((kim("GET", "/api/me").json as Me).registration.status).toBe("none");
  });

  it("registers approved iPhones with App Store Connect after a moment", () => {
    const { request, advance } = setup();
    const bad = request("PUT", "/api/family/apple", { issuer_id: "x", key_id: "y", private_key: "z" });
    expect(bad.status).toBe(400);
    const key = { issuer_id: "57246542-96fe-1a63-e053-0824d011072a", key_id: "2x9r4hxf34", private_key: "-----BEGIN PRIVATE KEY-----" };
    const apple = request("PUT", "/api/family/apple", key).json as AppleAccount;
    expect(apple).toMatchObject({ connected: true, key_id: "2X9R4HXF34", devices: 3 });
    request("POST", "/api/family/devices/ts%3AnNOAH1CNTRL/approve", {});
    expect((request("GET", "/api/me", undefined, "noah").json as Me).registration.ready).toBe(false);
    advance(5000);
    expect((request("GET", "/api/me", undefined, "noah").json as Me).registration.ready).toBe(true);
    const after = request("GET", "/api/family/apple").json as AppleAccount;
    expect(after.profile).toMatchObject({ kiwi: true, devices: 4 });
    expect(after.profile?.name).toMatch(/^Kiwi \* /);
    expect((request("DELETE", "/api/family/apple").json as AppleAccount).connected).toBe(false);
  });

  it("installs on the member's own iPhone", () => {
    const { request, advance } = setup();
    const emma = (method: string, path: string, body?: unknown) => request(method, path, body, "emma");
    const build = emma("POST", "/api/apps/altstore/com.rileytestut.Delta/prepare", {}).json as Build;
    advance(30_000);
    const ready = emma("GET", `/api/builds/${build.id}`).json as Build;
    expect(ready.status).toBe("ready");
    request("GET", `/ota/${new URL(decodeURIComponent(ready.install_url?.split("url=")[1] ?? "")).pathname.split("/")[2]}/manifest.plist`, undefined, "emma");
    advance(10_000);
    const library = emma("GET", "/api/library").json as AppSummary[];
    expect(library.some((a) => a.name === "Delta")).toBe(true);
    const owner = request("GET", "/api/library").json as AppSummary[];
    expect(owner.some((a) => a.name === "Delta")).toBe(false);
  });
});

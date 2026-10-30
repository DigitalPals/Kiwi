import { describe, expect, it } from "vitest";
import {
  APPLE_PATH,
  BUILDS_PATH,
  DEVICE_PATH,
  FAMILY_PATH,
  NOTIFICATIONS_PATH,
  SIGNING_PATH,
  appPath,
  buildPath,
  familyDevicePath,
  interceptedPath,
  matchRoute,
  ownerOnly,
  personPath,
  routeTab,
  sourcePath,
  tabsFor,
} from "./routes";

describe("matchRoute", () => {
  it("matches the tab routes, with or without a trailing slash", () => {
    expect(matchRoute("/")).toEqual({ name: "browse" });
    expect(matchRoute("")).toEqual({ name: "browse" });
    expect(matchRoute("/apps")).toEqual({ name: "apps" });
    expect(matchRoute("/sources")).toEqual({ name: "sources" });
    expect(matchRoute("/settings")).toEqual({ name: "settings" });
  });

  it("sends the old Updates and Library paths to My Apps", () => {
    expect(matchRoute("/updates")).toEqual({ name: "apps" });
    expect(matchRoute("/library/")).toEqual({ name: "apps" });
  });

  it("opens Kiwi Builds and each app's build page under Sources, and older links", () => {
    expect(matchRoute(BUILDS_PATH)).toEqual({ name: "builds" });
    expect(matchRoute("/settings/builds")).toEqual({ name: "builds" });
    expect(matchRoute("/automation")).toEqual({ name: "builds" });
    expect(matchRoute(buildPath("youtube"))).toEqual({ name: "build", app: "youtube" });
    expect(buildPath("youtube-music")).toBe("/builds/youtube-music");
    expect(matchRoute("/settings/builds/youtube")).toEqual({ name: "build", app: "youtube" });
    expect(matchRoute("/settings/builds/youtube-music/")).toEqual({ name: "build", app: "youtube-music" });
    expect(routeTab({ name: "builds" })).toBe("sources");
    expect(routeTab({ name: "build", app: "x" })).toBe("sources");
    expect(ownerOnly({ name: "builds" })).toBe(true);
  });

  it("opens the Settings pages; only Signing is the owner's", () => {
    expect(matchRoute(SIGNING_PATH)).toEqual({ name: "signing" });
    expect(matchRoute(`${DEVICE_PATH}/`)).toEqual({ name: "device" });
    expect(matchRoute(NOTIFICATIONS_PATH)).toEqual({ name: "notifications" });
    for (const name of ["signing", "device", "notifications"] as const) expect(routeTab({ name })).toBe("settings");
    expect(ownerOnly({ name: "signing" })).toBe(true);
    expect(ownerOnly({ name: "device" })).toBe(false);
    expect(ownerOnly({ name: "notifications" })).toBe(false);
  });

  it("decodes app routes; bundle ids with dots are one segment", () => {
    expect(matchRoute("/app/ytlite/com.google.ios.youtube")).toEqual({
      name: "app",
      sourceId: "ytlite",
      bundleId: "com.google.ios.youtube",
    });
    expect(matchRoute("/app/a%2Fb/com.example%20x")).toEqual({ name: "app", sourceId: "a/b", bundleId: "com.example x" });
  });

  it("round-trips appPath", () => {
    const path = appPath("src/1", "com.example.app");
    expect(path).toBe("/app/src%2F1/com.example.app");
    expect(matchRoute(path)).toEqual({ name: "app", sourceId: "src/1", bundleId: "com.example.app" });
  });

  it("decodes source routes", () => {
    expect(matchRoute("/sources/abc123/")).toEqual({ name: "source", sourceId: "abc123" });
    expect(matchRoute(sourcePath("a/b"))).toEqual({ name: "source", sourceId: "a/b" });
    expect(matchRoute("/sources/a/b")).toMatchObject({ name: "not-found" });
  });

  it("opens the family pages under Settings, including device links from notifications", () => {
    expect(matchRoute(FAMILY_PATH)).toEqual({ name: "family" });
    expect(matchRoute(`${FAMILY_PATH}/`)).toEqual({ name: "family" });
    expect(matchRoute(APPLE_PATH)).toEqual({ name: "apple" });
    expect(matchRoute(personPath("noah@example.com"))).toEqual({ name: "person", personId: "noah@example.com" });
    expect(personPath("noah@example.com")).toBe("/settings/family/noah%40example.com");
    expect(matchRoute("/settings/family/devices/ts:nNOAH1CNTRL")).toEqual({ name: "family-device", deviceId: "ts:nNOAH1CNTRL" });
    expect(matchRoute(familyDevicePath("ts:a/b"))).toEqual({ name: "family-device", deviceId: "ts:a/b" });
    expect(matchRoute("/settings/family/devices")).toMatchObject({ name: "not-found" });
    for (const route of [{ name: "family" }, { name: "apple" }, { name: "person", personId: "x" }, { name: "family-device", deviceId: "y" }] as const) {
      expect(routeTab(route)).toBe("settings");
      expect(ownerOnly(route)).toBe(true);
    }
    expect(ownerOnly({ name: "browse" })).toBe(false);
    expect(ownerOnly({ name: "app", sourceId: "s", bundleId: "b" })).toBe(false);
  });

  it("leaves out the Sources tab for members", () => {
    expect(tabsFor(false).map((t) => t.tab)).toEqual(["browse", "apps", "sources", "settings"]);
    expect(tabsFor(true).map((t) => t.tab)).toEqual(["browse", "apps", "settings"]);
  });

  it("returns not-found for unknown or malformed paths", () => {
    expect(matchRoute("/nope")).toEqual({ name: "not-found", path: "/nope" });
    expect(matchRoute("/app/only-source")).toMatchObject({ name: "not-found" });
    expect(matchRoute("/app/a/b/c")).toMatchObject({ name: "not-found" });
    expect(matchRoute("/app/%E0%A4%A/x")).toMatchObject({ name: "not-found" });
  });

  it("maps routes to tabs", () => {
    expect(routeTab({ name: "apps" })).toBe("apps");
    expect(routeTab({ name: "app", sourceId: "s", bundleId: "b" })).toBeNull();
    expect(routeTab({ name: "source", sourceId: "s" })).toBe("sources");
  });
});

describe("interceptedPath", () => {
  const origin = "https://mac-studio.example.ts.net";
  const click = { button: 0, metaKey: false, ctrlKey: false, shiftKey: false, altKey: false, defaultPrevented: false };
  const link = (href: string, extra: Partial<{ target: string; hasDownload: boolean }> = {}) => ({
    href,
    target: "",
    hasDownload: false,
    ...extra,
  });

  it("handles same-origin app links", () => {
    expect(interceptedPath(click, link(`${origin}/updates`), origin)).toBe("/updates");
    expect(interceptedPath(click, link(`${origin}/app/s/b?x=1#y`), origin)).toBe("/app/s/b?x=1#y");
  });

  it("leaves install, open, external and API links to the browser", () => {
    expect(interceptedPath(click, link("itms-services://?action=download-manifest&url=x"), origin)).toBeNull();
    expect(interceptedPath(click, link("kiwi-0123456789ab://"), origin)).toBeNull();
    expect(interceptedPath(click, link("https://github.com/x"), origin)).toBeNull();
    expect(interceptedPath(click, link(`${origin}/api/me`), origin)).toBeNull();
    expect(interceptedPath(click, link(`${origin}/ota/t/manifest.plist`), origin)).toBeNull();
    expect(interceptedPath(click, link(`${origin}/enroll/t/kiwi.mobileconfig`), origin)).toBeNull();
  });

  it("respects modifiers, targets and downloads", () => {
    expect(interceptedPath({ ...click, metaKey: true }, link(`${origin}/`), origin)).toBeNull();
    expect(interceptedPath({ ...click, button: 1 }, link(`${origin}/`), origin)).toBeNull();
    expect(interceptedPath({ ...click, defaultPrevented: true }, link(`${origin}/`), origin)).toBeNull();
    expect(interceptedPath(click, link(`${origin}/`, { target: "_blank" }), origin)).toBeNull();
    expect(interceptedPath(click, link(`${origin}/x`, { hasDownload: true }), origin)).toBeNull();
    expect(interceptedPath(click, link(`${origin}/`, { target: "_self" }), origin)).toBe("/");
  });
});

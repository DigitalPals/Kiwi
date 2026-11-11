import { describe, expect, it } from "vitest";
import type { AutomationJob, AutomationState, Builder } from "../api/types";
import {
  appsForDevice, automationSummary, buildsIos, buildsNotice, heroLine, intervalLabel, overviewSummary, samePreferences, selectedFeatures, statusLine,
} from "./automation";

const job = (over: Partial<AutomationJob> = {}): AutomationJob => ({
  id: "job", app: "youtube", sequence: 5, status: "running", stage: "Compiling patches", version: "21.43.5",
  external_id: null, recipe_key: null, signed_build_id: null, error: null, created_at: "2026-10-29", finished_at: null,
  ...over,
});

const state = (over: Partial<AutomationState> = {}): AutomationState => ({
  id: "youtube", platform: "ios", name: "YouTube", bundle_id: "com.google.ios.youtube", icon_url: null, tweak: "YouMod",
  summary: "No ads", experimental: false, warning: null, names: [], app_version: null, limitations: [],
  included: [{ id: "youmod", name: "YouMod", description: "" }],
  available_tweaks: [{ id: "ryd", name: "Return YouTube Dislikes", description: "" }, { id: "youpip", name: "Picture in Picture", description: "" }],
  added: true,
  preferences: { enabled: true, interval_minutes: 60, tweaks: ["ryd", "youpip"] },
  jobs: [], checks: [], sequence: 4, last_checked_at: null, last_success_key: "recipe",
  failed_recipe: null, failed_attempts: 0, configuration_changed: false, next_check_at: null, source_enabled: true,
  published: { version: "21.43.5", build: "21.43.5", revision: 4, date: null, tweaks: ["youpip", "ryd"], name: null },
  ...over,
});

describe("Kiwi Builds settings", () => {
  it("treats reordered patch selections as saved while detecting schedule, patch and name edits", () => {
    const original = state().preferences;
    expect(samePreferences(original, { ...original, tweaks: ["youpip", "ryd"] })).toBe(true);
    expect(samePreferences(original, { ...original, tweaks: ["youpip"] })).toBe(false);
    expect(samePreferences(original, { ...original, enabled: false })).toBe(false);
    expect(samePreferences(original, { ...original, interval_minutes: 150 })).toBe(false);
    expect(samePreferences({ ...original, name: "X" }, { ...original, name: "Twitter" })).toBe(false);
    expect(original.tweaks).toEqual(["ryd", "youpip"]);
  });

  it("keeps a running manual job visible when automatic builds are off", () => {
    expect(automationSummary(state({ preferences: { ...state().preferences, enabled: false }, jobs: [job()] })))
      .toBe("Compiling patches");
    expect(automationSummary(state({ configuration_changed: true }))).toBe("Saved changes ready to build");
    expect(automationSummary(state({ preferences: { ...state().preferences, enabled: false } }))).toBe("Automatic builds off");
    expect(automationSummary(state({ published: null }))).toBe("Ready for your first build");
  });

  it("summarises each app in one line and all apps for Settings", () => {
    expect(statusLine(state())).toBe("21.43.5 · Kiwi 4 · up to date");
    expect(statusLine(state({ jobs: [job({ status: "queued", stage: "Waiting for YouTube" })] }))).toBe("Waiting for YouTube");
    expect(statusLine(state({ jobs: [job({ status: "failed", finished_at: "2026-10-29" })] }))).toBe("21.43.5 · Kiwi 4 · needs attention");
    const music = state({ id: "youtube-music", name: "YouTube Music", jobs: [job({ app: "youtube-music", stage: "Decrypting YouTube Music" })] });
    const twitch = state({ id: "twitch", name: "Twitch" });
    const x = state({ id: "x", name: "X", added: false });
    expect(overviewSummary({ apps: [state(), music, twitch, x], checks: [] })).toBe("YouTube Music: Decrypting YouTube Music");
    expect(overviewSummary({ apps: [state(), twitch, x], checks: [] })).toBe("YouTube and Twitch");
    expect(overviewSummary({ apps: [state(), state({ id: "a" }), twitch], checks: [] })).toBe("3 apps");
    expect(overviewSummary({ apps: [x], checks: [] })).toBe("Patch and sign App Store apps on your Mac");
  });

  it("names the tweak and the app version it is built on", () => {
    expect(heroLine(state())).toBe("YouMod · YouTube 21.43.5");
    expect(heroLine(state({ name: "X", tweak: "NeoFreeBird 7.3.0", app_version: "12.38.1", published: null }))).toBe("NeoFreeBird 7.3.0 · X 12.38.1");
    expect(heroLine(state({ published: null }))).toBe("YouMod");
  });

  it("lists included features, chosen tweaks and the chosen name", () => {
    expect(selectedFeatures(state(), ["youpip"])).toBe("YouMod · Picture in Picture");
    expect(selectedFeatures(state({ names: ["Twitter", "X"] }), [], "Twitter")).toBe("YouMod · Named Twitter");
  });

  it("labels hourly, daily and custom intervals", () => {
    expect(intervalLabel(60)).toBe("Every hour");
    expect(intervalLabel(1440)).toBe("Every day");
    expect(intervalLabel(180)).toBe("Every 3 hours");
    expect(intervalLabel(150)).toBe("Every 150 minutes");
  });
});

describe("appsForDevice", () => {
  it("lists Android recipes on Android and the iPhone's elsewhere", () => {
    const apps = [state(), state({ id: "android-youtube", platform: "android" }), state({ id: "x" })];
    expect(appsForDevice(apps, true).map((a) => a.id)).toEqual(["android-youtube"]);
    expect(appsForDevice(apps, false).map((a) => a.id)).toEqual(["youtube", "x"]);
  });
});

describe("buildsNotice", () => {
  const mac = { os: "macos" } as const;
  const linux = { os: "linux" } as const;

  it("names where the apps come from", () => {
    expect(buildsNotice(false, mac)).toContain("Apple account");
    expect(buildsNotice(false, mac)).toContain("one-time setup");
    expect(buildsNotice(true, mac)).toContain("Google Play");
    expect(buildsNotice(true, mac)).not.toContain("Mac");
    expect(buildsNotice(true, linux)).toBe(buildsNotice(true, mac));
  });

  it("tells an iPhone plainly that a Kiwi on Linux without a Mac builder builds Android apps only", () => {
    const notice = buildsNotice(false, linux);
    expect(notice).toContain("Google Play");
    expect(notice).toContain("iOS apps need a Mac builder");
    expect(notice).toContain("Settings › Mac Builder");
    expect(notice).not.toContain("this Mac");
    expect(notice).not.toContain("Apple account");
    expect(buildsNotice(false, { os: "other" })).toBe(notice);
    expect(buildsNotice(false, linux, { builder: null })).toBe(notice);
  });

  it("names the paired Mac builder on a Kiwi on Linux", () => {
    const notice = buildsNotice(false, linux, { builder: { name: "Mac Studio" } as Builder });
    expect(notice).toContain("Apple account");
    expect(notice).toContain("decrypt them on Mac Studio");
    expect(notice).not.toContain("Google Play");
    expect(buildsNotice(true, linux, { builder: { name: "Mac Studio" } as Builder })).toBe(buildsNotice(true, mac));
  });
});

describe("buildsIos", () => {
  it("builds iOS apps on a Mac, or elsewhere with a paired Mac builder", () => {
    expect(buildsIos({ os: "macos" })).toBe(true);
    expect(buildsIos({ os: "linux" })).toBe(false);
    expect(buildsIos({ os: "other" })).toBe(false);
    expect(buildsIos({ os: "linux" }, { builder: null })).toBe(false);
    expect(buildsIos({ os: "linux" }, { builder: { name: "Mac Studio" } as Builder })).toBe(true);
  });
});

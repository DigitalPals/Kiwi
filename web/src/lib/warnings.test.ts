import { describe, expect, it } from "vitest";
import type { Me, Warning } from "../api/types";
import { blockingWarnings, listedSettingsWarnings, settingsWarnings, tabDots } from "./warnings";

const warning = (level: Warning["level"], code: string, link: string): Warning => ({ level, code, message: code, link });

function me(warnings: Warning[], sourceErrors = 0, features: Me["features"] = ["ios_apps"]): Me {
  return {
    features,
    warnings,
    counts: { updates: 0, installed: 0, sources: 3, source_errors: sourceErrors, preparing: 0 },
  } as Me;
}

describe("tabDots", () => {
  it("marks Sources when a source failed and Settings for signing problems", () => {
    expect(tabDots(null)).toEqual({});
    expect(tabDots(me([]))).toEqual({});
    expect(tabDots(me([warning("warning", "source_errors", "/sources")], 1))).toEqual({ sources: "warn" });
    expect(tabDots(me([warning("warning", "expiring", "/settings")]))).toEqual({ settings: "warn" });
    expect(tabDots(me([warning("warning", "expiring", "/settings"), warning("error", "no_profile", "/settings")]))).toEqual({
      settings: "error",
    });
  });

  it("leaves failed sources out on Android, which has no Sources tab", () => {
    expect(tabDots(me([warning("warning", "source_errors", "/sources")], 1, ["android_apps", "web_push"]))).toEqual({});
  });

  it("leaves expiring apps to the My Apps badge and info to Settings itself", () => {
    expect(tabDots(me([warning("warning", "expiring", "/apps"), warning("info", "unknown_os", "/settings")]))).toEqual({});
  });
});

describe("settingsWarnings and blockingWarnings", () => {
  it("picks the warnings Settings and Browse show", () => {
    const list = [
      warning("error", "no_identity", "/settings"),
      warning("warning", "source_errors", "/sources"),
      warning("info", "unknown_os", "/settings"),
    ];
    expect(settingsWarnings(me(list)).map((w) => w.code)).toEqual(["no_identity", "unknown_os"]);
    expect(blockingWarnings(me(list)).map((w) => w.code)).toEqual(["no_identity"]);
  });
});

describe("listedSettingsWarnings", () => {
  const androidApp = warning("warning", "android_app", "/settings");

  it("lists Android's Settings warnings, which have no row of their own", () => {
    const android = me([androidApp, warning("warning", "source_errors", "/sources")], 1, ["android_apps", "web_push"]);
    expect(tabDots(android)).toEqual({ settings: "warn" });
    expect(listedSettingsWarnings(android)).toEqual([androidApp]);
  });

  it("leaves an iPhone's signing and iOS version to their own rows, and lists anything else", () => {
    const signing = [
      warning("error", "no_identity", "/settings"),
      warning("error", "no_profile", "/settings"),
      warning("warning", "expiring", "/settings"),
      warning("info", "unknown_os", "/settings"),
    ];
    expect(listedSettingsWarnings(me(signing))).toEqual([]);
    const other = warning("warning", "something_new", "/settings");
    expect(listedSettingsWarnings(me([...signing, other]))).toEqual([other]);
  });

  it("gives every dot on Settings a reason in Settings", () => {
    const cases: Me[] = [
      me([androidApp], 0, ["android_apps"]),
      me([warning("warning", "expiring", "/settings")]),
      me([warning("error", "no_profile", "/settings")]),
      me([warning("warning", "something_new", "/settings")]),
    ];
    for (const m of cases) {
      expect(tabDots(m).settings).toBeDefined();
      const ownRow = m.features.includes("ios_apps") && m.warnings.some((w) => ["no_identity", "no_profile", "expiring"].includes(w.code));
      expect(ownRow || listedSettingsWarnings(m).length > 0).toBe(true);
    }
  });
});

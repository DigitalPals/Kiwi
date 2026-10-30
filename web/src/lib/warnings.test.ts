import { describe, expect, it } from "vitest";
import type { Me, Warning } from "../api/types";
import { blockingWarnings, settingsWarnings, tabDots } from "./warnings";

const warning = (level: Warning["level"], code: string, link: string): Warning => ({ level, code, message: code, link });

function me(warnings: Warning[], sourceErrors = 0): Me {
  return {
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

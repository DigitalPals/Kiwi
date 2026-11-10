import { describe, expect, it } from "vitest";
import { hasFeature } from "./features";

describe("hasFeature", () => {
  it("reads the features the server reported for this device", () => {
    const me = { features: ["android_apps", "web_push"] as const };
    const iphone = { features: ["ios_apps", "home_screen_app", "web_push", "devicectl"] as const };
    expect(hasFeature({ features: [...me.features] }, "android_apps")).toBe(true);
    expect(hasFeature({ features: [...me.features] }, "ios_apps")).toBe(false);
    expect(hasFeature({ features: [...iphone.features] }, "devicectl")).toBe(true);
  });

  it("shows nothing until /api/me answers", () => {
    expect(hasFeature(null, "ios_apps")).toBe(false);
    expect(hasFeature(undefined, "web_push")).toBe(false);
  });
});

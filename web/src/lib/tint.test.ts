import { describe, expect, it } from "vitest";
import type { Installed } from "../api/types";
import { appColor, contrastWithWhite, heroColors, parseTint, signatureLeft, vividTint } from "./tint";

describe("parseTint", () => {
  it("accepts 6-digit hex colours with or without #", () => {
    expect(parseTint("#FF0033")).toBe("#ff0033");
    expect(parseTint("8A28F7")).toBe("#8a28f7");
  });

  it("rejects anything else", () => {
    expect(parseTint(null)).toBeNull();
    expect(parseTint("")).toBeNull();
    expect(parseTint("#fff")).toBeNull();
    expect(parseTint("red")).toBeNull();
  });
});

describe("appColor", () => {
  it("prefers the tint", () => {
    expect(appColor("Delta", "#8A28F7")).toBe("#8a28f7");
  });

  it("picks the same palette colour for the same name", () => {
    const color = appColor("Nightly Builds", null);
    expect(color).toMatch(/^#[0-9a-f]{6}$/);
    expect(appColor("Nightly Builds", "nope")).toBe(color);
  });
});

describe("heroColors", () => {
  it("keeps tints that already carry white text", () => {
    expect(heroColors("#8a28f7").base).toBe("#8a28f7");
  });

  it("darkens light tints until white text reads at 4.5:1", () => {
    for (const tint of ["#ff0033", "#1db954", "#ffffff", "#ffd60a"]) {
      const { base } = heroColors(tint);
      expect(contrastWithWhite(base)).toBeGreaterThanOrEqual(4.5);
    }
  });

  it("returns a deeper shade for the gradient", () => {
    const { base, deep } = heroColors("#2b7bff");
    expect(contrastWithWhite(deep)).toBeGreaterThan(contrastWithWhite(base));
  });
});

describe("vividTint", () => {
  it("keeps colourful tints", () => {
    expect(vividTint("#1DB954")).toBe("#1db954");
    expect(vividTint("#FF0033")).toBe("#ff0033");
  });

  it("drops greys, near-white and near-black", () => {
    expect(vividTint("#8E8E93")).toBeNull();
    expect(vividTint("#3B3B3B")).toBeNull();
    expect(vividTint("#FAFAFA")).toBeNull();
    expect(vividTint("#050010")).toBeNull();
    expect(vividTint(null)).toBeNull();
  });
});

describe("signatureLeft", () => {
  const now = new Date("2026-10-30T12:00:00Z");
  const install = (installed_at: string, expires_at: string | null, origin: Installed["origin"] = "kiwi"): Installed => ({
    version: "1.0",
    build: null,
    signed_bundle_id: "com.example.app",
    origin,
    verified: true,
    installed_at,
    expires_at,
  });

  it("is the share of the signing period that is left", () => {
    expect(signatureLeft(install("2026-10-20T12:00:00Z", "2026-11-09T12:00:00Z"), now)).toBeCloseTo(0.5);
    expect(signatureLeft(install("2026-10-30T12:00:00Z", "2027-10-30T12:00:00Z"), now)).toBe(1);
  });

  it("is 0 once expired", () => {
    expect(signatureLeft(install("2025-10-01T00:00:00Z", "2026-10-01T00:00:00Z"), now)).toBe(0);
  });

  it("assumes a year when the install time doesn't fit", () => {
    expect(signatureLeft(install("2027-01-01T00:00:00Z", "2026-11-04T12:00:00Z"), now)).toBeCloseTo(5 / 365);
  });

  it("is null without an expiry or for apps installed outside Kiwi", () => {
    expect(signatureLeft(null, now)).toBeNull();
    expect(signatureLeft(install("2026-10-20T12:00:00Z", null), now)).toBeNull();
    expect(signatureLeft(install("2026-10-20T12:00:00Z", "2027-10-20T12:00:00Z", "device"), now)).toBeNull();
  });
});

import { describe, expect, it } from "vitest";
import {
  expiryInfo,
  formatBytes,
  formatDate,
  formatPercent,
  formatRelative,
  formatVersion,
  deviceName,
  osRequirement,
  parseDate,
  serverLabel,
  formatRemaining,
  splitBytes,
  plural,
  privacyLabel,
} from "./format";

describe("formatBytes", () => {
  it("uses decimal units like the App Store", () => {
    expect(formatBytes(0)).toBe("0 bytes");
    expect(formatBytes(1)).toBe("1 byte");
    expect(formatBytes(999)).toBe("999 bytes");
    expect(formatBytes(1000)).toBe("1 KB");
    expect(formatBytes(45_678)).toBe("46 KB");
    expect(formatBytes(150_300_000)).toBe("150.3 MB");
    expect(formatBytes(98_000_000)).toBe("98 MB");
    expect(formatBytes(1_250_000_000)).toBe("1.25 GB");
  });

  it("rolls over at unit boundaries", () => {
    expect(formatBytes(999_600)).toBe("1 MB");
    expect(formatBytes(999_960_000)).toBe("1 GB");
  });

  it("handles missing and invalid values", () => {
    expect(formatBytes(null)).toBe("—");
    expect(formatBytes(undefined)).toBe("—");
    expect(formatBytes(Number.NaN)).toBe("—");
    expect(formatBytes(-5)).toBe("—");
  });
});

describe("dates", () => {
  const opts = { locale: "en-US", timeZone: "UTC" };

  it("parses timestamps and plain dates", () => {
    expect(parseDate("2027-10-26")?.toISOString()).toBe("2027-10-26T00:00:00.000Z");
    expect(parseDate("2027-10-26T09:41:00Z")?.toISOString()).toBe("2027-10-26T09:41:00.000Z");
    expect(parseDate("not a date")).toBeNull();
    expect(parseDate(null)).toBeNull();
  });

  it("formats dates; plain dates ignore the time zone", () => {
    expect(formatDate("2027-10-26T09:41:00Z", opts)).toBe("Oct 26, 2027");
    expect(formatDate("2027-10-26", { locale: "en-US", timeZone: "America/Los_Angeles" })).toBe("Oct 26, 2027");
    expect(formatDate("sometime", opts)).toBe("sometime");
    expect(formatDate(null, opts)).toBe("—");
  });

  it("formats relative times", () => {
    const now = new Date("2026-10-29T12:00:00Z");
    const ago = (ms: number) => new Date(now.getTime() - ms);
    expect(formatRelative(ago(10_000), now, "en-US")).toBe("just now");
    expect(formatRelative(ago(5 * 60_000), now, "en-US")).toBe("5 minutes ago");
    expect(formatRelative(ago(3 * 3_600_000), now, "en-US")).toBe("3 hours ago");
    expect(formatRelative(ago(36 * 3_600_000), now, "en-US")).toBe("yesterday");
    expect(formatRelative(ago(4 * 86_400_000), now, "en-US")).toBe("4 days ago");
    expect(formatRelative(ago(70 * 86_400_000), now, "en-US")).toBe("2 months ago");
    expect(formatRelative(ago(800 * 86_400_000), now, "en-US")).toBe("2 years ago");
    expect(formatRelative("2026-11-01T12:00:00Z", now, "en-US")).toBe("in 3 days");
    expect(formatRelative(null, now, "en-US")).toBe("never");
  });


  it("describes expiry with a tone", () => {
    const now = new Date("2026-10-29T12:00:00Z");
    expect(expiryInfo("2027-10-26T09:41:00Z", now)).toEqual({ label: "Expires in 361 days", tone: "ok" });
    expect(expiryInfo("2026-11-20T12:00:00Z", now)).toEqual({ label: "Expires in 22 days", tone: "warn" });
    expect(expiryInfo("2026-10-30T13:00:00Z", now)).toEqual({ label: "Expires in 1 day", tone: "danger" });
    expect(expiryInfo("2026-10-29T18:00:00Z", now)).toEqual({ label: "Expires today", tone: "danger" });
    expect(expiryInfo("2026-10-26T12:00:00Z", now)).toEqual({ label: "Expired 3 days ago", tone: "danger" });
    expect(expiryInfo(null, now)).toBeNull();
  });
});

describe("small formatters", () => {
  it("pluralizes", () => {
    expect(plural(1, "app")).toBe("1 app");
    expect(plural(3, "app")).toBe("3 apps");
    expect(plural(2, "entry", "entries")).toBe("2 entries");
  });

  it("formats percentages", () => {
    expect(formatPercent(0.426)).toBe("43%");
    expect(formatPercent(2)).toBe("100%");
    expect(formatPercent(null)).toBeNull();
  });

  it("formats versions and requirements", () => {
    expect(formatVersion({ version: "1.7.1", build: "126" })).toBe("1.7.1 (126)");
    expect(formatVersion({ version: "21.44.0", build: "21.44.0" })).toBe("21.44.0");
    expect(formatVersion({ version: "2.0", build: null })).toBe("2.0");
    expect(formatVersion({ version: "21.43.5", build: "21.43.5", revision: 4 })).toBe("21.43.5 · Kiwi 4");
    expect(osRequirement({ min_os: "15.0", max_os: null })).toBe("iOS 15.0+");
    expect(osRequirement({ min_os: "15.0", max_os: "17.0" })).toBe("iOS 15.0–17.0");
    expect(osRequirement({ min_os: null, max_os: null })).toBe("Any iOS");
    expect(osRequirement({ min_os: null, max_os: null }, "android")).toBe("Android");
    expect(osRequirement({ min_os: "12", max_os: null }, "android")).toBe("Android 12+");
    expect([deviceName("android"), deviceName("ios"), deviceName(undefined)]).toEqual(["Android device", "iPhone", "iPhone"]);
  });

  it("names privacy keys", () => {
    expect(privacyLabel("NSCameraUsageDescription")).toBe("Camera");
    expect(privacyLabel("NSHomeKitUsageDescription")).toBe("Home Kit");
    expect(privacyLabel("CustomKey")).toBe("Custom Key");
  });
});

describe("serverLabel", () => {
  it("turns the first host label into words", () => {
    expect(serverLabel("https://mac-studio.example.ts.net")).toBe("Mac Studio");
    expect(serverLabel("http://localhost:8742")).toBe("Localhost");
  });

  it("keeps addresses and survives bad input", () => {
    expect(serverLabel("http://127.0.0.1:8742")).toBe("127.0.0.1");
    expect(serverLabel("http://[::1]:8742")).toBe("[::1]");
    expect(serverLabel("not a url")).toBe("not a url");
    expect(serverLabel("")).toBe("Kiwi");
  });
});

describe("formatRemaining", () => {
  const now = new Date("2026-10-29T12:00:00Z");
  it("rounds down to days, months or years", () => {
    expect(formatRemaining("2026-11-04T12:00:00Z", now)).toBe("6 days");
    expect(formatRemaining("2026-10-30T12:00:00Z", now)).toBe("1 day");
    expect(formatRemaining("2027-09-29T12:00:00Z", now)).toBe("11 mo");
    expect(formatRemaining("2027-10-29T12:00:00Z", now)).toBe("1 yr");
  });

  it("handles today, the past and missing dates", () => {
    expect(formatRemaining("2026-10-29T18:00:00Z", now)).toBe("Today");
    expect(formatRemaining("2026-10-01T00:00:00Z", now)).toBe("Expired");
    expect(formatRemaining(null, now)).toBeNull();
  });
});

describe("splitBytes", () => {
  it("separates the number and the unit", () => {
    expect(splitBytes(152_400_000)).toEqual(["152.4", "MB"]);
    expect(splitBytes(null)).toEqual(["—", ""]);
  });
});

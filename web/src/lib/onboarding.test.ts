import { describe, expect, it } from "vitest";
import type { Device, Warning } from "../api/types";
import { initialChoice, offersKiwiApp, setupPage, signingRow } from "./onboarding";

type Detection = Pick<Device, "platform" | "platform_source" | "detected_platform">;

const iphone: Detection = { platform: "ios", platform_source: "detected", detected_platform: "ios" };
const android: Detection = { platform: "android", platform_source: "detected", detected_platform: "android" };
const desktop: Detection = { platform: "ios", platform_source: "assumed", detected_platform: null };

const me = (device: Detection, androidApp = true) => ({
  device: device as Device,
  android_app: androidApp
    ? { package: "io.github.digitalpals.kiwi", version_name: "1.0", version_code: 1, size: 1, install_url: "/android/kiwi.apk" }
    : null,
});

describe("initialChoice", () => {
  it("preselects what Kiwi detected", () => {
    expect(initialChoice(iphone)).toBe("ios");
    expect(initialChoice(android)).toBe("android");
  });

  it("preselects nothing on a desktop browser, where iOS is only assumed", () => {
    expect(initialChoice(desktop)).toBeNull();
  });

  it("keeps an earlier choice when the setup is shown again", () => {
    expect(initialChoice({ ...desktop, platform: "android", platform_source: "chosen" })).toBe("android");
    expect(initialChoice({ ...android, platform: "ios", platform_source: "chosen" })).toBe("ios");
  });
});

describe("setupPage", () => {
  it("sets up the Home Screen app, notifications and signing on an iPhone", () => {
    expect(setupPage("ios", me(iphone))).toBe("iphone");
  });

  it("offers the Kiwi app on Android while Kiwi serves it", () => {
    expect(setupPage("android", me(android))).toBe("android");
    expect(setupPage("android", me(android, false))).toBeNull();
  });

  it("follows the choice, but offers the Kiwi app only where it installs", () => {
    expect(setupPage("ios", me(android))).toBe("iphone");
    // An iPhone can't install an APK.
    expect(setupPage("android", me(iphone))).toBeNull();
  });

  it("ends with the choice on a desktop browser", () => {
    expect(setupPage("ios", me(desktop))).toBeNull();
    expect(setupPage("android", me(desktop))).toBeNull();
  });
});

describe("offersKiwiApp", () => {
  it("offers the Kiwi app to a device detected as Android while Kiwi serves it", () => {
    expect(offersKiwiApp(me(android))).toBe(true);
    expect(offersKiwiApp(me(android, false))).toBe(false);
    expect(offersKiwiApp(me(desktop))).toBe(false);
    expect(offersKiwiApp(me(iphone))).toBe(false);
  });
});

describe("signingRow", () => {
  const now = new Date("2026-11-10T12:00:00Z");
  const active = (expires_at: string) => ({ identity_sha1: "a", profile_uuid: "b", expires_at });
  const warning = (level: Warning["level"], link: string): Warning => ({ level, code: "x", message: "m", link });

  it("says until when this Kiwi signs", () => {
    const row = signingRow([], active("2027-10-25T00:00:00Z"), now);
    expect(row.tone).toBe("ok");
    expect(row.title).toBe("Signing ready");
    expect(row.sub).toMatch(/^This Kiwi signs apps for this iPhone until .*2027/);
  });

  it("warns in amber when signing isn't set up", () => {
    expect(signingRow([warning("error", "/settings")], undefined, now)).toMatchObject({
      tone: "warn",
      title: "Signing isn't set up on this Kiwi yet",
    });
    expect(signingRow([], null, now).tone).toBe("warn");
  });

  it("ignores warnings that don't stop signing", () => {
    expect(signingRow([warning("error", "/sources"), warning("info", "/settings")], active("2027-10-25T00:00:00Z"), now).tone).toBe("ok");
  });

  it("warns when the signature expires soon", () => {
    expect(signingRow([], active("2026-11-20T12:00:00Z"), now)).toMatchObject({ tone: "warn", title: "Signing expires in 10 days" });
  });

  it("waits for the signing status", () => {
    expect(signingRow([], undefined, now)).toEqual({ tone: "off", title: "Checking signing…", sub: null });
  });
});

import { describe, expect, it } from "vitest";
import { base64UrlToBytes, usesServerKey } from "./push";

describe("base64UrlToBytes", () => {
  it("decodes unpadded base64url (VAPID keys)", () => {
    expect(Array.from(base64UrlToBytes("AQID"))).toEqual([1, 2, 3]);
    expect(Array.from(base64UrlToBytes("-_8"))).toEqual([251, 255]);
    const key = "BEl62iUYgUivxIkv69yViEuiBIa-Ib9-SkvMeAtA3LFgDzkrxZJjSgSnfckjBJuBkr3qBUYIHBQFLXYp5Nksh8U";
    expect(base64UrlToBytes(key)).toHaveLength(65);
  });
});

describe("usesServerKey", () => {
  const key = "AQID";
  const subscription = (applicationServerKey: ArrayBuffer | null) =>
    ({ options: { applicationServerKey } }) as unknown as PushSubscription;

  it("compares the subscription's key with the server's", () => {
    expect(usesServerKey(subscription(new Uint8Array([1, 2, 3]).buffer), key)).toBe(true);
    expect(usesServerKey(subscription(new Uint8Array([1, 2, 4]).buffer), key)).toBe(false);
  });

  it("trusts subscriptions that don't report a key", () => {
    expect(usesServerKey(subscription(null), key)).toBe(true);
  });
});

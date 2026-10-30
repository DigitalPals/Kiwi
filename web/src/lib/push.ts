import type { PushSubscriptionBody } from "../api/types";
import { isIOS, isStandalone } from "./pwa";

export type PushSupport =
  | "supported"
  | "needs-home-screen" // iOS only offers web push to Home Screen apps
  | "unsupported"
  | "no-service-worker" // development build or registration failed
  | "denied";

export function base64UrlToBytes(value: string): Uint8Array<ArrayBuffer> {
  const base64 = value.replace(/-/g, "+").replace(/_/g, "/").padEnd(Math.ceil(value.length / 4) * 4, "=");
  const binary = atob(base64);
  const bytes = new Uint8Array(binary.length);
  for (let i = 0; i < binary.length; i++) bytes[i] = binary.charCodeAt(i);
  return bytes;
}

export function subscriptionBody(subscription: PushSubscription): PushSubscriptionBody {
  const json = subscription.toJSON();
  const p256dh = json.keys?.p256dh;
  const auth = json.keys?.auth;
  if (!json.endpoint || !p256dh || !auth) throw new Error("The browser returned an incomplete push subscription.");
  return { endpoint: json.endpoint, keys: { p256dh, auth } };
}

async function registration(): Promise<ServiceWorkerRegistration | null> {
  if (!("serviceWorker" in navigator)) return null;
  return (await navigator.serviceWorker.getRegistration("/")) ?? null;
}

export async function pushSupport(): Promise<PushSupport> {
  const hasApis = "serviceWorker" in navigator && "PushManager" in window && "Notification" in window;
  if (!hasApis) return isIOS() && !isStandalone() ? "needs-home-screen" : "unsupported";
  if (Notification.permission === "denied") return "denied";
  return (await registration()) ? "supported" : "no-service-worker";
}

export async function currentSubscription(): Promise<PushSubscription | null> {
  const reg = await registration();
  return reg ? reg.pushManager.getSubscription() : null;
}

/** True when the subscription was made with this server key (or the browser doesn't say). */
export function usesServerKey(subscription: PushSubscription, vapidPublicKey: string): boolean {
  const key = subscription.options.applicationServerKey;
  return !key || sameBytes(new Uint8Array(key), base64UrlToBytes(vapidPublicKey));
}

/**
 * This browser's subscription if it still works with the server's key. One made with an
 * earlier key (the server regenerates an unreadable key file) can never receive pushes, so it
 * is dropped and notifications show as off until they are enabled again.
 */
export async function usableSubscription(vapidPublicKey: string): Promise<PushSubscription | null> {
  const subscription = await currentSubscription();
  if (!subscription || !vapidPublicKey || usesServerKey(subscription, vapidPublicKey)) return subscription;
  await subscription.unsubscribe().catch(() => false);
  return null;
}

/** Must be called directly from a tap: iOS only shows the permission prompt for a user gesture. */
export async function subscribe(vapidPublicKey: string): Promise<PushSubscription> {
  const permission = await Notification.requestPermission();
  if (permission !== "granted") {
    throw new Error(
      permission === "denied"
        ? "Notifications are blocked. Allow them in the iOS Settings app under Notifications › Kiwi."
        : "Notification permission was not granted.",
    );
  }
  const serverKey = base64UrlToBytes(vapidPublicKey);
  const reg = await navigator.serviceWorker.ready;
  const existing = await reg.pushManager.getSubscription();
  if (existing) {
    const key = existing.options.applicationServerKey;
    if (key && sameBytes(new Uint8Array(key), serverKey)) return existing;
    // Subscribed with another server key (e.g. the VAPID key was regenerated).
    await existing.unsubscribe();
  }
  return reg.pushManager.subscribe({ userVisibleOnly: true, applicationServerKey: serverKey });
}

function sameBytes(a: Uint8Array, b: Uint8Array): boolean {
  return a.length === b.length && a.every((byte, i) => byte === b[i]);
}

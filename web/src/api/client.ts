import type {
  AutomationOverview,
  AutomationPreferences,
  AutomationState,
  AppDetail,
  AppPatch,
  AppSummary,
  Build,
  Device,
  DevicePatch,
  DevicesResponse,
  Health,
  IdentityUpload,
  Me,
  MePatch,
  PrepareRequest,
  ProfileUpload,
  PushSubscriptionBody,
  PushTestResult,
  Settings,
  Signing,
  Source,
  SourcePatch,
  StoreApp,
} from "./types";

export class ApiError extends Error {
  readonly status: number;
  readonly code: string;

  constructor(message: string, status: number, code: string) {
    super(message);
    this.name = "ApiError";
    this.status = status;
    this.code = code;
  }
}

type Method = "GET" | "POST" | "PATCH" | "PUT" | "DELETE";
/** Request time limit: normal, slow (`true`) or long enough to download an IPA. */
type Speed = boolean | "download";
type FetchLike = (input: string, init: RequestInit) => Promise<Response>;

export const NETWORK_ERROR_MESSAGE = "Can't reach Kiwi. Check that the Mac running Kiwi is online and Tailscale is connected.";
export const TIMEOUT_MESSAGE = "Kiwi didn't answer in time. Try again.";

/**
 * Request time limits. Without one, a request stuck behind `tailscale serve` (or suspended while
 * iOS had the app in the background) would leave its screen loading until the app is closed.
 * Fetching sources and signing scans take longer than plain reads.
 */
export interface Timeouts {
  normal: number;
  slow: number;
  /** Adding or refreshing a source, which downloads the IPA of an app added by link. */
  download: number;
}

const TIMEOUTS: Timeouts = { normal: 30_000, slow: 120_000, download: 900_000 };

/** Turns any thrown value into a message suitable for the UI. */
export function errorMessage(error: unknown): string {
  if (error instanceof Error && error.message) return error.message;
  return "Something went wrong.";
}

function isErrorBody(value: unknown): value is { error: { code?: unknown; message?: unknown } } {
  return typeof value === "object" && value !== null && typeof (value as { error?: unknown }).error === "object";
}

export async function readError(res: Response): Promise<ApiError> {
  let code = "http";
  let message = res.statusText ? `${res.status} ${res.statusText}` : `Request failed (${res.status})`;
  try {
    const body: unknown = await res.json();
    if (isErrorBody(body) && body.error) {
      if (typeof body.error.code === "string") code = body.error.code;
      if (typeof body.error.message === "string" && body.error.message) message = body.error.message;
    }
  } catch {
    // Not JSON (e.g. a proxy error page); keep the status text.
  }
  return new ApiError(message, res.status, code);
}

export function createClient(
  fetchImpl: FetchLike = (input, init) => fetch(input, init),
  timeouts: Timeouts = TIMEOUTS,
) {
  // Without a device cookie every request would create its own browser device, so the first
  // `/api/me` (which sets the cookie) completes before any other request is sent. After a
  // failed first `/api/me` the next request starts over.
  let session: Promise<unknown> | null = null;
  let initializing: Promise<Me> | null = null;
  const reads = new Map<string, Promise<unknown>>();

  // Share only requests that are still running. Completed reads always go back to the
  // server, and a mutation starts a new generation so refreshes never join an older read.
  function read<T>(path: string): Promise<T> {
    const existing = reads.get(path);
    if (existing) return existing as Promise<T>;
    const result = send<T>("GET", path);
    reads.set(path, result);
    const clear = () => {
      if (reads.get(path) === result) reads.delete(path);
    };
    void result.then(clear, clear);
    return result;
  }

  function me(): Promise<Me> {
    if (initializing) return initializing;
    if (session) return read<Me>("/api/me");
    const result = read<Me>("/api/me");
    initializing = result;
    session = result.then(
      () => { initializing = null; },
      () => { initializing = null; session = null; },
    );
    return result;
  }

  async function request<T>(method: Method, path: string, body?: unknown, slow: Speed = false): Promise<T> {
    await (session ?? me().catch(() => undefined));
    if (method === "GET") return read<T>(path);
    reads.clear();
    try {
      return await send<T>(method, path, body, slow);
    } finally {
      // Reads started while the write was in flight may still describe the old state.
      reads.clear();
    }
  }

  async function send<T>(method: Method, path: string, body?: unknown, slow: Speed = false): Promise<T> {
    const controller = new AbortController();
    const limit = slow === "download" ? timeouts.download : slow ? timeouts.slow : timeouts.normal;
    const timer = setTimeout(() => controller.abort(), limit);
    try {
      return await exchange<T>(method, path, body, controller.signal);
    } catch (error) {
      if (controller.signal.aborted) throw new ApiError(TIMEOUT_MESSAGE, 0, "timeout");
      throw error;
    } finally {
      clearTimeout(timer);
    }
  }

  async function exchange<T>(method: Method, path: string, body: unknown, signal: AbortSignal): Promise<T> {
    const headers: Record<string, string> = { Accept: "application/json" };
    const init: RequestInit = { method, headers, credentials: "same-origin", cache: "no-store", signal };
    if (method !== "GET") {
      headers["X-Kiwi"] = "1";
      headers["Content-Type"] = "application/json";
      // POST/PATCH always carry a JSON body so the server never sees an empty JSON request.
      if (method !== "DELETE") init.body = JSON.stringify(body ?? {});
    }

    let res: Response;
    try {
      res = await fetchImpl(path, init);
    } catch (error) {
      if (error instanceof DOMException && error.name === "AbortError") throw error;
      throw new ApiError(NETWORK_ERROR_MESSAGE, 0, "network");
    }

    if (!res.ok) throw await readError(res);
    if (res.status === 204) return undefined as T;

    const type = res.headers.get("Content-Type") ?? "";
    if (!type.includes("json")) {
      throw new ApiError("Unexpected response from Kiwi. Is the server up to date?", res.status, "bad_response");
    }
    try {
      return (await res.json()) as T;
    } catch {
      throw new ApiError("Kiwi sent an invalid response.", res.status, "bad_response");
    }
  }

  const get = <T>(path: string) => request<T>("GET", path);
  const post = <T>(path: string, body?: unknown) => request<T>("POST", path, body);
  /** For requests that fetch sources, sync devices or scan signing on the server. */
  const slowPost = <T>(path: string, body?: unknown) => request<T>("POST", path, body, true);
  const patch = <T>(path: string, body: unknown) => request<T>("PATCH", path, body);
  const del = (path: string) => request<void>("DELETE", path);
  const seg = encodeURIComponent;
  const appPath = (sourceId: string, bundleId: string) => `/api/apps/${seg(sourceId)}/${seg(bundleId)}`;

  return {
    health: () => get<Health>("/api/health"),
    automationOverview: () => get<AutomationOverview>("/api/automation"),
    automation: (app: string) => get<AutomationState>(`/api/automation/${seg(app)}`),
    updateAutomation: (app: string, body: AutomationPreferences) =>
      patch<AutomationState>(`/api/automation/${seg(app)}`, body),
    addAutomation: (app: string, body: AutomationPreferences) =>
      post<AutomationState>(`/api/automation/${seg(app)}/add`, body),
    removeAutomation: (app: string) => request<AutomationState>("DELETE", `/api/automation/${seg(app)}`),
    checkAutomation: (app: string, force = false) =>
      post<AutomationState>(`/api/automation/${seg(app)}/check`, { force }),
    cancelAutomation: (app: string) => post<AutomationState>(`/api/automation/${seg(app)}/cancel`),
    me,
    updateMe: (body: MePatch) => patch<Me>("/api/me", body),

    sources: () => get<Source[]>("/api/sources"),
    addSource: (url: string) => request<Source>("POST", "/api/sources", { url }, "download"),
    updateSource: (id: string, body: SourcePatch) => patch<Source>(`/api/sources/${seg(id)}`, body),
    removeSource: (id: string) => del(`/api/sources/${seg(id)}`),
    refreshSource: (id: string) => request<Source>("POST", `/api/sources/${seg(id)}/refresh`, undefined, "download"),
    refreshSources: () => slowPost<Source[]>("/api/sources/refresh"),

    /** `hidden`: include apps hidden from Browse (each says whether it is). */
    apps: (query: { q?: string; source?: string; hidden?: boolean } = {}) => {
      const params = new URLSearchParams();
      if (query.q) params.set("q", query.q);
      if (query.source) params.set("source", query.source);
      if (query.hidden) params.set("hidden", "1");
      const qs = params.toString();
      return get<AppSummary[]>(qs ? `/api/apps?${qs}` : "/api/apps");
    },
    app: (sourceId: string, bundleId: string) => get<AppDetail>(appPath(sourceId, bundleId)),
    updateApp: (sourceId: string, bundleId: string, body: AppPatch) =>
      patch<AppDetail>(appPath(sourceId, bundleId), body),
    searchStore: (term: string) => get<StoreApp[]>(`/api/store/search?${new URLSearchParams({ term })}`),
    prepare: (sourceId: string, bundleId: string, body: PrepareRequest = {}) =>
      post<Build>(`${appPath(sourceId, bundleId)}/prepare`, body),
    build: (id: string) => get<Build>(`/api/builds/${seg(id)}`),

    library: () => get<AppSummary[]>("/api/library"),
    forget: (signedBundleId: string) => del(`/api/library/${seg(signedBundleId)}`),
    updates: () => get<AppSummary[]>("/api/updates"),
    prepareUpdates: () => slowPost<Build[]>("/api/updates/prepare"),

    signing: () => get<Signing>("/api/signing"),
    rescanSigning: () => slowPost<Signing>("/api/signing/rescan"),
    // Uploads and removals answer after a rescan, like rescanSigning.
    uploadIdentity: (body: IdentityUpload) => slowPost<Signing>("/api/signing/identity", body),
    removeIdentity: () => request<Signing>("DELETE", "/api/signing/identity", undefined, true),
    uploadProfile: (body: ProfileUpload) => slowPost<Signing>("/api/signing/profiles", body),
    removeProfile: (uuid: string) => request<Signing>("DELETE", `/api/signing/profiles/${seg(uuid)}`, undefined, true),
    settings: () => get<Settings>("/api/settings"),
    updateSettings: (body: Partial<Settings>) => patch<Settings>("/api/settings", body),

    subscribePush: (body: PushSubscriptionBody) => post<void>("/api/push/subscribe", body),
    unsubscribePush: (endpoint: string) => post<void>("/api/push/unsubscribe", { endpoint }),
    testPush: () => slowPost<PushTestResult>("/api/push/test"),

    devices: () => get<DevicesResponse>("/api/devices"),
    updateDevice: (id: string, body: DevicePatch) => patch<Device>(`/api/devices/${seg(id)}`, body),
    syncDevices: () => slowPost<DevicesResponse>("/api/devices/sync"),
  };
}

export type Client = ReturnType<typeof createClient>;

export const api: Client = createClient();

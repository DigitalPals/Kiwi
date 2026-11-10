import { useEffect, useState } from "preact/hooks";
import { api, errorMessage } from "../api/client";
import type { CoreDevice, Host, Identity, Me, Profile, Settings as SettingsData, Signing } from "../api/types";
import { Icon } from "../components/Icon";
import { KiwiLogo } from "../components/KiwiLogo";
import {
  BackBar,
  Cell,
  ErrorState,
  LinkCell,
  Loading,
  Page,
  Section,
  Spinner,
  Toggle,
} from "../components/ui";
import {
  expiryInfo,
  formatDate,
  formatDateTime,
  formatRelative,
  osName,
  osVersionSourceLabel,
  plural,
  serverLabel,
} from "../lib/format";
import { useResource, useTask } from "../lib/hooks";
import { useMe } from "../lib/me";
import { blockedMessage, pushSupport, subscribe, subscriptionBody, usableSubscription, type PushSupport } from "../lib/push";
import { BUILDS_PATH, DEVICE_PATH, NOTIFICATIONS_PATH, SIGNING_PATH } from "../lib/routes";
import { checkUploadSize, readBase64, signingSummary, splitCommand, uploadErrorMessage, UPLOADS, type UploadKind } from "../lib/signing";
import { countryName } from "../lib/store";

/** Text field that saves on blur or Return when changed. */
function InlineField(props: {
  value: string;
  label: string;
  placeholder?: string;
  inputMode?: "text" | "decimal";
  onSave: (value: string) => Promise<unknown>;
}) {
  const [draft, setDraft] = useState(props.value);
  const save = useTask(props.onSave);
  useEffect(() => setDraft(props.value), [props.value]);
  const commit = () => {
    const next = draft.trim();
    if (next && next !== props.value) void save.run(next);
    else setDraft(props.value);
  };
  return (
    <span class="inline-field">
      <input
        type="text"
        value={draft}
        aria-label={props.label}
        placeholder={props.placeholder}
        inputMode={props.inputMode ?? "text"}
        autocapitalize="off"
        autocorrect="off"
        spellcheck={false}
        enterKeyHint="done"
        onInput={(e) => setDraft(e.currentTarget.value)}
        onBlur={commit}
        onKeyDown={(e) => {
          if (e.key === "Enter") e.currentTarget.blur();
          if (e.key === "Escape") setDraft(props.value);
        }}
        disabled={save.pending}
      />
      {save.pending && <Spinner />}
      {save.error && <span class="form-error">{save.error}</span>}
    </span>
  );
}

function coreDeviceLabel(device: CoreDevice): string {
  const kind = device.physical ? (device.model ?? device.platform ?? "Device") : "Simulator";
  return `${device.name} (${kind}${device.os_version ? `, ${device.os_version}` : ""})`;
}

function DeviceSection({ me, setMe }: { me: Me; setMe: (me: Me) => void }) {
  const { device } = me;
  const devices = useResource("devices", api.devices);
  const link = useTask(async (devicectlId: string | null) => {
    await api.updateDevice(device.id, { devicectl_id: devicectlId });
    setMe(await api.me());
    await devices.reload();
  });
  const sync = useTask(async () => {
    devices.mutate(await api.syncDevices());
    setMe(await api.me());
  });
  const clearManual = useTask(async () => setMe(await api.updateMe({ os_version: null })));

  const android = device.platform === "android";
  const os = osName(device.platform);
  const linked = device.devicectl;
  const cores = devices.data?.core_devices ?? [];
  const physicalFirst = [...cores].sort((a, b) => Number(b.physical) - Number(a.physical));

  return (
    <Section
      footer={
        android
          ? "Kiwi can't see which apps an Android device has installed, so it tracks what you download here."
          : me.server.devicectl_available
            ? "Linking this device to the iPhone paired with the Mac lets Kiwi read its iOS version and installed apps."
            : "devicectl is not available on the Mac, so Kiwi can't see the iPhone directly."
      }
    >
      <Cell label="Name">
        <InlineField value={device.name} label="Device name" onSave={async (name) => setMe(await api.updateMe({ name }))} />
      </Cell>
      <Cell
        label={`${os} Version`}
        sub={device.os_version
          ? osVersionSourceLabel(device.os_version_source)
          : android ? "Not reported by the browser; Android apps install without it" : "Not known yet; all releases are offered"}
      >
        <InlineField
          value={device.os_version ?? ""}
          label={`${os} version`}
          placeholder={android ? "e.g. 16" : "e.g. 27.0.1"}
          inputMode="decimal"
          onSave={async (os_version) => setMe(await api.updateMe({ os_version }))}
        />
      </Cell>
      {device.os_version_source === "manual" && (
        <Cell label="Manual Version" sub={android ? "Use the version the browser reports instead" : "Use the version reported by the iPhone instead"}>
          <button type="button" class="text-button" onClick={() => void clearManual.run()} disabled={clearManual.pending}>
            Clear
          </button>
        </Cell>
      )}
      {device.model && (
        <Cell label="Model">
          {device.model}
        </Cell>
      )}
      <Cell label="Tailscale">{device.tailscale_name ?? (device.kind === "browser" ? "Not identified (browser cookie)" : "—")}</Cell>
      {!android && <Cell label="Linked Mac Device" sub={linked ? linkStatus(linked) : null}>
        {devices.data ? (
          <select
            class="select"
            aria-label="Linked Mac device"
            value={linked?.id ?? ""}
            disabled={link.pending || !me.server.devicectl_available}
            onChange={(e) => void link.run(e.currentTarget.value || null)}
          >
            <option value="">None</option>
            {linked && !cores.some((c) => c.id === linked.id) && <option value={linked.id}>{linked.name}</option>}
            {physicalFirst.map((c) => (
              <option key={c.id} value={c.id} disabled={!!c.linked_device_id && c.linked_device_id !== device.id}>
                {coreDeviceLabel(c)}
                {c.linked_device_id && c.linked_device_id !== device.id ? " — linked elsewhere" : ""}
              </option>
            ))}
          </select>
        ) : devices.error ? (
          <span class="danger small">{devices.error.message}</span>
        ) : (
          <Spinner />
        )}
      </Cell>}
      {!android && <Cell label="Sync with Mac" sub={linked?.last_sync_at ? `Last synced ${formatRelative(linked.last_sync_at)}` : "Reads the iOS version and installed apps"}>
        <button
          type="button"
          class="text-button"
          onClick={() => void sync.run()}
          disabled={sync.pending || !me.server.devicectl_available}
        >
          {sync.pending ? <Spinner /> : "Sync Now"}
        </button>
      </Cell>}
      {(link.error || sync.error || clearManual.error) && (
        <div class="cell">
          <p class="form-error">{link.error ?? sync.error ?? clearManual.error}</p>
        </div>
      )}
    </Section>
  );
}

function linkStatus(linked: NonNullable<Me["device"]["devicectl"]>): string {
  if (linked.last_error) return `Error: ${linked.last_error}`;
  return linked.reachable ? "Reachable" : "Not reachable (connect the iPhone to the Mac's network)";
}

const SUPPORT_MESSAGES: Record<Exclude<PushSupport, "supported" | "denied">, string> = {
  "needs-home-screen": "Add Kiwi to your Home Screen (Share › Add to Home Screen) and open it from there to turn on notifications.",
  unsupported: "This browser doesn't support web push notifications.",
  "no-service-worker": "The service worker isn't running (it is only registered in production builds).",
};

function NotificationsSection({ me, reloadMe }: { me: Me; reloadMe: () => void }) {
  const [support, setSupport] = useState<PushSupport | null>(null);
  const [subscription, setSubscription] = useState<PushSubscription | null>(null);
  const [message, setMessage] = useState<string | null>(null);

  useEffect(() => {
    void pushSupport().then(setSupport);
    void usableSubscription(me.push.vapid_public_key)
      .then(setSubscription)
      .catch(() => setSubscription(null));
  }, [me.push.vapid_public_key]);

  const enable = useTask(async () => {
    setMessage(null);
    const sub = await subscribe(me.push.vapid_public_key);
    await api.subscribePush(subscriptionBody(sub));
    setSubscription(sub);
    reloadMe();
  });
  const test = useTask(async () => {
    const { sent } = await api.testPush();
    setMessage(sent > 0 ? `Sent a test notification to ${plural(sent, "subscription")}.` : "No subscriptions to send to.");
  });
  const disable = useTask(async () => {
    setMessage(null);
    if (subscription) {
      await api.unsubscribePush(subscription.endpoint);
      await subscription.unsubscribe();
    }
    setSubscription(null);
    reloadMe();
  });

  const error = enable.error ?? test.error ?? disable.error;
  let body;
  if (!me.push.vapid_public_key) {
    body = <Cell label="Unavailable" sub="The server has no push key configured." />;
  } else if (support === null) {
    body = (
      <Cell label="Checking…">
        <Spinner />
      </Cell>
    );
  } else if (subscription) {
    body = (
      <>
        <Cell label="Notifications" sub="Updates, ready installs and expiry warnings">
          <span class="tone-ok">On</span>
        </Cell>
        <Cell label="Send a Test">
          <button type="button" class="text-button" onClick={() => void test.run()} disabled={test.pending}>
            {test.pending ? <Spinner /> : "Send"}
          </button>
        </Cell>
        <Cell label="Turn Off">
          <button type="button" class="text-button danger" onClick={() => void disable.run()} disabled={disable.pending}>
            {disable.pending ? <Spinner /> : "Disable"}
          </button>
        </Cell>
      </>
    );
  } else if (support !== "supported") {
    body = (
      <Cell label="Notifications" sub={support === "denied" ? blockedMessage() : SUPPORT_MESSAGES[support]}>
        {me.push.subscribed ? "On elsewhere" : "Off"}
      </Cell>
    );
  } else {
    body = (
      <>
        <Cell
          label="Notifications"
          sub={me.push.subscribed ? "On for another browser on this device" : "Get notified about updates and expiring apps"}
        >
          <button type="button" class="pill pill-filled" onClick={() => void enable.run()} disabled={enable.pending}>
            {enable.pending ? "Enabling…" : "Enable"}
          </button>
        </Cell>
        {me.push.subscribed && (
          <Cell label="Send a Test">
            <button type="button" class="text-button" onClick={() => void test.run()} disabled={test.pending}>
              {test.pending ? <Spinner /> : "Send"}
            </button>
          </Cell>
        )}
      </>
    );
  }

  return (
    <Section>
      {body}
      {(error || message) && (
        <div class="cell">
          <p class={error ? "form-error" : "muted small"}>{error ?? message}</p>
        </div>
      )}
    </Section>
  );
}

/** "com.example.app", or "Wildcard (TEAM.*)": Xcode names every profile alike. */
function profileLabel(profile: Profile): string {
  if (profile.wildcard) return `Wildcard (${profile.app_id_pattern})`;
  return profile.app_id_pattern.startsWith(`${profile.team_id}.`)
    ? profile.app_id_pattern.slice(profile.team_id.length + 1)
    : profile.app_id_pattern;
}

/** docs/signing-on-linux.md in the public repository. */
const SIGNING_GUIDE_URL = "https://github.com/DigitalPals/Kiwi/blob/main/docs/signing-on-linux.md";

const EXPORT_HINT =
  "On a Mac, open Keychain Access › My Certificates, right-click “Apple Development: …”, choose Export, save as .p12 and set a password.";

const REMOVE_IDENTITY =
  "Remove the signing identity?\n\nKiwi can't sign apps until you upload one again. Apps already installed keep working until they expire.";

/** "Upload .p12…": a row round a hidden file input, so a tap opens the file picker. */
function UploadCell(props: { kind: UploadKind; label: string; pending: boolean; onFile: (file: File) => void }) {
  return (
    <label class="cell cell-button center upload-cell" aria-busy={props.pending}>
      <input
        type="file"
        class="visually-hidden"
        accept={UPLOADS[props.kind].accept}
        disabled={props.pending}
        onChange={(e) => {
          const file = e.currentTarget.files?.[0];
          // Choosing the same file again (after fixing the password) fires change again.
          e.currentTarget.value = "";
          if (file) props.onFile(file);
        }}
      />
      {props.pending ? (
        <Spinner />
      ) : (
        <span class="cell-action">
          <Icon name="upload" size={18} />
          {props.label}
        </span>
      )}
    </label>
  );
}

/** A failed upload or removal, with a suggested command on a line of its own. */
function UploadError({ message, class: className = "form-error" }: { message: string; class?: string }) {
  const { text, command } = splitCommand(message);
  return (
    <p class={className} role="alert">
      {text}
      {command && (
        <code class="error-command">
          {/* Lines break between the arguments, never after the hyphen of an option. */}
          {command.split(" ").flatMap((part, i) => [i > 0 ? " " : null, <span key={i}>{part}</span>])}
        </code>
      )}
    </p>
  );
}

function IdentityCell({ identity, inUse, files }: { identity: Identity; inUse: boolean; files: boolean }) {
  const expiry = expiryInfo(identity.expires_at);
  return (
    <div class="cell cell-stacked">
      <div class="cell-label">
        {identity.name} {inUse && <span class="tag tag-tint">In Use</span>}
      </div>
      <div class="cell-sub">
        {identity.team_id ? `Team ${identity.team_id}` : "No team"}
        {expiry && (
          <>
            {" · "}
            <span class={expiry.tone === "ok" ? undefined : `tone-${expiry.tone}`}>
              {expiry.tone !== "ok"
                ? expiry.label.toLowerCase()
                : `${files ? "expires" : "until"} ${formatDate(identity.expires_at)}`}
            </span>
          </>
        )}
      </div>
      {!files && <div class="cell-sub mono small">{identity.sha1}</div>}
    </div>
  );
}

function ProfileCell({ profile, active, details, onRemove, removing }: {
  profile: Profile;
  active: boolean;
  /** The profile's name, UUID and entitlements, folded (among Xcode's many look-alike profiles). */
  details: boolean;
  /** Uploaded profiles only: Xcode's are not Kiwi's to delete. */
  onRemove?: () => void;
  removing?: boolean;
}) {
  const expiry = expiryInfo(profile.expires_at);
  // An expired profile's reason only repeats the date.
  const expired = Date.parse(profile.expires_at) <= Date.now();
  const facts = [
    profile.wildcard ? null : "Explicit",
    profile.development ? null : "Distribution",
    plural(profile.devices, "device"),
  ].filter(Boolean);
  return (
    <div class="cell">
      <div class="cell-main">
        <div class="cell-label">
          {profileLabel(profile)} {active && <span class="tag tag-tint">In Use</span>}
        </div>
        <div class="cell-sub">
          {facts.join(" · ")}
          {" · "}
          {expiry && expiry.tone !== "ok" ? (
            <span class={`tone-${expiry.tone}`}>{expiry.label.toLowerCase()}</span>
          ) : (
            `until ${formatDate(profile.expires_at)}`
          )}
        </div>
        {!profile.usable && !expired && <div class="cell-sub danger">{profile.reason ?? "Not usable"}</div>}
        {details && (
          <details class="cell-details">
            <summary class="cell-sub">
              {profile.name}
              {profile.entitlements.length > 0 && ` · ${plural(profile.entitlements.length, "entitlement")}`}
            </summary>
            <ul class="mono-list">
              <li>{profile.uuid}</li>
              {profile.entitlements.map((e) => (
                <li key={e}>{e}</li>
              ))}
            </ul>
          </details>
        )}
      </div>
      {onRemove && (
        <button
          type="button"
          class="text-button small danger"
          onClick={onRemove}
          disabled={removing}
          aria-label={`Remove ${profileLabel(profile)}`}
        >
          {removing ? <Spinner /> : "Remove"}
        </button>
      )}
    </div>
  );
}

/** The answer first: can Kiwi sign, and until when. */
function SigningStatus({ data, os }: { data: Signing; os: Host["os"] | undefined }) {
  const status = signingSummary(data, os);
  return (
    <div class={status.tone === "ok" ? "status-card" : `status-card ${status.tone}`}>
      <span class="status-icon" aria-hidden="true">
        <Icon name={status.icon} size={20} />
      </span>
      <div>
        <div class="status-title">{status.title}</div>
        <div class="status-text">{status.text}</div>
      </div>
    </div>
  );
}

/** Asks the password of the chosen .p12, then uploads both; the server checks them. */
function IdentitySheet({ file, onUploaded, onClose }: { file: File; onUploaded: (next: Signing) => void; onClose: () => void }) {
  const [password, setPassword] = useState("");
  const upload = useTask(async () => {
    let next: Signing;
    try {
      next = await api.uploadIdentity({ p12: await readBase64(file), password });
    } catch (e) {
      throw new Error(uploadErrorMessage("identity", e));
    }
    onUploaded(next);
  });
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if (e.key === "Escape") onClose();
    };
    document.addEventListener("keydown", onKey);
    return () => document.removeEventListener("keydown", onKey);
  }, [onClose]);
  return (
    <div class="sheet-backdrop" onClick={onClose}>
      <div class="sheet" role="dialog" aria-modal="true" aria-labelledby="identity-title" onClick={(e) => e.stopPropagation()}>
        <header class="sheet-header">
          <h2 id="identity-title">Upload Identity</h2>
          <button type="button" class="text-button" onClick={onClose}>
            Cancel
          </button>
        </header>
        <p class="sheet-lead">Enter the password you set when you exported “{file.name}”.</p>
        <form
          onSubmit={(e) => {
            e.preventDefault();
            if (!upload.pending) void upload.run();
          }}
        >
          <label class="sheet-field">
            <Icon name="key" size={18} />
            <span class="visually-hidden">Password of the .p12 file</span>
            <input
              type="password"
              placeholder="Password"
              value={password}
              onInput={(e) => {
                setPassword(e.currentTarget.value);
                upload.setError(null);
              }}
              autoFocus
              autocomplete="off"
              autocapitalize="off"
              autocorrect="off"
              spellcheck={false}
              enterKeyHint="go"
              disabled={upload.pending}
            />
          </label>
          {upload.error ? (
            <UploadError message={upload.error} class="sheet-field-status form-error" />
          ) : (
            <p class="sheet-field-status">
              {upload.pending
                ? "Checking the identity…"
                : "Kiwi keeps the identity and its password on the computer it runs on, readable only by Kiwi, and never sends them back."}
            </p>
          )}
          <button type="submit" class="button button-block" disabled={upload.pending}>
            {upload.pending ? <Spinner /> : "Upload"}
          </button>
        </form>
      </div>
    </div>
  );
}

/** Files backend: the uploaded .p12 (one at a time), or how to get one. */
function IdentitySection({ data, onChange }: { data: Signing; onChange: (next: Signing) => void }) {
  const [file, setFile] = useState<File | null>(null);
  const [problem, setProblem] = useState<string | null>(null);
  const remove = useTask(async () => onChange(await api.removeIdentity()));
  const { identities } = data;
  // An upload replaces the identity, so an expiring one can be renewed in place.
  const renew = identities.some((i) => {
    const expiry = expiryInfo(i.expires_at);
    return expiry !== null && expiry.tone !== "ok";
  });
  const choose = (chosen: File) => {
    const tooLarge = checkUploadSize("identity", chosen.size);
    setProblem(tooLarge);
    remove.setError(null);
    if (!tooLarge) setFile(chosen);
  };
  const confirmRemove = () => {
    if (!window.confirm(REMOVE_IDENTITY)) return;
    setProblem(null);
    void remove.run();
  };
  const error = problem ?? remove.error;
  return (
    <>
      <Section
        title="Signing Identity"
        footer={
          identities.length === 0 ? (
            <>
              {EXPORT_HINT} Without a Mac, see{" "}
              <a href={SIGNING_GUIDE_URL} target="_blank" rel="noreferrer">
                Signing on Linux
              </a>
              .
            </>
          ) : undefined
        }
      >
        {identities.length === 0 && (
          <Cell label="No Identity" sub="An Apple Development certificate with its private key, as a .p12 file" />
        )}
        {identities.map((identity) => (
          <IdentityCell key={identity.sha1} identity={identity} inUse={identity.sha1 === data.active?.identity_sha1} files />
        ))}
        {(identities.length === 0 || renew) && (
          <UploadCell kind="identity" label={identities.length ? "Upload a New .p12…" : "Upload .p12…"} pending={false} onFile={choose} />
        )}
        {identities.length > 0 && (
          <button type="button" class="cell cell-button center" onClick={confirmRemove} disabled={remove.pending}>
            {remove.pending ? <Spinner /> : <span class="cell-action danger">Remove Identity</span>}
          </button>
        )}
        {error && (
          <div class="cell">
            <UploadError message={error} />
          </div>
        )}
      </Section>
      {file && (
        <IdentitySheet
          file={file}
          onUploaded={(next) => {
            setFile(null);
            onChange(next);
          }}
          onClose={() => setFile(null)}
        />
      )}
    </>
  );
}

/** Every backend: the profiles Kiwi found, removing uploaded ones, and uploading more. */
function ProfilesSection({ data, onChange }: { data: Signing; onChange: (next: Signing) => void }) {
  const files = data.backend === "files";
  const [problem, setProblem] = useState<string | null>(null);
  const [removing, setRemoving] = useState<string | null>(null);
  const upload = useTask(async (file: File) => {
    try {
      onChange(await api.uploadProfile({ profile: await readBase64(file) }));
    } catch (e) {
      throw new Error(uploadErrorMessage("profile", e));
    }
  });
  const remove = useTask(async (uuid: string) => {
    setRemoving(uuid);
    try {
      onChange(await api.removeProfile(uuid));
    } finally {
      setRemoving(null);
    }
  });
  const choose = (file: File) => {
    const tooLarge = checkUploadSize("profile", file.size);
    setProblem(tooLarge);
    upload.setError(null);
    remove.setError(null);
    if (!tooLarge) void upload.run(file);
  };
  const confirmRemove = (profile: Profile) => {
    const inUse = profile.uuid === data.active?.profile_uuid;
    const question = `Remove the profile ${profileLabel(profile)}?`;
    if (!window.confirm(inUse ? `${question}\n\nKiwi signs with it now. Without another usable profile it can't sign new builds.` : question)) return;
    setProblem(null);
    upload.setError(null);
    void remove.run(profile.uuid);
  };
  const error = problem ?? upload.error ?? remove.error;
  return (
    <Section
      title="Provisioning Profiles"
      footer={
        files ? undefined : "Kiwi reads the profiles Xcode manages and the ones you upload, and prefers an explicit App ID profile when there is one."
      }
    >
      {data.profiles.length === 0 &&
        (files ? (
          <Cell label="No Profiles" sub="Download a development profile at developer.apple.com" />
        ) : (
          <Cell label="No profiles found" sub="Xcode stores them in ~/Library/Developer/Xcode/UserData/Provisioning Profiles." />
        ))}
      {data.profiles.map((p) => (
        <ProfileCell
          key={p.uuid}
          profile={p}
          active={p.uuid === data.active?.profile_uuid}
          details={!files}
          onRemove={p.uploaded ? () => confirmRemove(p) : undefined}
          removing={removing === p.uuid}
        />
      ))}
      <UploadCell kind="profile" label="Upload Profile…" pending={upload.pending} onFile={choose} />
      {error && (
        <div class="cell">
          <UploadError message={error} />
        </div>
      )}
    </Section>
  );
}

/** Settings › Signing: the certificate and profiles Kiwi signs with, from the keychain or uploaded. */
export function SigningScreen() {
  const signing = useResource<Signing>("signing", api.signing);
  const { me, reload: reloadMe } = useMe();
  // Signing decides Me's warnings (and the Settings tab's dot), so refresh them too.
  const apply = (next: Signing) => {
    signing.mutate(next);
    reloadMe();
  };
  const rescan = useTask(async () => apply(await api.rescanSigning()));
  const data = signing.data;

  let content;
  if (!data) {
    content = signing.error ? <ErrorState error={signing.error} onRetry={signing.reload} /> : <Loading />;
  } else if (data.backend === "files") {
    content = (
      <>
        <SigningStatus data={data} os={me?.host.os} />
        <IdentitySection data={data} onChange={apply} />
        <ProfilesSection data={data} onChange={apply} />
      </>
    );
  } else {
    content = (
      <>
        <SigningStatus data={data} os={me?.host.os} />
        <Section title={data.identities.length === 1 ? "Certificate" : "Certificates"}>
          {data.identities.length === 0 && (
            <Cell label="No signing identities" sub="Kiwi needs an Apple Development certificate in the login keychain." />
          )}
          {data.identities.map((identity) => (
            <IdentityCell
              key={identity.sha1}
              identity={identity}
              inUse={identity.sha1 === data.active?.identity_sha1}
              files={false}
            />
          ))}
        </Section>
        <ProfilesSection data={data} onChange={apply} />
        <Section footer={rescan.error ? <span class="danger">{rescan.error}</span> : "After renewing in Xcode, rescan to use the new files."}>
          <button type="button" class="cell cell-button center" onClick={() => void rescan.run()} disabled={rescan.pending}>
            {rescan.pending ? <Spinner /> : <span class="cell-action">Rescan Profiles and Identities</span>}
          </button>
        </Section>
      </>
    );
  }
  return (
    <main class="page">
      <BackBar label="Settings" fallback="/settings" />
      <header class="page-header">
        <h1 class="large-title">Signing</h1>
      </header>
      {content}
    </main>
  );
}

const REFRESH_CHOICES = [15, 30, 60, 120, 240, 360, 720, 1440];
const SYNC_CHOICES = [5, 10, 15, 30, 60];

function minutesLabel(minutes: number): string {
  if (minutes % 1440 === 0) return plural(minutes / 1440, "day");
  if (minutes % 60 === 0) return plural(minutes / 60, "hour");
  return plural(minutes, "minute");
}

function MinutesSelect(props: { value: number; choices: number[]; label: string; disabled?: boolean; onChange: (v: number) => void }) {
  const choices = props.choices.includes(props.value)
    ? props.choices
    : [...props.choices, props.value].sort((a, b) => a - b);
  return (
    <select
      class="select"
      aria-label={props.label}
      value={String(props.value)}
      disabled={props.disabled}
      onChange={(e) => props.onChange(Number(e.currentTarget.value))}
    >
      {choices.map((m) => (
        <option key={m} value={String(m)}>
          {minutesLabel(m)}
        </option>
      ))}
    </select>
  );
}

// Storefronts offered for the App Store comparison; any other stored value is kept.
const STORE_COUNTRIES = [
  "nl", "be", "lu", "de", "at", "ch", "fr", "gb", "ie", "es", "pt", "it", "dk", "se", "no", "fi", "pl", "cz",
  "us", "ca", "mx", "br", "au", "nz", "jp", "kr", "tw", "hk", "sg", "in", "tr", "ae",
];

function CountrySelect(props: { value: string; onChange: (code: string) => void }) {
  const codes = STORE_COUNTRIES.includes(props.value) ? STORE_COUNTRIES : [...STORE_COUNTRIES, props.value];
  const options = codes.map((code) => ({ code, name: countryName(code) })).sort((a, b) => a.name.localeCompare(b.name));
  return (
    <select
      class="select"
      aria-label="App Store country"
      value={props.value}
      onChange={(e) => props.onChange(e.currentTarget.value)}
    >
      {options.map((o) => (
        <option key={o.code} value={o.code}>
          {o.name}
        </option>
      ))}
    </select>
  );
}

/** The server's settings, saved one change at a time (put back when saving fails). */
function usePreferences() {
  const settings = useResource<SettingsData>("settings", api.settings);
  const [error, setError] = useState<string | null>(null);
  const data = settings.data;
  const update = async (patch: Partial<SettingsData>) => {
    if (!data) return;
    const previous = data;
    setError(null);
    settings.mutate({ ...data, ...patch });
    try {
      settings.mutate(await api.updateSettings(patch));
    } catch (e) {
      settings.mutate(previous);
      setError(errorMessage(e));
    }
  };
  return { settings, data, update, error };
}

const BUILDS_NOTICE =
  "Turn on Kiwi Builds?\n\n" +
  "Kiwi will download App Store apps with your Apple account, decrypt them on this Mac and patch them with " +
  "open-source tweaks. Patched apps may break the app's terms of service. Use it only for apps you may use, " +
  "on your own devices, at your own risk.\n\n" +
  "The Mac needs a one-time setup first (see Kiwi's documentation).";

/** Kiwi Builds: off until the owner turns it on and confirms what it does. */
function BuildsSection({ data, update }: { data: SettingsData; update: (patch: Partial<SettingsData>) => Promise<void> }) {
  const toggle = (on: boolean) => {
    if (on && !window.confirm(BUILDS_NOTICE)) return;
    void update({ kiwi_builds: on });
  };
  return (
    <Section title="Kiwi Builds" footer="Patch App Store apps with open-source tweaks and sign them on this Mac.">
      <Cell label="Kiwi Builds" icon="wrench" sub={data.kiwi_builds ? undefined : "Off"}>
        <Toggle label="Kiwi Builds" checked={data.kiwi_builds} onChange={toggle} />
      </Cell>
      {data.kiwi_builds && <LinkCell href={BUILDS_PATH} label="Apps, Patches and Schedules" />}
    </Section>
  );
}

/** Signing and the App Store country, then how Kiwi keeps apps up to date. */
function MacSections({ me }: { me: Me }) {
  const { settings, data, update, error } = usePreferences();
  const signing = useResource<Signing>("signing", api.signing);
  const active = signing.data?.active;
  const expiry = active ? expiryInfo(active.expires_at) : null;
  let signingValue;
  if (!signing.data) signingValue = signing.error ? "Unknown" : <Spinner />;
  else if (!active) signingValue = <span class="tone-danger">Can't sign</span>;
  else if (expiry && expiry.tone !== "ok") signingValue = <span class={`tone-${expiry.tone}`}>{expiry.label}</span>;
  else signingValue = `Until ${formatDate(active.expires_at)}`;

  return (
    <>
      <Section title={serverLabel(me.server.public_url)}>
        <LinkCell href={SIGNING_PATH} icon="shield" label="Signing" value={signingValue} />
        <Cell label="App Store Country" icon="globe">
          {data ? (
            <CountrySelect value={data.app_store_country} onChange={(code) => void update({ app_store_country: code })} />
          ) : (
            <Spinner />
          )}
        </Cell>
      </Section>
      {!data ? (
        <Section title="Updates">
          {settings.error ? (
            <Cell label="Couldn't load settings" sub={settings.error.message}>
              <button type="button" class="text-button" onClick={() => void settings.reload()}>
                Retry
              </button>
            </Cell>
          ) : (
            <Cell label="Loading…">
              <Spinner />
            </Cell>
          )}
        </Section>
      ) : (
        <Section title="Updates" footer={error ? <span class="danger">{error}</span> : undefined}>
          <Cell label="Check Sources Every">
            <MinutesSelect
              value={data.refresh_interval_minutes}
              choices={REFRESH_CHOICES}
              label="Source refresh interval"
              onChange={(v) => void update({ refresh_interval_minutes: v })}
            />
          </Cell>
          <Cell label="Prepare Updates Automatically" sub="Download and sign updates in the background so they're ready to install">
            <Toggle label="Prepare updates automatically" checked={data.auto_prepare_updates} onChange={(v) => void update({ auto_prepare_updates: v })} />
          </Cell>
          <Cell label="Notify About Updates">
            <Toggle label="Notify about updates" checked={data.notify_updates} onChange={(v) => void update({ notify_updates: v })} />
          </Cell>
          <Cell label="Notify Before Apps Expire" sub="30, 7 and 1 days before the signature expires">
            <Toggle label="Notify before apps expire" checked={data.notify_expiry} onChange={(v) => void update({ notify_expiry: v })} />
          </Cell>
          <Cell label="Remove Apple Watch Apps" sub="Smaller installs; watch apps can't be re-signed for the phone alone">
            <Toggle label="Remove Apple Watch apps" checked={data.strip_watch_apps} onChange={(v) => void update({ strip_watch_apps: v })} />
          </Cell>
        </Section>
      )}
      {data && <BuildsSection data={data} update={update} />}
    </>
  );
}

/** How often Kiwi asks the Mac's devicectl about paired iPhones. */
function DeviceSyncSection() {
  const { data, update, error } = usePreferences();
  if (!data) return null;
  return (
    <Section
      title="Sync with devicectl"
      footer={error ? <span class="danger">{error}</span> : "Checks installed apps and the iOS version when the iPhone is paired with the Mac."}
    >
      <Cell label="Sync Automatically">
        <Toggle label="Sync with devicectl" checked={data.devicectl_sync} onChange={(v) => void update({ devicectl_sync: v })} />
      </Cell>
      {data.devicectl_sync && (
        <Cell label="Sync Every">
          <MinutesSelect
            value={data.devicectl_sync_minutes}
            choices={SYNC_CHOICES}
            label="devicectl sync interval"
            onChange={(v) => void update({ devicectl_sync_minutes: v })}
          />
        </Cell>
      )}
    </Section>
  );
}

/** A browser Kiwi can't place on the tailnet, or an Android phone or tablet, is "This Device". */
const deviceTitle = (me: Me) => (me.device.kind === "browser" || me.device.platform === "android" ? "This Device" : "This iPhone");

/** This iPhone (name and iOS version) and its notifications. */
function DeviceRows({ me }: { me: Me }) {
  const { device } = me;
  return (
    <>
      <LinkCell
        href={DEVICE_PATH}
        icon="phone"
        label={device.name}
        value={
          device.platform === "android"
            ? [osName(device.platform), device.os_version].filter(Boolean).join(" ")
            : device.os_version ? `iOS ${device.os_version}` : <span class="tone-warn">iOS unknown</span>
        }
      />
      <LinkCell href={NOTIFICATIONS_PATH} icon="bell" label="Notifications" value={me.push.subscribed ? "On" : "Off"} />
    </>
  );
}

/** Kiwi's card: a turning kiwi slice, the version and address, and whether it can sign right now. */
function AboutCard({ me }: { me: Me }) {
  const signing = useResource<Signing>("signing", api.signing);
  const active = signing.data?.active;
  const expiry = active ? expiryInfo(active.expires_at) : null;
  let dot = "dot";
  let status: string;
  if (signing.data === undefined) {
    dot = "dot dot-off";
    status = signing.error ? "Signing status unknown" : "Checking signing…";
  } else if (!active) {
    dot = "dot dot-error";
    status = "Can't sign apps";
  } else if (expiry && expiry.tone !== "ok") {
    dot = "dot dot-warn";
    status = `Signing ${expiry.label.toLowerCase()}`;
  } else status = "Ready to sign";
  return (
    <div class="about-card">
      <KiwiLogo size={150} />
      <div class="about-text">
        <div class="about-name">
          Kiwi<small>{me.server.version}</small>
        </div>
        <div class="about-host">{me.server.public_url.replace(/^https?:\/\//, "")}</div>
        <div class="about-status">
          <span class={dot === "dot" ? "dot dot-live" : dot} />
          <span>{status}</span>
        </div>
      </div>
    </div>
  );
}

function Versions({ me }: { me: Me }) {
  return (
    <p class="fine-print">
      Server {me.server.version} · web app {__APP_VERSION__}
    </p>
  );
}

export function Settings() {
  const { me, error, reload } = useMe();

  let content;
  if (me) {
    content = (
      <>
        <AboutCard me={me} />
        <Section title={deviceTitle(me)}>
          <DeviceRows me={me} />
        </Section>
        <MacSections me={me} />
        <Versions me={me} />
      </>
    );
  } else if (error) {
    content = <ErrorState error={error} onRetry={reload} />;
  } else {
    content = <Loading />;
  }

  return <Page title="Settings">{content}</Page>;
}

/** Settings › This iPhone: its name, iOS version, Tailscale name and link to the Mac. */
export function DeviceScreen() {
  const { me, error, reload, setMe } = useMe();
  let content;
  if (me) {
    content = (
      <>
        <DeviceSection me={me} setMe={setMe} />
        {me.device.platform !== "android" && <DeviceSyncSection />}
        <Section title="Details">
          <Cell label="Device ID">
            <span class="mono small">{me.device.id}</span>
          </Cell>
          <Cell label="Last Seen">{formatDateTime(me.device.last_seen)}</Cell>
        </Section>
      </>
    );
  } else content = error ? <ErrorState error={error} onRetry={reload} /> : <Loading />;
  return (
    <main class="page">
      <BackBar label="Settings" fallback="/settings" />
      <header class="page-header">
        <h1 class="large-title">{me ? deviceTitle(me) : "This iPhone"}</h1>
      </header>
      {content}
    </main>
  );
}

/** Settings › Notifications: turn them on, send a test, turn them off. */
export function NotificationsScreen() {
  const { me, error, reload } = useMe();
  return (
    <main class="page">
      <BackBar label="Settings" fallback="/settings" />
      <header class="page-header">
        <h1 class="large-title">Notifications</h1>
      </header>
      {me ? (
        <NotificationsSection me={me} reloadMe={reload} />
      ) : error ? (
        <ErrorState error={error} onRetry={reload} />
      ) : (
        <Loading />
      )}
    </main>
  );
}

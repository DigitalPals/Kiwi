import { useState } from "preact/hooks";
import { api } from "../api/client";
import type { AppleAccount, FamilyDevice, Person } from "../api/types";
import { Avatar, StepCell } from "../components/family";
import { Icon } from "../components/Icon";
import { BackBar, Banner, Cell, CellIcon, ErrorState, Loading, Section, Spinner } from "../components/ui";
import { modelName } from "../lib/family";
import { formatDate, formatRelative, plural } from "../lib/format";
import { useOnForeground, useResource, useTask } from "../lib/hooks";
import { useMe } from "../lib/me";
import { APPLE_PATH, FAMILY_PATH, familyDevicePath, personPath } from "../lib/routes";

/** "Ready", "Waiting", ... for a device's registration, with the tag's tone. */
export function deviceStatus(device: FamilyDevice): { label: string; tag: string } {
  const r = device.registration;
  switch (r.status) {
    case "pending":
      return { label: "Waiting for approval", tag: "tag tag-warn" };
    case "declined":
      return { label: "Declined", tag: "tag" };
    case "approved":
      return r.in_profile ? { label: "Ready", tag: "tag tag-tint" } : { label: "Registering", tag: "tag tag-warn" };
    default:
      // The owner's devices install apps without registering here.
      return r.ready ? { label: "Ready", tag: "tag tag-tint" } : { label: "Not registered", tag: "tag" };
  }
}

export function deviceLine(device: FamilyDevice): string {
  return [modelName(device.model), device.os_version && `iOS ${device.os_version}`].filter(Boolean).join(" · ");
}

function PersonRow({ person }: { person: Person }) {
  const owner = person.role === "owner";
  const pending = person.devices.find((d) => d.registration.status === "pending");
  const href = pending ? familyDevicePath(pending.id) : personPath(person.id);
  let sub: string;
  if (owner) sub = [plural(person.devices.length, "device"), "All apps", "Manages Kiwi"].join(" · ");
  else if (pending) sub = `${pending.name} is waiting for approval`;
  else {
    const devices = person.devices.length === 1 ? person.devices[0]?.name ?? "" : plural(person.devices.length, "device");
    sub = [devices, plural(person.app_count, "app")].filter(Boolean).join(" · ");
  }
  return (
    <a class="cell has-icon cell-link" href={href}>
      <Avatar name={person.name} />
      <div class="cell-main">
        <div class="cell-label">
          {person.name} {owner && <span class="tag">You</span>}
        </div>
        <div class="cell-sub">{sub}</div>
      </div>
      {pending && <span class="tag tag-tint">Approve</span>}
      <Icon name="chevron" size={16} class="cell-chevron" />
    </a>
  );
}

/** App Store Connect, the device count and the profile Kiwi signs with. */
export function AppleSection({ apple }: { apple: AppleAccount }) {
  const profile = apple.profile;
  return (
    <Section title="Apple Developer Account">
      <a class="cell has-icon cell-link" href={APPLE_PATH}>
        <CellIcon name="key" />
        <div class="cell-main">
          <div class="cell-label">App Store Connect</div>
          <div class="cell-sub">{apple.connected ? "Registers iPhones for you" : "Optional: lets Kiwi register iPhones for you"}</div>
        </div>
        {apple.connected ? <span class="tag tag-tint">Connected</span> : <span class="cell-action">Set Up</span>}
        <Icon name="chevron" size={16} class="cell-chevron" />
      </a>
      <Cell label="Registered Devices" icon="phone" sub="Slots free up when the membership renews">
        {apple.devices === null ? "—" : `${apple.devices} of ${apple.device_limit}`}
      </Cell>
      {profile && (
        <Cell
          label={profile.kiwi ? "Kiwi Profile" : "Signing Profile"}
          icon="doc"
          sub={`${profile.kiwi ? "" : `${profile.name} · `}${plural(profile.devices, "device")} · Expires ${formatDate(profile.expires_at)}`}
        />
      )}
      {apple.error && (
        <div class="cell">
          <p class="form-error">{apple.error}</p>
        </div>
      )}
    </Section>
  );
}

/** Settings › Family: everyone, their iPhones, and the Apple account. */
export function FamilyScreen() {
  const family = useResource("family", api.family);
  const { me } = useMe();
  useOnForeground(() => void family.reload());
  const host = me?.server.public_url.replace(/^https?:\/\//, "") ?? "Kiwi's address";

  let content;
  if (!family.data) {
    content = family.error ? <ErrorState error={family.error} onRetry={family.reload} /> : <Loading />;
  } else {
    const data = family.data;
    content = (
      <>
        <Section
          title="People"
          footer={`To add someone, share this Mac with them in Tailscale and send them ${host}. They register their iPhone there.`}
        >
          {data.people.map((p) => (
            <PersonRow key={p.id} person={p} />
          ))}
        </Section>
        <AppleSection apple={data.apple} />
      </>
    );
  }
  return (
    <main class="page">
      <BackBar label="Settings" fallback="/settings" />
      <header class="page-header">
        <h1 class="large-title">Family</h1>
      </header>
      {content}
    </main>
  );
}

function FieldCell(props: {
  label: string;
  value: string;
  placeholder: string;
  onInput: (value: string) => void;
}) {
  return (
    <div class="cell">
      <div class="cell-main cell-label">{props.label}</div>
      <div class="cell-value form-cell" style={{ flex: "0 1 60%" }}>
        <input
          type="text"
          value={props.value}
          aria-label={props.label}
          placeholder={props.placeholder}
          autocapitalize="off"
          autocorrect="off"
          spellcheck={false}
          onInput={(e) => props.onInput(e.currentTarget.value)}
        />
      </div>
    </div>
  );
}

function AppleSetup({ onSaved }: { onSaved: (account: AppleAccount) => void }) {
  const [issuer, setIssuer] = useState("");
  const [keyId, setKeyId] = useState("");
  const [file, setFile] = useState<{ name: string; text: string } | null>(null);
  const save = useTask(async () => {
    if (!file) throw new Error("Choose the .p8 file.");
    onSaved(await api.connectApple({ issuer_id: issuer.trim(), key_id: keyId.trim(), private_key: file.text }));
  });
  const choose = (input: HTMLInputElement) => {
    const chosen = input.files?.[0];
    if (!chosen) return;
    const reader = new FileReader();
    reader.onload = () => setFile({ name: chosen.name, text: String(reader.result ?? "") });
    reader.onerror = () => save.setError("Couldn't read that file.");
    reader.readAsText(chosen);
    // The file name is often AuthKey_<key id>.p8.
    const id = /^AuthKey_([A-Z0-9]+)\.p8$/i.exec(chosen.name)?.[1];
    if (id && !keyId) setKeyId(id.toUpperCase());
  };
  return (
    <>
      <p class="page-summary">
        With an API key, Kiwi registers family iPhones in your developer account and makes signing profiles that include them. Without
        one, you add each iPhone in the Apple Developer portal yourself.
      </p>
      <Section title="Create a Key" footer="Keys with the Admin role can register devices and create profiles.">
        <div class="cells steps">
          <StepCell
            n={1}
            state="todo"
            label={
              <a class="link" href="https://appstoreconnect.apple.com/access/integrations/api" target="_blank" rel="noreferrer">
                Open App Store Connect
              </a>
            }
            sub="Users and Access › Integrations › App Store Connect API › Team Keys"
          />
          <StepCell n={2} state="todo" label="Generate a key with the Admin role" />
          <StepCell n={3} state="todo" label="Download the .p8 file" sub="Apple lets you download it only once." />
        </div>
      </Section>
      <Section title="Key" footer="The key stays on the Mac, readable only by your account.">
        <FieldCell label="Issuer ID" value={issuer} placeholder="57246542-96fe-…" onInput={setIssuer} />
        <FieldCell label="Key ID" value={keyId} placeholder="2X9R4HXF34" onInput={setKeyId} />
        <div class="cell">
          <div class="cell-main">
            <div class="cell-label">Private Key</div>
            <div class="cell-sub">{file ? file.name : "The .p8 file"}</div>
          </div>
          <label class="text-button file-button">
            {file ? "Change" : "Choose File"}
            <input type="file" accept=".p8,text/plain,application/x-pem-file" onChange={(e) => choose(e.currentTarget)} />
          </label>
        </div>
      </Section>
      {save.error && <Banner level="error">{save.error}</Banner>}
      <button
        type="button"
        class="button button-block"
        onClick={() => void save.run()}
        disabled={save.pending || !issuer.trim() || !keyId.trim() || !file}
      >
        {save.pending ? <Spinner /> : "Save"}
      </button>
    </>
  );
}

/** Settings › Family › App Store Connect: set up, check or remove the API key. */
export function AppleScreen() {
  const apple = useResource("apple", api.apple);
  const sync = useTask(async () => apple.mutate(await api.syncApple()));
  const disconnect = useTask(async () => {
    const ok = window.confirm(
      "Disconnect App Store Connect?\n\nKiwi forgets the key. Registered iPhones and existing profiles stay in your developer account.",
    );
    if (ok) apple.mutate(await api.disconnectApple());
  });

  let content;
  const data = apple.data;
  if (!data) {
    content = apple.error ? <ErrorState error={apple.error} onRetry={apple.reload} /> : <Loading />;
  } else if (!data.connected) {
    content = <AppleSetup onSaved={apple.mutate} />;
  } else {
    content = (
      <>
        <Section title="Key" footer="Kiwi registers approved iPhones and makes a new profile whenever one is missing from it.">
          <Cell label="Key ID">
            <span class="mono">{data.key_id}</span>
          </Cell>
          <Cell label="Issuer ID">
            <span class="mono small">{data.issuer_id}</span>
          </Cell>
          <Cell label="Registered Devices">{data.devices === null ? "—" : `${data.devices} of ${data.device_limit}`}</Cell>
          <Cell label="Last Checked">{data.checked_at ? formatRelative(data.checked_at) : "—"}</Cell>
          {data.profile && (
            <Cell label={data.profile.kiwi ? "Kiwi Profile" : "Signing Profile"} sub={data.profile.name}>
              {plural(data.profile.devices, "device")}
            </Cell>
          )}
          {(data.error || sync.error) && (
            <div class="cell">
              <p class="form-error">{sync.error ?? data.error}</p>
            </div>
          )}
        </Section>
        <Section>
          <Cell label="Register and Refresh Profiles" icon="refresh">
            <button type="button" class="text-button" onClick={() => void sync.run()} disabled={sync.pending}>
              {sync.pending ? <Spinner /> : "Sync Now"}
            </button>
          </Cell>
          <Cell label="Disconnect" icon="close">
            <button type="button" class="text-button danger" onClick={() => void disconnect.run()} disabled={disconnect.pending}>
              {disconnect.pending ? <Spinner /> : "Disconnect"}
            </button>
          </Cell>
        </Section>
        {disconnect.error && <Banner level="error">{disconnect.error}</Banner>}
      </>
    );
  }
  return (
    <main class="page">
      <BackBar label="Family" fallback={FAMILY_PATH} />
      <header class="page-header">
        <h1 class="large-title">App Store Connect</h1>
      </header>
      {content}
    </main>
  );
}

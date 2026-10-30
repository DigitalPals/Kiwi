import { useEffect, useState } from "preact/hooks";
import { ApiError, api } from "../api/client";
import type { FamilyDevice, PersonDetail } from "../api/types";
import { Avatar, PersonApps, StepCell, type StepState } from "../components/family";
import { Icon } from "../components/Icon";
import { BackBar, Banner, Cell, CellIcon, Empty, ErrorState, Loading, Section, Spinner } from "../components/ui";
import { firstName, modelName, possessive } from "../lib/family";
import { formatDateTime, formatRelative, plural } from "../lib/format";
import { useOnForeground, useResource, useTask } from "../lib/hooks";
import { navigate } from "../lib/router";
import { APPLE_PATH, FAMILY_PATH, familyDevicePath, personPath } from "../lib/routes";
import { deviceLine, deviceStatus } from "./Family";

function NotFound({ what }: { what: string }) {
  return (
    <Empty icon="people" title={`${what} Not Found`} message="They may have been removed from Kiwi.">
      <a class="button" href={FAMILY_PATH}>
        Family
      </a>
    </Empty>
  );
}

function DeviceRow({ device }: { device: FamilyDevice }) {
  const status = deviceStatus(device);
  return (
    <a class="cell has-icon cell-link" href={familyDevicePath(device.id)}>
      <CellIcon name="phone" />
      <div class="cell-main">
        <div class="cell-label">{device.name}</div>
        <div class="cell-sub">{deviceLine(device)}</div>
      </div>
      <span class={status.tag}>{status.label}</span>
      <Icon name="chevron" size={16} class="cell-chevron" />
    </a>
  );
}

/** "•••": rename or remove a member. */
function PersonMenu(props: { person: PersonDetail; onChange: (person: PersonDetail) => void }) {
  const { person } = props;
  const [open, setOpen] = useState(false);
  const rename = useTask(async () => {
    setOpen(false);
    const name = window.prompt("Name", person.name)?.trim();
    if (name && name !== person.name) props.onChange(await api.updatePerson(person.id, { name }));
  });
  const remove = useTask(async () => {
    setOpen(false);
    const first = firstName(person.name);
    const ok = window.confirm(
      `Remove ${person.name}?\n\nKiwi forgets ${possessive(person.name)} apps and iPhone registration. Installed apps keep working until they expire, and the iPhone stays in your Apple developer account. ${first} can register again later.`,
    );
    if (!ok) return;
    await api.removePerson(person.id);
    navigate(FAMILY_PATH, { replace: true });
  });
  const pending = rename.pending || remove.pending;
  return (
    <div class="menu-anchor">
      <button
        type="button"
        class={open ? "more-button open" : "more-button"}
        aria-label="More"
        aria-haspopup="menu"
        aria-expanded={open}
        onClick={() => setOpen(!open)}
        disabled={pending}
      >
        {pending ? <Spinner /> : <Icon name="more" size={20} />}
      </button>
      {open && (
        <>
          <div class="menu-scrim" onClick={() => setOpen(false)} />
          <div class="menu" role="menu">
            <button type="button" role="menuitem" class="menu-item" onClick={() => void rename.run()}>
              <span>Rename</span>
              <Icon name="sliders" size={19} />
            </button>
            <button type="button" role="menuitem" class="menu-item danger" onClick={() => void remove.run()}>
              <span>Remove {firstName(person.name)}</span>
              <Icon name="close" size={19} />
            </button>
          </div>
        </>
      )}
      {(rename.error || remove.error) && <p class="menu-error">{rename.error ?? remove.error}</p>}
    </div>
  );
}

/** Settings › Family › a person: their iPhones and the apps they may use. */
export function PersonScreen({ personId }: { personId: string }) {
  const person = useResource(`person/${personId}`, () => api.person(personId));
  useOnForeground(() => void person.reload());
  const data = person.data;

  let content;
  if (!data) {
    content =
      person.error instanceof ApiError && person.error.status === 404 ? (
        <NotFound what="Person" />
      ) : person.error ? (
        <ErrorState error={person.error} onRetry={person.reload} />
      ) : (
        <Loading />
      );
  } else {
    const owner = data.role === "owner";
    const first = firstName(data.name);
    content = (
      <>
        <header class="person-header">
          <Avatar name={data.name} size={60} />
          <div>
            <h1>{data.name}</h1>
            <p>
              {data.id} · {owner ? "Owner" : "Member"}
            </p>
          </div>
        </header>
        <Section title={data.devices.length === 1 ? "iPhone" : "Devices"}>
          {data.devices.map((d) => (
            <DeviceRow key={d.id} device={d} />
          ))}
          {data.devices.length === 0 && <Cell label="No devices" sub={`${first} hasn't opened Kiwi on an iPhone yet.`} />}
        </Section>
        {owner ? (
          <Section title="Apps" footer="As the owner you always have every app.">
            <Cell label="All Apps" icon="apps">
              {plural(data.app_count, "app")}
            </Cell>
          </Section>
        ) : (
          <PersonApps
            person={data}
            onChange={person.mutate}
            newApps
            footer={`Turning an app off hides it from ${first} and stops its updates. An installed copy keeps working until its signature expires.`}
          />
        )}
      </>
    );
  }
  return (
    <main class="page">
      <BackBar
        label="Family"
        fallback={FAMILY_PATH}
        trailing={data && data.role === "member" && <PersonMenu person={data} onChange={person.mutate} />}
      />
      {content}
    </main>
  );
}

/** "Copy" that says "Copied" for a moment. */
function CopyButton({ text }: { text: string }) {
  const [copied, setCopied] = useState(false);
  useEffect(() => {
    if (!copied) return;
    const timer = window.setTimeout(() => setCopied(false), 2000);
    return () => window.clearTimeout(timer);
  }, [copied]);
  return (
    <button
      type="button"
      class="text-button small"
      onClick={() => void navigator.clipboard?.writeText(text).then(() => setCopied(true))}
    >
      <Icon name={copied ? "check" : "copy"} size={15} strokeWidth={2.2} />
      {copied ? "Copied" : "Copy"}
    </button>
  );
}

function DeviceCard({ device, person }: { device: FamilyDevice; person: PersonDetail | null }) {
  const r = device.registration;
  const tags = [
    modelName(r.product ?? device.model),
    device.os_version && `iOS ${device.os_version}`,
    r.requested_at && formatRelative(r.requested_at),
  ].filter(Boolean);
  return (
    <div class="about-card">
      <Avatar name={person?.name ?? device.name} size={58} />
      <div class="about-text">
        <div class="about-name">{device.name}</div>
        <div class="about-host">
          {r.status === "pending" ? `${person?.id ?? "Someone"} wants to use Kiwi` : (person?.name ?? device.person_id ?? "")}
        </div>
        <div class="about-status">
          {tags.map((t) => (
            <span key={t} class="tag">
              {t}
            </span>
          ))}
        </div>
      </div>
    </div>
  );
}

/** Settings › Family › a device: approve a new iPhone, or see where its registration stands. */
export function FamilyDeviceScreen({ deviceId }: { deviceId: string }) {
  const device = useResource(`family-device/${deviceId}`, () => api.familyDevice(deviceId));
  const personId = device.data?.person_id ?? null;
  const person = useResource<PersonDetail | null>(`person/${personId ?? ""}`, () =>
    personId ? api.person(personId) : Promise.resolve(null),
  );
  const apple = useResource("apple", api.apple);
  const reloadAll = () => {
    void device.reload();
    void apple.reload();
  };
  useOnForeground(reloadAll);

  const data = device.data;
  const r = data?.registration;
  // Kiwi registers with Apple in the background after approval.
  const registering = !!r && r.status === "approved" && !r.in_profile && !!apple.data?.connected && !r.error;
  useEffect(() => {
    if (!registering) return;
    const timer = window.setInterval(() => void device.reload(), 4000);
    return () => window.clearInterval(timer);
  }, [registering, device.reload]);

  const approve = useTask(async () => {
    device.mutate(await api.approveDevice(deviceId));
    void apple.reload();
  });
  const decline = useTask(async () => {
    const ok = window.confirm(`Decline ${data?.name ?? "this iPhone"}?\n\nIt can't install apps. It can ask again later.`);
    if (ok) device.mutate(await api.declineDevice(deviceId));
  });
  const retry = useTask(async () => device.mutate(await api.registerDevice(deviceId)));
  const rescan = useTask(async () => {
    await api.rescanSigning();
    await device.reload();
  });
  const forget = useTask(async () => {
    const ok = window.confirm(
      `Remove ${data?.name ?? "this iPhone"} from Kiwi?\n\nIt has to register again before it can install apps. Installed apps keep working until they expire.`,
    );
    if (!ok) return;
    await api.forgetRegistration(deviceId);
    navigate(personId ? personPath(personId) : FAMILY_PATH, { replace: true });
  });

  const owner = person.data?.role === "owner";
  let content;
  let trailing = null;
  if (!data || !r) {
    content =
      device.error instanceof ApiError && device.error.status === 404 ? (
        <NotFound what="Device" />
      ) : device.error ? (
        <ErrorState error={device.error} onRetry={device.reload} />
      ) : (
        <Loading />
      );
  } else if (r.status === "pending") {
    trailing = (
      <button type="button" class="text-button nav-danger" onClick={() => void decline.run()} disabled={decline.pending}>
        Decline
      </button>
    );
    const p = person.data ?? null;
    const account = apple.data;
    content = (
      <>
        <DeviceCard device={data} person={p} />
        {p ? (
          <PersonApps
            person={p}
            onChange={person.mutate}
            footer={`${firstName(p.name)} only sees the apps you turn on. You can change this later in Settings › Family.`}
          />
        ) : (
          <Loading />
        )}
        {(approve.error || decline.error) && <Banner level="error">{approve.error ?? decline.error}</Banner>}
        <button type="button" class="button button-block" onClick={() => void approve.run()} disabled={approve.pending}>
          {approve.pending ? <Spinner /> : "Approve and Register"}
        </button>
        <p class="fine-print">
          {account?.connected
            ? `Registers this iPhone with Apple: ${(account.devices ?? 0) + 1} of ${account.device_limit} devices this membership year.`
            : "Next, add it to your Apple developer account; Kiwi shows how."}
        </p>
      </>
    );
  } else {
    const approved = r.status === "approved";
    const step3: StepState = r.in_profile ? "done" : approved ? "busy" : "off";
    content = (
      <>
        <DeviceCard device={data} person={person.data ?? null} />
        {r.status === "none" ? (
          <Banner level="info">
            {owner
              ? "Kiwi doesn't know this iPhone's ID. It installs apps as long as the profile Kiwi signs with includes it."
              : "This iPhone hasn't registered. It can register when Kiwi is opened on it."}
          </Banner>
        ) : (
          <Section title="Registration">
            <div class="cells steps">
              <StepCell n={1} state="done" label="ID received" sub={r.requested_at && formatRelative(r.requested_at)} />
              <StepCell
                n={2}
                state={approved ? "done" : "todo"}
                label={approved ? "Approved" : "Declined"}
                sub={r.decided_at && formatRelative(r.decided_at)}
              />
              <StepCell
                n={3}
                state={step3}
                label={r.in_profile ? "In the signing profile" : "Registered with Apple"}
                sub={r.in_profile ? "Ready to install apps" : approved ? "Waiting for a profile that includes it" : undefined}
              />
            </div>
          </Section>
        )}
        {r.status === "declined" && (
          <Banner level="warning">
            You declined this iPhone.{" "}
            <button type="button" class="text-button small" onClick={() => void approve.run()} disabled={approve.pending}>
              Approve Instead
            </button>
          </Banner>
        )}
        {approved && !r.in_profile && apple.data?.connected && (
          <Section>
            {r.error ? (
              <div class="cell">
                <p class="form-error">{r.error}</p>
              </div>
            ) : (
              <Cell label="Registering with Apple…" sub="Kiwi adds the iPhone to your developer account and makes a profile.">
                <Spinner />
              </Cell>
            )}
            <Cell label="Register Again" icon="refresh">
              <button type="button" class="text-button" onClick={() => void retry.run()} disabled={retry.pending}>
                {retry.pending ? <Spinner /> : "Try Again"}
              </button>
            </Cell>
          </Section>
        )}
        {approved && !r.in_profile && apple.data && !apple.data.connected && (
          <Section
            title="Add It in Apple Developer"
            footer={
              <>
                Or let Kiwi do this: <a href={APPLE_PATH}>set up App Store Connect</a>.
              </>
            }
          >
            <div class="cells steps">
              <StepCell n={1} state="todo" label="Copy the UDID" sub={<span class="mono small">{r.udid}</span>} />
              <StepCell
                n={2}
                state="todo"
                label={
                  <a class="link" href="https://developer.apple.com/account/resources/devices/add" target="_blank" rel="noreferrer">
                    Register it under Devices
                  </a>
                }
                sub="Certificates, Identifiers & Profiles › Devices › +"
              />
              <StepCell
                n={3}
                state="todo"
                label="Add it to a development profile"
                sub="Edit the wildcard profile (or let Xcode refresh its own), then download it in Xcode › Settings › Accounts."
              />
              <div class="cell">
                <span class="step-num">4</span>
                <div class="cell-main cell-label">Rescan</div>
                <button type="button" class="text-button" onClick={() => void rescan.run()} disabled={rescan.pending}>
                  {rescan.pending ? <Spinner /> : "Rescan"}
                </button>
              </div>
            </div>
          </Section>
        )}
        {(approve.error || retry.error || rescan.error || forget.error) && (
          <Banner level="error">{approve.error ?? retry.error ?? rescan.error ?? forget.error}</Banner>
        )}
        <Section title="Details">
          {r.udid && (
            <Cell label="UDID" sub={<span class="mono small">{r.udid}</span>}>
              <CopyButton text={r.udid} />
            </Cell>
          )}
          <Cell label="Model">{modelName(r.product ?? data.model)}</Cell>
          {data.os_version && <Cell label="iOS">{data.os_version}{r.os_build && ` (${r.os_build})`}</Cell>}
          <Cell label="Last Seen">{formatDateTime(data.last_seen)}</Cell>
        </Section>
        {r.status !== "none" && (
          <Section footer="Installed apps keep working until they expire. The iPhone stays in your Apple developer account.">
            <button type="button" class="cell cell-button" onClick={() => void forget.run()} disabled={forget.pending}>
              <div class="cell-main cell-label danger">Remove from Kiwi</div>
              {forget.pending && <Spinner />}
            </button>
          </Section>
        )}
      </>
    );
  }
  return (
    <main class="page">
      <BackBar label="Family" fallback={personId ? personPath(personId) : FAMILY_PATH} trailing={trailing} />
      {content}
    </main>
  );
}

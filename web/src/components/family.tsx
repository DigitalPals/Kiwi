import type { ComponentChildren } from "preact";
import { useEffect, useMemo, useRef, useState } from "preact/hooks";
import { api, errorMessage } from "../api/client";
import type { AppSummary, PersonApp, PersonDetail } from "../api/types";
import { featuredKeys, firstName, personAppKey } from "../lib/family";
import { plural } from "../lib/format";
import { AppIcon } from "./AppIcon";
import { Icon } from "./Icon";
import { CellIcon, Spinner, Toggle } from "./ui";

/** A person's initial in a grey circle. */
export function Avatar(props: { name: string; size?: number }) {
  const size = props.size ?? 32;
  return (
    <span
      class="avatar"
      style={{
        width: `${size}px`,
        height: `${size}px`,
        fontSize: `${Math.round(size * 0.45)}px`,
      }}
      aria-hidden="true"
    >
      {firstName(props.name).charAt(0).toUpperCase() || "?"}
    </span>
  );
}

export type StepState = "done" | "busy" | "todo" | "off";

/** One numbered step: a number, a check when done, or a spinner while waiting. */
export function StepCell(props: { n: number; state: StepState; label: ComponentChildren; sub?: ComponentChildren }) {
  const { state } = props;
  return (
    <div class={state === "off" ? "cell off" : "cell"}>
      <span class={`step-num${state === "done" ? " done" : state === "busy" ? " busy" : state === "off" ? " off" : ""}`}>
        {state === "done" ? <Icon name="check" size={15} strokeWidth={3} /> : state === "busy" ? <Spinner /> : props.n}
      </span>
      <div class="cell-main">
        <div class="cell-label">{props.label}</div>
        {props.sub && <div class="cell-sub">{props.sub}</div>}
      </div>
    </div>
  );
}

/** How to turn on Developer Mode, which iOS needs once before development-signed apps open. */
export function DevModeSheet(props: { app?: Pick<AppSummary, "name" | "state"> | null; onClose: () => void }) {
  const { app, onClose } = props;
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if (e.key === "Escape") onClose();
    };
    document.addEventListener("keydown", onKey);
    return () => document.removeEventListener("keydown", onKey);
  }, [onClose]);
  const name = app?.name ?? "an app from Kiwi";
  return (
    <div class="sheet-backdrop" onClick={onClose}>
      <div class="sheet" role="dialog" aria-modal="true" aria-labelledby="devmode-title" onClick={(e) => e.stopPropagation()}>
        <header class="sheet-header">
          <h2 id="devmode-title">Turn On Developer Mode</h2>
          <button type="button" class="text-button" onClick={onClose}>
            Done
          </button>
        </header>
        <p class="sheet-lead">iOS needs this once before apps from Kiwi can open. Updates don't ask again.</p>
        <div class="cells steps sheet-steps">
          <StepCell n={1} state="todo" label={`Open ${name}`} sub="iOS says “Developer Mode Required”. Tap OK." />
          <StepCell n={2} state="todo" label="Open the Settings app" sub="Privacy & Security › Developer Mode" />
          <StepCell n={3} state="todo" label="Turn it on and tap Restart" />
          <StepCell n={4} state="todo" label="After the restart, tap Turn On" sub="and enter your passcode" />
        </div>
        {app?.state.open_url && (
          <a class="button button-block" href={app.state.open_url}>
            Open {app.name}
          </a>
        )}
      </div>
    </div>
  );
}

function PersonAppCell(props: { app: PersonApp; onToggle: (app: PersonApp, allowed: boolean) => void }) {
  const { app } = props;
  const sub = app.installed_version ? `Installed · ${app.installed_version}` : app.source_name;
  return (
    <div class={app.allowed ? "cell has-icon pick" : "cell has-icon pick off"}>
      <AppIcon url={app.icon_url} name={app.name} tint={app.tint_color} size={32} />
      <div class="cell-main">
        <div class="cell-label one-line">{app.name}</div>
        <div class="cell-sub">{sub}</div>
      </div>
      <Toggle checked={app.allowed} onChange={(on) => props.onToggle(app, on)} label={`Allow ${app.name}`} />
    </div>
  );
}

/**
 * Switches for the apps a member may use, the ones they use (or have installed) first;
 * `newApps` adds the "Apps You Add Later" switch. Changes are saved one at a time.
 */
export function PersonApps(props: {
  person: PersonDetail;
  onChange: (person: PersonDetail) => void;
  newApps?: boolean;
  footer?: ComponentChildren;
}) {
  const { person, onChange } = props;
  const [error, setError] = useState<string | null>(null);
  const [expanded, setExpanded] = useState(false);
  const [savingNew, setSavingNew] = useState(false);
  // Fixed when the person is loaded so rows don't move while switching them.
  const featured = useRef<{ id: string; keys: Set<string> } | null>(null);
  if (featured.current?.id !== person.id) featured.current = { id: person.id, keys: featuredKeys(person.apps) };
  const keys = featured.current.keys;
  const latest = useRef(person);
  latest.current = person;

  const { first, rest } = useMemo(() => {
    const first: PersonApp[] = [];
    const rest: PersonApp[] = [];
    for (const app of person.apps) (keys.has(personAppKey(app)) ? first : rest).push(app);
    return { first, rest };
  }, [person.apps, keys]);
  // Nobody picked yet: a few apps to start with.
  const shown = expanded ? [...first, ...rest] : first.length > 0 ? first : rest.slice(0, 5);
  const hiddenCount = person.apps.length - shown.length;

  const toggle = async (app: PersonApp, allowed: boolean) => {
    setError(null);
    const set = (value: boolean, explicit: boolean) => {
      const current = latest.current;
      const apps = current.apps.map((a) => (personAppKey(a) === personAppKey(app) ? { ...a, allowed: value, explicit } : a));
      onChange({ ...current, apps, app_count: apps.filter((a) => a.allowed).length });
    };
    set(allowed, true);
    try {
      onChange(await api.setPersonApps(person.id, [{ source_id: app.source_id, bundle_id: app.bundle_id, allowed }]));
    } catch (e) {
      set(app.allowed, app.explicit);
      setError(errorMessage(e));
    }
  };
  const setNewApps = async (on: boolean) => {
    setError(null);
    setSavingNew(true);
    onChange({ ...latest.current, new_apps: on });
    try {
      onChange(await api.updatePerson(person.id, { new_apps: on }));
    } catch (e) {
      onChange({ ...latest.current, new_apps: !on });
      setError(errorMessage(e));
    } finally {
      setSavingNew(false);
    }
  };

  const name = firstName(person.name);
  return (
    <section class="section">
      <h2 class="section-title">
        Apps for {name} · {person.app_count} of {person.apps.length}
      </h2>
      <div class="cells">
        {props.newApps && (
          <div class="cell has-icon">
            <CellIcon name="plus" />
            <div class="cell-main">
              <div class="cell-label">Apps You Add Later</div>
              <div class="cell-sub">
                {person.new_apps ? `New apps are allowed for ${name} automatically` : `Off: you choose for ${name} each time`}
              </div>
            </div>
            <Toggle checked={person.new_apps} onChange={(on) => void setNewApps(on)} label="Allow apps you add later" disabled={savingNew} />
          </div>
        )}
        {shown.map((app) => (
          <PersonAppCell key={personAppKey(app)} app={app} onToggle={(a, on) => void toggle(a, on)} />
        ))}
        {person.apps.length === 0 && (
          <div class="cell">
            <div class="cell-main cell-sub">No apps yet. Add a source first.</div>
          </div>
        )}
        {hiddenCount > 0 && (
          <button type="button" class="cell has-icon cell-button" onClick={() => setExpanded(true)}>
            <CellIcon name="apps" />
            <div class="cell-main cell-label">Show All {plural(person.apps.length, "App")}</div>
            <Icon name="chevron" size={16} class="cell-chevron" />
          </button>
        )}
      </div>
      {error ? <div class="section-footer danger">{error}</div> : props.footer && <div class="section-footer">{props.footer}</div>}
    </section>
  );
}

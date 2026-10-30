import { useEffect, useState } from "preact/hooks";
import { api } from "../api/client";
import type { AutomationPreferences, AutomationState } from "../api/types";
import { AppIcon } from "../components/AppIcon";
import { BackBar, Banner, Cell, ErrorState, Loading, Section, Spinner, Toggle } from "../components/ui";
import {
  activeJob, automationSummary, builtAppPath, heroLine, intervalLabel, JOB_LABELS, limitationIcon, samePreferences,
  selectedFeatures,
} from "../lib/automation";
import { formatDateTime, formatRelative, formatVersion } from "../lib/format";
import { useOnForeground, useResource, useTask } from "../lib/hooks";
import { BUILDS_PATH } from "../lib/routes";
import { ExperimentalTag, SetupCells } from "./Builds";

type Edit = (patch: Partial<AutomationPreferences>) => void;

function Hero({ data }: { data: AutomationState }) {
  return (
    <div class="build-hero">
      <AppIcon url={data.icon_url} name={data.name} size={64} />
      <div class="build-hero-text">
        <h1 class="build-hero-title">{data.name} {data.experimental && <ExperimentalTag />}</h1>
        <p class="cell-sub">{heroLine(data)}</p>
      </div>
    </div>
  );
}

function Limitations({ data, title }: { data: AutomationState; title: string }) {
  return (
    <Section title={title}>
      {data.limitations.map((l) => (
        <Cell key={l.title} label={l.title} sub={l.detail} icon={limitationIcon(l.icon)} />
      ))}
    </Section>
  );
}

/** App name (where there is a choice), the always-included tweak and the optional ones. */
function Options({ data, preferences, edit, disabled, footer }: {
  data: AutomationState;
  preferences: AutomationPreferences;
  edit: Edit;
  disabled: boolean;
  footer?: string;
}) {
  if (data.names.length === 0 && data.included.length === 0 && data.available_tweaks.length === 0) return null;
  return (
    <Section title="Patches" footer={footer}>
      {data.names.length > 0 && (
        <Cell label="App Name" sub={`${data.names[0]} restores the classic look`}>
          <div class="segmented build-names" role="radiogroup" aria-label="App name">
            {data.names.map((name) => (
              <button key={name} type="button" role="radio" aria-checked={preferences.name === name} disabled={disabled}
                class={preferences.name === name ? "segment active" : "segment"} onClick={() => edit({ name })}>
                {name}
              </button>
            ))}
          </div>
        </Cell>
      )}
      {data.included.map((f) => <Cell key={f.id} label={f.name} sub={f.description}>Included</Cell>)}
      {data.available_tweaks.map((tweak) => (
        <Cell key={tweak.id} label={tweak.name} sub={tweak.description}>
          <Toggle label={tweak.name} checked={preferences.tweaks.includes(tweak.id)} disabled={disabled}
            onChange={(on) => edit({
              tweaks: on ? [...preferences.tweaks, tweak.id] : preferences.tweaks.filter((t) => t !== tweak.id),
            })} />
        </Cell>
      ))}
    </Section>
  );
}

/** Before an app is added: what changes in the patched app, its options, then Add. */
function AddApp({ data, onAdded }: { data: AutomationState; onAdded: (state: AutomationState) => void }) {
  const [preferences, setPreferences] = useState(data.preferences);
  const add = useTask(async () => onAdded(await api.addAutomation(data.id, preferences)));
  const edit: Edit = (patch) => setPreferences({ ...preferences, ...patch });
  return (
    <>
      {data.warning && <Banner level="warning">{data.warning}</Banner>}
      {add.error && <Banner level="error">{add.error}</Banner>}
      <Limitations data={data} title="Before You Add" />
      {(data.names.length > 0 || data.available_tweaks.length > 0) && (
        <Options data={data} preferences={preferences} edit={edit} disabled={add.pending}
          footer="You can change these later. Changes apply to the next build." />
      )}
      <button class="button build-add" type="button" disabled={add.pending} onClick={() => void add.run()}>
        {add.pending ? <Spinner /> : `Add ${data.name} to Kiwi Builds`}
      </button>
      <p class="build-caption">
        Builds and signs {data.name} on your Mac, then checks for updates {intervalLabel(preferences.interval_minutes).toLowerCase()}.
      </p>
    </>
  );
}

function Added({ data, state }: { data: AutomationState; state: ReturnType<typeof useAutomation> }) {
  const [draft, setDraft] = useState<AutomationPreferences | null>(null);
  const check = useTask(async (force: boolean) => {
    state.mutate(await api.checkAutomation(data.id, force));
    void state.reload();
  });
  const cancel = useTask(async () => state.mutate(await api.cancelAutomation(data.id)));
  const save = useTask(async (preferences: AutomationPreferences) => {
    state.mutate(await api.updateAutomation(data.id, preferences));
    setDraft(null);
    void state.reload();
  });
  const showSource = useTask(async () => {
    await api.updateSource("kiwi-youtube", { enabled: true });
    await state.reload();
  });
  const remove = useTask(async () => {
    const ok = window.confirm(
      `Remove ${data.name} from Kiwi Builds?\n\nInstalled copies keep working until they expire, but won't get updates.`,
    );
    if (ok) state.mutate(await api.removeAutomation(data.id));
  });
  const job = activeJob(data);
  const active = !!job;
  const preferences = draft ?? data.preferences;
  const dirty = !samePreferences(preferences, data.preferences);
  const edit: Edit = (patch) => setDraft({ ...preferences, ...patch });
  const error = check.error ?? save.error ?? cancel.error ?? showSource.error ?? remove.error;
  const intervals = [...new Set([15, 30, 60, 180, 360, 720, 1440, preferences.interval_minutes])].sort((a, b) => a - b);
  const published = data.published;
  const busy = save.pending || check.pending || cancel.pending || remove.pending;
  return (
    <>
      <p class="page-summary">{automationSummary(data)}</p>
      {error && <Banner level="error">{error}</Banner>}
      {dirty && <Banner level="info">You have unsaved settings. Save Changes below before starting a build.</Banner>}

      <Section title="Available in Kiwi" footer="New builds appear in Kiwi Builds. Install updates from the app page or My Apps.">
        {published ? <>
          <Cell label={data.name} sub={formatVersion({ ...published, revision: published.revision ?? undefined })}>
            <a class="link" href={builtAppPath(data)}>View App</a>
          </Cell>
          <Cell label="Built">{formatDateTime(published.date)}</Cell>
          {published.tweaks && (data.included.length > 0 || data.available_tweaks.length > 0) && <Cell label="Included Features" sub={selectedFeatures(data, published.tweaks, published.name)} />}
        </> : <Cell label="No build yet" sub={active ? "The first build is on its way." : "Choose Build Now to start the first build."} />}
        {data.source_enabled === false && <Cell label="Kiwi Builds is hidden" sub="Show this source to browse and install its builds.">
          <button class="text-button" type="button" disabled={showSource.pending} onClick={() => void showSource.run()}>Show</button>
        </Cell>}
      </Section>

      {data.configuration_changed && <Banner level="info">
        Your saved patch choices differ from the available build. {active ? "The current job uses the choices saved when it started." : "Choose Build Changes to apply them now."}
      </Banner>}

      <Section title="Build Status" footer="Kiwi keeps working while this screen is closed. If a build fails, the previous release stays available.">
        <Cell label={job ? job.stage : `Look for a New ${data.name} Version`} sub={job?.version ? `${data.name} ${job.version}` : undefined}>
          {active ? <span class="build-actions">{job.status === "running" && <Spinner />}<button class="text-button danger" type="button" disabled={cancel.pending} onClick={() => void cancel.run()}>Cancel</button></span>
            : <button class="text-button" type="button" disabled={busy || dirty} onClick={() => void check.run(false)}>
              {check.pending ? <Spinner /> : !published ? "Build Now" : data.configuration_changed ? "Build Changes" : "Check Now"}
            </button>}
        </Cell>
        {!active && published && <Cell label="Rebuild Current Recipe" sub="Create a fresh build even when nothing has changed.">
          <button class="text-button" type="button" disabled={busy || dirty} onClick={() => void check.run(true)}>Rebuild</button>
        </Cell>}
        <Cell label="Last Checked">{data.last_checked_at ? formatRelative(data.last_checked_at) : "Never"}</Cell>
        <Cell label="Next Automatic Check">
          {active ? "After this job" : data.next_check_at ? formatDateTime(data.next_check_at) : "Off"}
        </Cell>
      </Section>

      <form onSubmit={(e) => { e.preventDefault(); if (dirty && !save.pending) void save.run(preferences); }}>
        <Section title="Schedule" footer={`The Mac checks for new ${data.name} versions, builds with your saved choices and signs the result. Installation still needs your confirmation on the iPhone.`}>
          <Cell label="Build Updates Automatically">
            <Toggle checked={preferences.enabled} onChange={(enabled) => edit({ enabled })} label={`Build ${data.name} updates automatically`} disabled={save.pending} />
          </Cell>
          <Cell label="Check for Updates">
            <select class="select" aria-label={`${data.name} check interval`} value={preferences.interval_minutes} disabled={save.pending}
              onChange={(e) => edit({ interval_minutes: Number(e.currentTarget.value) })}>
              {intervals.map((minutes) => <option key={minutes} value={minutes}>{intervalLabel(minutes)}</option>)}
            </select>
          </Cell>
        </Section>

        <Options data={data} preferences={preferences} edit={edit} disabled={save.pending}
          footer={active ? "Changes apply to the next job. The running job keeps its original choices." : "Save your choices, then build them now or let the next scheduled check apply them."} />
        <div class="build-save" aria-live="polite">
          <span class="muted small">{dirty ? "Unsaved changes" : "Settings saved"}</span>
          <span class="build-actions">
            {dirty && <button class="text-button" type="button" disabled={save.pending} onClick={() => setDraft(null)}>Discard</button>}
            <button class="button" type="submit" disabled={!dirty || save.pending}>{save.pending ? <Spinner /> : "Save Changes"}</button>
          </span>
        </div>
      </form>

      <Limitations data={data} title="Known Limitations" />

      <Section title="Mac Setup" footer="Requirements depend on the app. Choose Check Now or Build Now above to refresh these checks.">
        <SetupCells checks={data.checks} />
      </Section>

      <Section title="Build History">
        {data.jobs.length === 0 && <Cell label="No jobs yet" sub="Start a build after saving your settings." />}
        {data.jobs.map((job) => <div class="cell cell-stacked" key={job.id}>
          <div class="cell-label">{job.version ? `${data.name} ${job.version}` : `${data.name} check`} <span class={`tag ${job.status === "ready" ? "tag-tint" : ""}`}>{JOB_LABELS[job.status]}</span></div>
          <div class="cell-sub" role={job.finished_at ? undefined : "status"}>{job.stage}{job.status === "ready" ? ` · Kiwi ${job.sequence}` : ""}</div>
          <div class="cell-sub">{formatDateTime(job.created_at)}</div>
          {job.error && <div class="cell-sub danger">{job.error}</div>}
          {job.preferences && (data.names.length > 0 || data.included.length > 0 || data.available_tweaks.length > 0) && <details class="cell-details"><summary class="cell-sub">Included features</summary>
            <div class="cell-sub">{selectedFeatures(data, job.preferences.tweaks, job.preferences.name)}</div>
          </details>}
        </div>)}
      </Section>

      <p class="build-remove">
        <button class="text-button danger" type="button" disabled={busy || active} onClick={() => void remove.run()}>
          Remove from Kiwi Builds
        </button>
      </p>
    </>
  );
}

function useAutomation(app: string) {
  return useResource(`automation:${app}`, () => api.automation(app));
}

export function Automation({ app }: { app: string }) {
  const state = useAutomation(app);
  const data = state.data;
  const active = !!data && !!activeJob(data);
  useOnForeground(() => void state.reload());
  useEffect(() => {
    const timer = window.setInterval(() => {
      if (document.visibilityState === "visible") void state.reload();
    }, active ? 2000 : 30_000);
    return () => window.clearInterval(timer);
  }, [active, state.reload]);

  return (
    <main class="page">
      <BackBar label="Kiwi Builds" fallback={BUILDS_PATH} />
      {!data ? (state.error ? <ErrorState error={state.error} onRetry={state.reload} /> : <Loading />) : <>
        <Hero data={data} />
        {data.added ? <Added key={data.id} data={data} state={state} /> : <AddApp key={data.id} data={data} onAdded={state.mutate} />}
      </>}
    </main>
  );
}

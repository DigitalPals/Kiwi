import { useState } from "preact/hooks";
import { api, errorMessage } from "../api/client";
import type { AppSummary, BrowseMode, Source } from "../api/types";
import { AppIcon } from "../components/AppIcon";
import { BackBar, Banner, Cell, Empty, ErrorState, LinkCell, Loading, Section, Spinner, Toggle } from "../components/ui";
import { appKey } from "../lib/action";
import { formatRelative, plural } from "../lib/format";
import { useResource, useTask } from "../lib/hooks";
import { useMe } from "../lib/me";
import { navigate } from "../lib/router";
import { BUILDS_PATH, appPath } from "../lib/routes";

const MODES: { mode: BrowseMode; label: string; footer: string }[] = [
  {
    mode: "all",
    label: "All Apps",
    footer: "Browse shows every app from this source except the ones you turn off.",
  },
  {
    mode: "picked",
    label: "Apps I Pick",
    footer: "Only the apps you turn on appear in Browse. New apps from this source wait here, marked New.",
  },
];

function AppCell(props: { app: AppSummary; onToggle: (app: AppSummary, shown: boolean) => void }) {
  const { app } = props;
  const sub = app.original ? `Based on ${app.original.short_name}` : (app.subtitle ?? app.developer ?? app.bundle_id);
  return (
    <div class={app.hidden ? "cell has-icon pick off" : "cell has-icon pick"}>
      <a class="pick-link" href={appPath(app.source_id, app.bundle_id)}>
        <AppIcon url={app.icon_url} name={app.name} tint={app.tint_color} size={40} />
        <div class="cell-main">
          <div class="cell-label one-line">
            {app.name}
            {app.is_new && <span class="tag tag-tint tag-new">NEW</span>}
          </div>
          <div class="cell-sub">{sub}</div>
        </div>
      </a>
      <Toggle checked={!app.hidden} onChange={(on) => props.onToggle(app, on)} label={`Show ${app.name} in Browse`} />
    </div>
  );
}

/** On or off, the URL, refreshing and the source's warnings. */
function SourceSettings({ source, onChange }: { source: Source; onChange: (source: Source) => void }) {
  const local = source.kind === "local_build";
  const [optimistic, setOptimistic] = useState<boolean | null>(null);
  const toggle = useTask(async (enabled: boolean) => {
    setOptimistic(enabled);
    try {
      onChange(await api.updateSource(source.id, { enabled }));
    } finally {
      setOptimistic(null);
    }
  });
  const refresh = useTask(async () => onChange(await api.refreshSource(source.id)));
  const error = toggle.error ?? refresh.error;
  const checked = source.last_attempt_at ? `Checked ${formatRelative(source.last_attempt_at)}` : "Not checked yet";
  return (
    <>
      {source.error && (
        <Banner level="error">
          {source.error}
          {source.last_success_at ? ` · last worked ${formatRelative(source.last_success_at)}` : " · never loaded"}
        </Banner>
      )}
      <Section footer={error ? <span class="danger">{error}</span> : source.subtitle}>
        <Cell
          label={local ? "Show in Kiwi" : "Use This Source"}
          sub={local ? "Kiwi Builds' apps in Browse and My Apps" : "Turn off to hide its apps without removing it"}
        >
          <Toggle
            checked={optimistic ?? source.enabled}
            onChange={(value) => void toggle.run(value)}
            label={local ? `Show ${source.name} in Kiwi` : `Enable ${source.name}`}
            disabled={toggle.pending}
          />
        </Cell>
        {local ? (
          <LinkCell href={BUILDS_PATH} label="Kiwi Builds" sub="Choose apps, patches and schedules" />
        ) : (
          <>
            <div class="cell cell-stacked">
              <div class="cell-label">URL</div>
              <div class="source-url">{source.url}</div>
            </div>
            <Cell label={source.error ? "Try Again" : "Refresh"} sub={checked}>
              <button
                type="button"
                class="text-button"
                onClick={() => void refresh.run()}
                disabled={refresh.pending || !source.enabled}
              >
                {refresh.pending ? <Spinner /> : source.error ? "Retry" : "Refresh Now"}
              </button>
            </Cell>
          </>
        )}
      </Section>
      {source.diagnostics.length > 0 && (
        <Section title={plural(source.diagnostics.length, "Warning")} footer="Kiwi skipped what it couldn't read; the rest of the source works.">
          <div class="cell">
            <ul class="diagnostics-list">
              {source.diagnostics.map((d, i) => (
                <li key={i}>{d}</li>
              ))}
            </ul>
          </div>
        </Section>
      )}
    </>
  );
}

/** A source's page: on or off, refresh and remove, and which of its apps Browse shows. */
export function SourceApps({ sourceId }: { sourceId: string }) {
  const sources = useResource("sources", api.sources);
  const apps = useResource(`source-apps/${sourceId}`, () => api.apps({ source: sourceId, hidden: true }));
  const { reload: reloadMe } = useMe();
  const [error, setError] = useState<string | null>(null);
  const source = sources.data?.find((s) => s.id === sourceId);

  const replaceSource = (next: Source) => {
    sources.mutate((list) => list?.map((s) => (s.id === next.id ? next : s)));
    reloadMe();
    void apps.reload();
  };
  const setMode = useTask(async (browse: BrowseMode) => {
    sources.mutate((list) => list?.map((s) => (s.id === sourceId ? { ...s, browse } : s)));
    replaceSource(await api.updateSource(sourceId, { browse }));
  });
  const remove = useTask(async () => {
    if (!source) return;
    const ok = window.confirm(
      `Remove ${source.name}?\n\nInstalled apps stay on your iPhone but won't get updates from this source.`,
    );
    if (!ok) return;
    await api.removeSource(source.id);
    sources.mutate((list) => list?.filter((s) => s.id !== source.id));
    reloadMe();
    navigate("/sources", { replace: true });
  });
  const toggle = async (app: AppSummary, shown: boolean) => {
    setError(null);
    const set = (hidden: boolean, isNew: boolean) =>
      apps.mutate((list) =>
        list?.map((a) => (appKey(a) === appKey(app) ? { ...a, hidden, is_new: isNew } : a)),
      );
    set(!shown, false);
    try {
      await api.updateApp(app.source_id, app.bundle_id, { hidden: !shown });
      void sources.reload();
    } catch (e) {
      set(app.hidden, app.is_new);
      setError(errorMessage(e));
    }
  };

  let content;
  if (!source) {
    content = sources.error ? (
      <ErrorState error={sources.error} onRetry={sources.reload} />
    ) : sources.data ? (
      <Empty icon="sources" title="Source Not Found" message="It may have been removed.">
        <a class="button" href="/sources">
          Sources
        </a>
      </Empty>
    ) : (
      <Loading />
    );
  } else {
    const footer = MODES.find((m) => m.mode === source.browse)?.footer ?? "";
    const list = apps.data ?? [];
    // A source that is off has no apps to choose.
    let appList = null;
    if (source.enabled && !apps.data) {
      appList = apps.error ? <ErrorState error={apps.error} onRetry={apps.reload} /> : <Loading />;
    } else if (source.enabled && list.length > 0) {
      const shown = list.filter((a) => !a.hidden).length;
      appList = (
        <section class="section">
          <h2 class="section-title">
            Apps · {shown} of {list.length} in Browse
          </h2>
          <div class="cells">
            {list.map((app) => (
              <AppCell key={appKey(app)} app={app} onToggle={(a, on) => void toggle(a, on)} />
            ))}
          </div>
          {error && <div class="section-footer danger">{error}</div>}
        </section>
      );
    }
    content = (
      <>
        <header class="page-header">
          <h1 class="large-title">{source.name}</h1>
        </header>
        <p class="page-summary">
          {plural(source.app_count, "app")}
          {source.last_success_at && ` · updated ${formatRelative(source.last_success_at)}`}
        </p>
        <SourceSettings source={source} onChange={replaceSource} />
        {source.enabled && source.app_count > 0 && (
          <section class="section">
            <h2 class="section-title">Show in Browse</h2>
            <div class="segmented" role="radiogroup" aria-label="Show in Browse">
              {MODES.map((m) => (
                <button
                  key={m.mode}
                  type="button"
                  role="radio"
                  aria-checked={m.mode === source.browse}
                  class={m.mode === source.browse ? "segment active" : "segment"}
                  onClick={() => m.mode !== source.browse && void setMode.run(m.mode)}
                  disabled={setMode.pending}
                >
                  {m.label}
                </button>
              ))}
            </div>
            <div class="section-footer">
              {setMode.pending ? <Spinner /> : setMode.error ? <span class="danger">{setMode.error}</span> : footer}
            </div>
          </section>
        )}
        {appList}
        {source.kind !== "local_build" && (
          <Section footer={remove.error ? <span class="danger">{remove.error}</span> : "Installed apps stay on your iPhone but stop getting updates."}>
            <button type="button" class="cell cell-button" onClick={() => void remove.run()} disabled={remove.pending}>
              <div class="cell-main cell-label danger">Remove Source</div>
              {remove.pending && <Spinner />}
            </button>
          </Section>
        )}
      </>
    );
  }

  return (
    <main class="page page-detail">
      <BackBar label="Sources" fallback="/sources" />
      {content}
    </main>
  );
}

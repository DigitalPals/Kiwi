import { useEffect, useState } from "preact/hooks";
import { api } from "../api/client";
import type { AutomationOverview, Source } from "../api/types";
import { AppIcon } from "../components/AppIcon";
import { Icon } from "../components/Icon";
import { Banner, Empty, ErrorState, LinkCell, Loading, Page, Section, Spinner } from "../components/ui";
import { overviewSummary } from "../lib/automation";
import { formatRelative, plural } from "../lib/format";
import { useOnForeground, useResource, useTask } from "../lib/hooks";
import { useMe } from "../lib/me";
import { BUILDS_PATH, sourcePath } from "../lib/routes";

/** "4 apps · updated 12 min ago", how many Browse shows, or that it is off. */
function SourceLine({ source }: { source: Source }) {
  if (!source.enabled) return <>Off · {plural(source.app_count, "app")}</>;
  if (source.error) return <span class="tone-danger">Failed: {source.error}</span>;
  const picked = source.shown_count < source.app_count;
  return (
    <>
      {picked
        ? `${source.shown_count} of ${plural(source.app_count, "app")} in Browse`
        : plural(source.app_count, "app")}
      {!picked && !source.diagnostics.length && source.last_success_at && ` · updated ${formatRelative(source.last_success_at)}`}
      {source.new_count > 0 && <span class="tone-accent"> · {source.new_count} new</span>}
      {source.diagnostics.length > 0 && <span class="tone-warn"> · {plural(source.diagnostics.length, "warning")}</span>}
    </>
  );
}

/** A source opens its page; a failed one can be retried from the list. */
function SourceRow({ source, onChange }: { source: Source; onChange: (source: Source) => void }) {
  const retry = useTask(async () => onChange(await api.refreshSource(source.id)));
  const off = !source.enabled;
  const icon = <AppIcon url={source.icon_url} name={source.name} size={36} />;
  const text = (
    <div class="cell-main">
      <div class="cell-label one-line">{source.name}</div>
      <div class="cell-sub one-line">
        {retry.error ? <span class="tone-danger">{retry.error}</span> : <SourceLine source={source} />}
      </div>
    </div>
  );
  if (source.error && !off) {
    return (
      <div class="cell has-icon">
        <a class="pick-link" href={sourcePath(source.id)}>
          {icon}
          {text}
        </a>
        <button type="button" class="pill pill-small" onClick={() => void retry.run()} disabled={retry.pending}>
          {retry.pending ? <Spinner /> : "Retry"}
        </button>
      </div>
    );
  }
  return (
    <a class={off ? "cell has-icon cell-link off" : "cell has-icon cell-link"} href={sourcePath(source.id)}>
      {icon}
      {text}
      <Icon name="chevron" size={16} class="cell-chevron" />
    </a>
  );
}

/** "YouTube · 4 more available": the apps Kiwi builds and the ones it could. */
function buildsLine(overview: AutomationOverview): string {
  const added = overview.apps.filter((a) => a.added).length;
  const more = overview.apps.length - added;
  return added > 0 && more > 0 ? `${overviewSummary(overview)} · ${more} more available` : overviewSummary(overview);
}

function BuildsRow() {
  const overview = useResource("automation", api.automationOverview);
  useOnForeground(() => void overview.reload());
  return (
    <LinkCell
      href={BUILDS_PATH}
      art={
        <span class="tile-icon" aria-hidden="true">
          <Icon name="wrench" size={19} strokeWidth={1.8} />
        </span>
      }
      label="Kiwi Builds"
      sub={overview.data ? buildsLine(overview.data) : "YouTube, YouTube Music, Twitch, X and Reddit"}
    />
  );
}

/** The URL of an AltStore source; the server checks it before adding it. */
function AddSourceSheet({ onAdded, onClose }: { onAdded: (source: Source) => void; onClose: () => void }) {
  const [url, setUrl] = useState("");
  const add = useTask(async () => {
    onAdded(await api.addSource(url.trim()));
    onClose();
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
      <div class="sheet" role="dialog" aria-modal="true" aria-labelledby="add-source-title" onClick={(e) => e.stopPropagation()}>
        <header class="sheet-header">
          <h2 id="add-source-title">Add Source</h2>
          <button type="button" class="text-button" onClick={onClose}>
            Cancel
          </button>
        </header>
        <form
          onSubmit={(e) => {
            e.preventDefault();
            if (url.trim() && !add.pending) void add.run();
          }}
        >
          <label class="add-source">
            <Icon name="link" size={18} />
            <span class="visually-hidden">Source URL</span>
            <input
              type="url"
              inputMode="url"
              placeholder="https://…/apps.json"
              value={url}
              onInput={(e) => {
                setUrl(e.currentTarget.value);
                add.setError(null);
              }}
              autoFocus
              autocapitalize="off"
              autocorrect="off"
              spellcheck={false}
              enterKeyHint="go"
              disabled={add.pending}
            />
          </label>
          {add.error ? (
            <p class="add-source-status form-error">{add.error}</p>
          ) : (
            <p class="add-source-status">
              {add.pending
                ? "Fetching and checking the source…"
                : "An AltStore source, e.g. a repository's apps.json. GitHub file links work too."}
            </p>
          )}
          <button type="submit" class="button button-block" disabled={!url.trim() || add.pending}>
            {add.pending ? <Spinner /> : "Add Source"}
          </button>
        </form>
      </div>
    </div>
  );
}

function lastChecked(list: Source[]): string | null {
  const times = list.filter((s) => s.enabled && s.last_attempt_at).map((s) => s.last_attempt_at as string);
  return times.length ? times.reduce((a, b) => (a > b ? a : b)) : null;
}

export function Sources() {
  const sources = useResource("sources", api.sources);
  const settings = useResource("settings", api.settings);
  const { reload: reloadMe } = useMe();
  const [adding, setAdding] = useState(false);
  const { mutate } = sources;
  const refreshAll = useTask(async () => {
    mutate(await api.refreshSources());
    reloadMe();
  });

  const replace = (source: Source) => {
    mutate((list) => list?.map((s) => (s.id === source.id ? source : s)));
    reloadMe();
  };
  const added = (source: Source) => {
    mutate((list) => [...(list ?? []).filter((s) => s.id !== source.id), source]);
    reloadMe();
  };

  // Kiwi Builds has its own row above the AltStore sources.
  const list = sources.data?.filter((s) => s.kind !== "local_build");
  let content;
  if (!list) {
    content = sources.error ? <ErrorState error={sources.error} onRetry={sources.reload} /> : <Loading />;
  } else if (list.length === 0) {
    content = (
      <Empty icon="sources" title="No Sources" message="Add an AltStore source, for example a repository's apps.json.">
        <button type="button" class="button" onClick={() => setAdding(true)}>
          Add a Source
        </button>
      </Empty>
    );
  } else {
    content = (
      <Section title="AltStore Sources" footer="Open a source to pick which of its apps appear in Browse, turn it off or remove it.">
        {list.map((source) => (
          <SourceRow key={source.id} source={source} onChange={replace} />
        ))}
      </Section>
    );
  }

  let summary = null;
  if (list && list.length > 0) {
    const enabled = list.filter((s) => s.enabled);
    const checked = lastChecked(list);
    summary = (
      <>
        <span>
          {[`${enabled.length} on`, plural(enabled.reduce((sum, s) => sum + s.app_count, 0), "app"), checked && `checked ${formatRelative(checked)}`]
            .filter(Boolean)
            .join(" · ")}
        </span>
        {enabled.length > 0 && (
          <>
            <span>·</span>
            <button type="button" class="text-button small" onClick={() => void refreshAll.run()} disabled={refreshAll.pending}>
              {refreshAll.pending ? "Checking…" : "Check Now"}
            </button>
          </>
        )}
      </>
    );
  }

  return (
    <Page
      title="Sources"
      summary={summary}
      actions={
        <button type="button" class="icon-button" aria-label="Add Source" onClick={() => setAdding(true)}>
          <Icon name="plus" size={20} strokeWidth={2.2} />
        </button>
      }
    >
      {refreshAll.error && <Banner level="error">{refreshAll.error}</Banner>}
      {settings.data?.kiwi_builds && (
        <Section title="Built on Your Mac">
          <BuildsRow />
        </Section>
      )}
      {content}
      {adding && <AddSourceSheet onAdded={added} onClose={() => setAdding(false)} />}
    </Page>
  );
}

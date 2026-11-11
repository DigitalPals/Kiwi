import { useEffect } from "preact/hooks";
import { api, ApiError } from "../api/client";
import type { AutomationState, BuilderInfo, SetupCheck } from "../api/types";
import { AppIcon } from "../components/AppIcon";
import { Icon } from "../components/Icon";
import { BackBar, Banner, Cell, CommandText, Empty, ErrorState, LinkCell, Loading, Page, Section, Spinner } from "../components/ui";
import { activeJob, appsForDevice, BUILD_SOURCE_ID, buildsNotice, statusLine } from "../lib/automation";
import { checkView, iosBuilds, NEEDS_MAC_CHECK } from "../lib/builder";
import { hasFeature, useFeatures } from "../lib/features";
import { useOnForeground, useResource, useTask } from "../lib/hooks";
import { useMe } from "../lib/me";
import { buildPath, MAC_BUILDER_PATH, sourcePath } from "../lib/routes";

export function ExperimentalTag() {
  return <span class="tag tag-warn">Experimental</span>;
}

/** Tells an Android app from the iPhone app of the same name. */
export function AndroidTag() {
  return <span class="tag">Android</span>;
}

function BuildRow({ app }: { app: AutomationState }) {
  const active = activeJob(app);
  return (
    <a class="cell has-icon cell-link build-row" href={buildPath(app.id)}>
      <AppIcon url={app.icon_url} name={app.name} size={40} />
      <div class="cell-main">
        <div class="cell-label">{app.name} {app.experimental && <ExperimentalTag />}</div>
        <div class="cell-sub">
          {app.added ? (
            <span class="build-status">{active?.status === "running" && <Spinner />}{statusLine(app)}</span>
          ) : app.summary}
        </div>
      </div>
      {app.added ? <Icon name="chevron" size={16} class="cell-chevron" /> : <span class="pill pill-small">Add</span>}
    </a>
  );
}

/**
 * The setup checks, each with its fix. The missing or unavailable Mac builder links to
 * Settings › Mac Builder, except on that page itself (`link` false).
 */
export function CheckCells({ checks, info, link = false }: { checks: SetupCheck[]; info?: BuilderInfo; link?: boolean }) {
  return (
    <>
      {checks.map((check) => {
        const view = checkView(check, info);
        const status = <span class={view.ready ? "tone-ok" : view.settings ? "tone-warn" : "danger"}>{view.status}</span>;
        const sub = view.hint && (
          <>
            {view.hint}
            {view.command && <CommandText command={view.command} />}
          </>
        );
        if (view.settings && link) return <LinkCell key={check.name} href={MAC_BUILDER_PATH} label={view.label} sub={sub} value={status} />;
        return <Cell key={check.name} label={view.label} sub={sub}>{status}</Cell>;
      })}
    </>
  );
}

export function SetupCells({ checks, info }: { checks: SetupCheck[]; info?: BuilderInfo }) {
  if (checks.length === 0) return <Cell label="Setup has not been checked" sub="Start a build to check the Mac." />;
  return <CheckCells checks={checks} info={info} link />;
}

/**
 * `GET /api/builder` where iOS apps are built on a paired Mac (a Kiwi not on macOS, on an
 * iPhone): whether one is paired and how it is doing. Undefined elsewhere or until known.
 */
export function useBuilderInfo(): { info: BuilderInfo | undefined; error: Error | null } {
  const { me } = useMe();
  const wanted = !!me && me.host.os !== "macos" && !hasFeature(me, "android_apps");
  const resource = useResource<BuilderInfo | null>(wanted ? "builder" : "builder:none", () => (wanted ? api.builder() : Promise.resolve(null)));
  useOnForeground(() => void resource.reload());
  return wanted ? { info: resource.data ?? undefined, error: resource.error } : { info: undefined, error: null };
}

/** The line under Kiwi Builds' title on an iPhone: where its apps are built. */
function summary(builds: ReturnType<typeof iosBuilds>, needsBuilder: boolean, builder: string | undefined): string {
  if (needsBuilder) return "iOS apps need a Mac builder";
  if (builder) return `Patched on ${builder}, signed by Kiwi`;
  return builds === "unknown" ? "Patched App Store apps" : "Patched and signed on your Mac";
}

/**
 * Kiwi Builds: the apps added, the ones to add and the Mac's setup. An iPhone opens it from
 * Sources and sees iPhone apps; on Android it is a tab with the Android apps only.
 */
export function Builds() {
  const android = useFeatures().has("android_apps");
  const host = useMe().me?.host;
  // An iPhone on a Kiwi that doesn't run on macOS: its apps are built on the paired Mac builder.
  const { info } = useBuilderInfo();
  const builder = info?.builder ?? null;
  const overview = useResource("automation", api.automationOverview);
  const sources = useResource("sources", api.sources);
  const source = sources.data?.find((s) => s.id === BUILD_SOURCE_ID);
  const data = overview.data;
  const active = !!data?.apps.some((a) => activeJob(a));
  const builds = android || !host ? "local" : iosBuilds(host, info);
  // Until `GET /api/builder` answers, the missing Mac shows in the checks.
  const needsBuilder = builds === "none" || (builds === "unknown" && !!data?.checks.some((c) => c.name === NEEDS_MAC_CHECK));
  useOnForeground(() => void overview.reload());
  useEffect(() => {
    const timer = window.setInterval(() => {
      if (document.visibilityState === "visible") void overview.reload();
    }, active ? 3000 : 30_000);
    return () => window.clearInterval(timer);
  }, [active, overview.reload]);

  // Android has no Settings switch for Kiwi Builds' Mac side, so it turns it on here.
  const turnOn = useTask(async () => {
    await api.updateSettings({ kiwi_builds: true });
    await overview.reload();
  });

  let content;
  if (overview.error instanceof ApiError && overview.error.code === "builds_off") {
    content = android ? (
      <Empty icon="wrench" title="Kiwi Builds Is Off" message="Turn it on to patch Android apps and sign them with this Kiwi's own key.">
        <button
          type="button"
          class="button"
          disabled={turnOn.pending}
          onClick={() => window.confirm(buildsNotice(true, host ?? { os: "other" }, info)) && void turnOn.run()}
        >
          {turnOn.pending ? <Spinner /> : "Turn On Kiwi Builds"}
        </button>
        {turnOn.error && <p class="form-error">{turnOn.error}</p>}
      </Empty>
    ) : (
      <Empty
        icon="wrench"
        title="Kiwi Builds Is Off"
        message={needsBuilder
          ? "Turn it on in Settings. iOS apps also need a Mac builder, which you pair in Settings › Mac Builder."
          : builder
            ? `Turn it on in Settings to patch App Store apps on ${builder.name}.`
            : "Turn it on in Settings to patch App Store apps on this Mac."}
      >
        <a class="button" href="/settings">
          Open Settings
        </a>
        {needsBuilder && (
          <a class="link builds-off-link" href={MAC_BUILDER_PATH}>
            Pair a Mac
          </a>
        )}
      </Empty>
    );
  } else if (!data) {
    content = overview.error ? <ErrorState error={overview.error} onRetry={overview.reload} /> : <Loading />;
  } else {
    const apps = appsForDevice(data.apps, android);
    const added = apps.filter((a) => a.added);
    const available = apps.filter((a) => !a.added);
    const ready = data.checks.length > 0 && data.checks.every((c) => c.ready);
    content = (
      <>
        {!android && needsBuilder && (
          <Banner level="info" icon="server" href={MAC_BUILDER_PATH}>
            <strong>iOS apps need a Mac builder.</strong> Pair a Mac in Settings › Mac Builder to build them.
          </Banner>
        )}
        {added.length > 0 && (
          <Section title="Your Apps" footer="One build runs at a time. Each app keeps its own schedule.">
            {added.map((app) => <BuildRow key={app.id} app={app} />)}
          </Section>
        )}
        {available.length > 0 && (
          <Section
            title={android ? "Add an Android App" : "Add an App"}
            footer={android
              ? "Downloaded from Google Play, patched with pinned open-source patches and signed with this Kiwi's own key."
              : builder
                ? `Downloaded with your Apple account and patched from pinned open-source tweaks on ${builder.name}, then signed by this Kiwi.`
                : "Downloaded with your Apple account, patched from pinned open-source tweaks and signed on the Mac."}
          >
            {available.map((app) => <BuildRow key={app.id} app={app} />)}
          </Section>
        )}
        {source && (
          <Section title="In Kiwi">
            <LinkCell
              href={sourcePath(source.id)}
              label="Kiwi Builds Source"
              sub="Which built apps Browse shows"
              value={source.enabled ? "On" : "Off"}
            />
          </Section>
        )}
        {!android && (
          <Section title={builder ? builder.name : "Mac"} footer="Apple account sign-in and enrollment stay on the Mac.">
            {ready ? (
              <Cell label="Mac Setup" sub="Build tools, App Store sign-in, enrollment" icon="server">
                <span class="tone-ok">Ready</span>
              </Cell>
            ) : <SetupCells checks={data.checks} info={info} />}
          </Section>
        )}
      </>
    );
  }
  if (android) {
    return (
      <Page title="Kiwi Builds" summary="Android apps, patched and signed by this Kiwi">
        {content}
      </Page>
    );
  }
  return (
    <main class="page">
      <BackBar label="Sources" fallback="/sources" />
      <header class="page-header">
        <h1 class="large-title">Kiwi Builds</h1>
      </header>
      <p class="page-summary">{summary(builds, needsBuilder, builder?.name)}</p>
      {content}
    </main>
  );
}

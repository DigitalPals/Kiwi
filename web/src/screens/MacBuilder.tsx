import { useEffect, useState } from "preact/hooks";
import { api } from "../api/client";
import type { Builder, BuilderInfo, BuilderPairing } from "../api/types";
import { Icon } from "../components/Icon";
import { BackBar, Banner, Cell, CommandText, Empty, ErrorState, Loading, Section, Spinner } from "../components/ui";
import {
  BUILDER_CHECK,
  builderState,
  builderStatus,
  countdown,
  PAIRING_POLL_MS,
  pairingStep,
  statusDetail,
  statusText,
  taskTitle,
  UPDATE_COMMAND,
  watchPairing,
  type PairingWatch,
} from "../lib/builder";
import { formatDate, formatDateTime, formatRelative } from "../lib/format";
import { useOnForeground, useResource, useTask } from "../lib/hooks";
import { CheckCells } from "./Builds";

const INTRO =
  "iOS Kiwi Builds need a Mac. The Mac downloads, decrypts and patches the apps; this Kiwi signs and installs them. Your Apple account and the decryption stay on the Mac.";

const removeQuestion = (name: string) =>
  `Remove ${name}?\n\niOS apps stop building until a Mac is paired again. Installed apps keep working.`;

/** The current time, every `ms` while `active`. */
function useNow(ms: number, active: boolean): number {
  const [now, setNow] = useState(Date.now);
  useEffect(() => {
    if (!active) return;
    setNow(Date.now());
    const timer = window.setInterval(() => setNow(Date.now()), ms);
    return () => window.clearInterval(timer);
  }, [ms, active]);
  return active ? now : Date.now();
}

/** Copies the command; where the clipboard isn't available, a tap on the command selects it. */
function CopyButton({ text }: { text: string }) {
  const [state, setState] = useState<"idle" | "copied" | "failed">("idle");
  useEffect(() => {
    if (state === "idle") return;
    const timer = window.setTimeout(() => setState("idle"), 2000);
    return () => window.clearTimeout(timer);
  }, [state]);
  const copy = () => {
    if (!navigator.clipboard) return setState("failed");
    navigator.clipboard.writeText(text).then(() => setState("copied"), () => setState("failed"));
  };
  return (
    <button type="button" class="pill pill-small" onClick={copy} aria-live="polite">
      {state === "copied" ? (
        <>
          <Icon name="check" size={15} strokeWidth={2.4} /> Copied
        </>
      ) : state === "failed" ? "Select It" : "Copy"}
    </button>
  );
}

function Unpaired({ onPair, pending, error }: { onPair: () => void; pending: boolean; error: string | null }) {
  return (
    <>
      <div class="status-card off">
        <span class="status-icon" aria-hidden="true">
          <Icon name="server" size={20} />
        </span>
        <div>
          <div class="status-title">No Mac Paired</div>
          <div class="status-text">{INTRO}</div>
        </div>
      </div>
      {error && <Banner level="error">{error}</Banner>}
      <button type="button" class="button button-block" onClick={onPair} disabled={pending}>
        {pending ? <Spinner /> : "Pair a Mac"}
      </button>
      <p class="build-caption">You get a command to run in Terminal on the Mac, with a code that works once.</p>
    </>
  );
}

function Pairing(props: {
  pairing: BuilderPairing;
  replaces: Builder | null;
  now: number;
  onCancel: () => void;
  cancelling: boolean;
  error: string | null;
}) {
  const { pairing, replaces, now } = props;
  const left = countdown(pairing.expires_at, now);
  return (
    <>
      <div class="status-card">
        <span class="status-icon" aria-hidden="true">
          <Spinner />
        </span>
        <div>
          <div class="status-title">Waiting for the Mac</div>
          <div class="status-text">Run the command below in Terminal on the Mac. This screen updates once the Mac has paired.</div>
        </div>
      </div>
      {props.error && <Banner level="error">{props.error}</Banner>}
      <Section
        title="In Terminal on the Mac"
        footer="Run it in Kiwi's folder (a copy of its repository). It installs Kiwi's builder, pairs it with this Kiwi and keeps it running while you're logged in."
      >
        <div class="cell cell-stacked command-cell">
          <CommandText command={pairing.command} class="command-block" />
          <CopyButton text={pairing.command} />
        </div>
      </Section>
      <Section footer={replaces ? `Pairing replaces ${replaces.name}. The code works once.` : "The code works once."}>
        <Cell label="Pairing Code">
          <span class="mono pairing-code">{pairing.code}</span>
        </Cell>
        <Cell label="Expires In">
          <span class="mono" role="timer" aria-label={`${left.label} left`}>
            {left.label}
          </span>
        </Cell>
        <button type="button" class="cell cell-button center" onClick={props.onCancel} disabled={props.cancelling}>
          {props.cancelling ? <Spinner /> : <span class="cell-action">Cancel Pairing</span>}
        </button>
      </Section>
    </>
  );
}

function Paired({ builder, reload, onRemoved }: { builder: Builder; reload: () => Promise<void>; onRemoved: () => void }) {
  const [asked, setAsked] = useState(false);
  const status = builderStatus(builder);
  const check = useTask(async () => {
    await api.checkBuilder();
    setAsked(true);
    await reload();
  });
  const remove = useTask(async () => {
    if (!window.confirm(removeQuestion(builder.name))) return;
    await api.removeBuilder();
    onRemoved();
  });
  // Once the check has run, its results speak for themselves.
  useEffect(() => {
    if (builder.task?.command === "doctor") setAsked(false);
  }, [builder.task?.command]);
  const checks = builder.checks.filter((c) => c.name !== BUILDER_CHECK);
  const task = builder.task;
  let checkNote = "Kiwi checks the Mac when it connects and every 6 hours. Apple account sign-in and enrollment stay on the Mac.";
  if (check.error) checkNote = check.error;
  else if (asked) checkNote = status === "offline" ? `${builder.name} checks its setup when it's back online.` : `Asked ${builder.name} to check its setup.`;
  return (
    <>
      <div class={status === "online" ? "status-card" : "status-card warn"}>
        <span class="status-icon" aria-hidden="true">
          <Icon name="server" size={20} />
        </span>
        <div>
          <div class="status-title">{builder.name}</div>
          <div class={status === "online" ? "status-text tone-ok" : "status-text tone-warn"}>{statusText(builder)}</div>
          <div class="status-text">{statusDetail(builder)}</div>
        </div>
      </div>
      {!builder.compatible && (
        <Banner level="warning">
          Kiwi on {builder.name} is a different version. On the Mac:
          <CommandText command={UPDATE_COMMAND} />
        </Banner>
      )}
      {task && (
        <Section title="Now">
          <Cell label={taskTitle(task)} sub={`${task.stage} · started ${formatRelative(task.started_at)}`}>
            <Spinner />
          </Cell>
        </Section>
      )}
      <Section title="Setup" footer={check.error ? <span class="danger">{checkNote}</span> : checkNote}>
        {checks.length > 0 ? (
          <CheckCells checks={checks} />
        ) : (
          <Cell label="Setup has not been checked" sub={`${builder.name} checks it when it connects.`} />
        )}
        <button type="button" class="cell cell-button center" onClick={() => void check.run()} disabled={check.pending}>
          {check.pending ? <Spinner /> : <span class="cell-action">Check Again</span>}
        </button>
      </Section>
      <Section title="Details">
        <Cell label="Kiwi">{builder.version}</Cell>
        <Cell label="System">{builder.os}</Cell>
        <Cell label="Paired">{formatDate(builder.paired_at)}</Cell>
        <Cell label="Last Seen">{builder.last_seen ? formatDateTime(builder.last_seen) : "Not yet"}</Cell>
      </Section>
      <Section footer={remove.error ? <span class="danger">{remove.error}</span> : "iOS apps stop building until a Mac is paired again. Installed apps keep working."}>
        <button type="button" class="cell cell-button center" onClick={() => void remove.run()} disabled={remove.pending}>
          {remove.pending ? <Spinner /> : <span class="cell-action danger">Remove {builder.name}</span>}
        </button>
      </Section>
    </>
  );
}

/**
 * Settings › Mac Builder: pairs the Mac that builds iOS apps for a Kiwi that doesn't run on macOS,
 * and shows whether it's online, up to date and set up.
 */
export function MacBuilderScreen() {
  const info = useResource<BuilderInfo>("builder", api.builder);
  const data = info.data;
  // The code this screen waits on, and how the last wait ended.
  const [watch, setWatch] = useState<PairingWatch | null>(null);
  const [notice, setNotice] = useState<{ level: "info" | "warning"; text: string } | null>(null);
  const now = useNow(1000, !!watch || !!data?.pairing);
  const state = data ? builderState(data, now) : null;
  const polling = !!watch && !!data && pairingStep(watch, data, now) === "poll";
  const busy = !!data?.builder?.task;

  // Follow each answer while a code is out: stop once the Mac paired or the code expired.
  useEffect(() => {
    if (!data) return;
    if (!watch) {
      if (data.pairing) setWatch(watchPairing(data));
      return;
    }
    const step = pairingStep(watch, data, now);
    if (step === "poll") {
      if (data.pairing && data.pairing.code !== watch.code) setWatch({ ...watch, code: data.pairing.code, expires_at: data.pairing.expires_at });
      return;
    }
    setWatch(null);
    if (step === "paired") setNotice({ level: "info", text: `Paired with ${data.builder!.name}.` });
    else if (step === "expired") setNotice({ level: "warning", text: "The pairing code expired. Pair again for a new one." });
  }, [data, watch, now]);

  useOnForeground(() => void info.reload());
  useEffect(() => {
    if (!data || state === "unsupported") return;
    const ms = polling ? PAIRING_POLL_MS : busy ? 3000 : 30_000;
    const timer = window.setInterval(() => {
      if (document.visibilityState === "visible") void info.reload();
    }, ms);
    return () => window.clearInterval(timer);
  }, [!!data, state, polling, busy, info.reload]);

  const pair = useTask(async () => {
    const pairing = await api.pairBuilder();
    setNotice(null);
    setWatch(null);
    info.mutate((previous) => previous && { ...previous, pairing });
  });
  const cancel = useTask(async () => {
    await api.cancelBuilderPairing();
    setWatch(null);
    info.mutate((previous) => previous && { ...previous, pairing: null });
    void info.reload();
  });
  const removed = () => {
    setNotice(null);
    info.mutate((previous) => previous && { ...previous, builder: null });
    void info.reload();
  };

  let content;
  if (!data) content = info.error ? <ErrorState error={info.error} onRetry={info.reload} /> : <Loading />;
  else if (state === "unsupported") {
    content = <Empty icon="server" title="Builds on This Mac" message={data.reason ?? "This Kiwi builds iOS apps itself."} />;
  } else if (state === "pairing" && data.pairing) {
    content = (
      <Pairing
        pairing={data.pairing}
        replaces={data.builder}
        now={now}
        onCancel={() => void cancel.run()}
        cancelling={cancel.pending}
        error={cancel.error}
      />
    );
  } else if (data.builder) {
    content = <Paired key={data.builder.paired_at} builder={data.builder} reload={info.reload} onRemoved={removed} />;
  } else {
    content = <Unpaired onPair={() => void pair.run()} pending={pair.pending} error={pair.error} />;
  }

  return (
    <main class="page">
      <BackBar label="Settings" fallback="/settings" />
      <header class="page-header">
        <h1 class="large-title">Mac Builder</h1>
      </header>
      {notice && state !== "pairing" && (
        <Banner level={notice.level} icon={notice.level === "info" ? "check" : undefined} onDismiss={() => setNotice(null)}>
          {notice.text}
        </Banner>
      )}
      {content}
    </main>
  );
}

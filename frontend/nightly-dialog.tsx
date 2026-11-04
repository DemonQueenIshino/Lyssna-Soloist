import { Check, CircleDashed, LoaderCircle, Minus, X } from "lucide-react";
import { IS_MAC } from "./platform";
import type { NightlyReport, NightlyStep } from "./types";

/** How much of the whole build each step is, roughly by its time. */
const WEIGHTS: Record<string, number> = {
  cpp: 14,
  cmake: 2,
  libclang: 2,
  node: 3,
  source: 2,
  rust: 8,
  packages: 6,
  interface: 3,
  crates: 6,
  app: 50,
  check: 2,
  install: 2,
};

/** The whole build's progress, 0 to 1. */
export function nightlyProgress(report: NightlyReport): number {
  let total = 0,
    done = 0;
  for (const step of report.steps) {
    const weight = WEIGHTS[step.id] ?? 2;
    total += weight;
    if (step.state === "done" || step.state === "skipped") done += weight;
    else if (step.state === "running") done += weight * (step.progress ?? 0);
  }
  return total ? done / total : 0;
}

/**
 * Switching to Nightly: what Lyssna will set up, then each step with its
 * progress while it builds. The build keeps going when this is hidden.
 */
export function NightlyDialog({
  report,
  starting,
  onStart,
  onCancelBuild,
  onHide,
  onLog,
}: {
  report: NightlyReport | null;
  /** Build requested, first progress not in yet. */
  starting: boolean;
  onStart: () => void;
  onCancelBuild: () => void;
  onHide: () => void;
  onLog: () => void;
}) {
  const running = starting || !!report?.running;
  const installed = !!report?.finished && !report.note;
  const percent = report ? Math.round(nightlyProgress(report) * 100) : 0;
  return (
    <div
      className="modal-backdrop nightly-backdrop"
      onClick={(event) => {
        event.stopPropagation();
        if (!installed) onHide();
      }}
    >
      <section
        className="dialog nightly-dialog"
        role="alertdialog"
        aria-modal="true"
        aria-labelledby="nightly-title"
        onClick={(event) => event.stopPropagation()}
        onKeyDown={(event) => {
          if (event.key !== "Escape") return;
          event.stopPropagation();
          if (!installed) onHide();
        }}
      >
        {!report && !starting ? (
          <>
            <h2 id="nightly-title">Switch to Nightly?</h2>
            <p>
              Nightly is the newest Lyssna, built on this computer from the
              latest code. It gets changes first and hasn't been tested like
              Stable.
            </p>
            <h3>Lyssna sets everything up itself</h3>
            <ul>
              <li>
                Uses the Rust, Node.js, CMake and libclang already on your
                computer. Anything missing is downloaded into its own folder, up
                to 2 GB, and nothing else on your computer changes.
              </li>
              {IS_MAC ? (
                <li>
                  Installs Apple&rsquo;s Command Line Tools if they&rsquo;re
                  missing. macOS asks in a window of its own.
                </li>
              ) : (
                <li>
                  Installs Visual Studio&rsquo;s C++ build tools if
                  they&rsquo;re missing, about 6 GB. Windows asks for permission
                  once.
                </li>
              )}
              <li>
                Builds Lyssna. The first build takes 15 to 40 minutes; music
                keeps playing.
              </li>
            </ul>
            <p className="nightly-note">
              When it&rsquo;s done, Lyssna restarts into Nightly. Choose Stable
              any time to go back.
            </p>
            <div className="inline-actions">
              <button className="pill" autoFocus onClick={onHide}>
                Cancel
              </button>
              <button className="pill primary" onClick={onStart}>
                Build Nightly
              </button>
            </div>
          </>
        ) : (
          <>
            <h2 id="nightly-title">
              {installed
                ? "Restarting into Nightly…"
                : report?.error
                  ? "Nightly build stopped"
                  : report?.note
                    ? "Nightly is up to date"
                    : "Building Nightly"}
            </h2>
            <div
              className="nightly-bar nightly-total"
              role="progressbar"
              aria-label="Nightly build"
              aria-valuemin={0}
              aria-valuemax={100}
              aria-valuenow={percent}
            >
              <span style={{ width: `${percent}%` }} />
            </div>
            <ol className="nightly-steps">
              {(report?.steps ?? []).map((step) => (
                <NightlyStepRow key={step.id} step={step} />
              ))}
            </ol>
            {report?.error && (
              <pre className="nightly-error" role="alert">
                {report.error}
              </pre>
            )}
            {report?.note && <p className="nightly-note">{report.note}</p>}
            {!installed && (
              <div className="inline-actions">
                {report?.log && !running && (
                  <button className="pill outline" onClick={onLog}>
                    Open log
                  </button>
                )}
                {running ? (
                  <>
                    <button className="pill" onClick={onCancelBuild}>
                      Cancel build
                    </button>
                    <button className="pill primary" autoFocus onClick={onHide}>
                      Hide
                    </button>
                  </>
                ) : (
                  <>
                    <button className="pill" onClick={onHide}>
                      Close
                    </button>
                    {report?.error && (
                      <button className="pill primary" onClick={onStart}>
                        Try again
                      </button>
                    )}
                  </>
                )}
              </div>
            )}
          </>
        )}
      </section>
    </div>
  );
}

function NightlyStepRow({ step }: { step: NightlyStep }) {
  const Icon = {
    pending: CircleDashed,
    running: LoaderCircle,
    done: Check,
    skipped: Minus,
    failed: X,
  }[step.state];
  const status =
    step.state === "skipped"
      ? "Already there"
      : step.state === "done"
        ? "Done"
        : step.state === "failed"
          ? "Failed"
          : step.state === "running" && step.progress !== null
            ? `${Math.round(step.progress * 100)}%`
            : "";
  return (
    <li className={`nightly-step ${step.state}`}>
      <Icon size={16} aria-hidden="true" />
      <span className="nightly-step-label">{step.label}</span>
      <span className="nightly-step-status">{status}</span>
      {step.state === "running" && (
        <div
          className={`nightly-bar${step.progress === null ? " moving" : ""}`}
          role="progressbar"
          aria-label={step.label}
          aria-valuemin={0}
          aria-valuemax={100}
          aria-valuenow={
            step.progress === null ? undefined : Math.round(step.progress * 100)
          }
        >
          <span
            style={
              step.progress === null
                ? undefined
                : { width: `${step.progress * 100}%` }
            }
          />
        </div>
      )}
      {step.detail && (step.state === "running" || step.state === "failed") && (
        <p className="nightly-step-detail">{step.detail}</p>
      )}
      {step.id === "app" &&
        step.state === "running" &&
        (step.progress ?? 0) >= 0.95 && (
          <p className="nightly-step-note">
            Still building, don&rsquo;t worry. The last few parts are Lyssna
            itself and take the longest, often several minutes.
          </p>
        )}
    </li>
  );
}

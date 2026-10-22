/* Pentasio "Attack surface" screen. This is not a graph. It is the agent at work:
   you watch Pentasio's autonomous pentester reason, invoke tools, discover
   candidates, and prove them, in real time. Three zones read left to right the way
   the agent actually thinks: a self-ticking PLAN on the left, the live TRANSCRIPT
   in the middle (the star: thoughts, shell tools, and findings that get promoted
   from candidate to confirmed in place), and a PROVEN-FINDINGS evidence stack on
   the right that fills as the run lands each result. A blueprint grid sits behind
   the stream; a small tool tower replays the run or jumps to now.

   One ordered EVENTS array is the whole script. A pure buildStateUpTo(n) folds the
   declarative events into the render state, and n ticks forward on a setTimeout
   chain (each event carries its own delay). Replay resets to 0 and plays; Jump to
   now folds everything at once. prefers-reduced-motion skips straight to the end.

   Colour is never written here: every surface, line, and severity rides a token
   class, so the screen follows the light and dark themes on its own. No hex, no rgb,
   no em-dashes in any user-facing string. */
import { useEffect, useMemo, useRef, useState } from "react";
import type { ReactNode } from "react";
import { Check, ChevronsRight, Circle, RotateCcw, Terminal } from "lucide-react";
import { Panel, Status, chipLike, cn, ToolTower, ToolButton, ToolDivider } from "../../ui";
import { CountUp } from "./charts";
import { PHASES, SEV_LABEL, SEV_TONE, type Sev } from "./data";

/* ---------- local helpers (kept in this file so the screen is self-contained) ---------- */
function Eyebrow({ children }: { children: ReactNode }) {
  return (
    <div className="text-fine font-medium uppercase tracking-wider text-ink-2">
      {children}
    </div>
  );
}

function Mono({ children, className }: { children: ReactNode; className?: string }) {
  return <span className={cn("font-mono text-fine text-ink-2", className)}>{children}</span>;
}

/* three bouncing dots: the design system's "still working" mark */
function Dots() {
  return (
    <span className="u-dots">
      <span />
      <span />
      <span />
    </span>
  );
}

type LogKind = "ok" | "dim" | "warn" | "bad";

function logText(k: LogKind) {
  return k === "bad"
    ? "text-danger"
    : k === "warn"
      ? "text-warn"
      : k === "ok"
        ? "text-ok"
        : "text-ink-2";
}

const SEV_BORDER: Record<Sev, string> = {
  crit: "border-danger",
  high: "border-contest",
  med: "border-warn",
  low: "border-violet",
  info: "border-line",
};

/* CVSS counts up to a single decimal; rounding to whole numbers would drop the
   .1 that tells critical from critical. Honours reduced motion by landing on the
   value with no animation. */
function CvssCount({ to }: { to: number }) {
  const [n, setN] = useState(0);
  useEffect(() => {
    if (
      typeof window !== "undefined" &&
      typeof window.matchMedia === "function" &&
      window.matchMedia("(prefers-reduced-motion: reduce)").matches
    ) {
      setN(to);
      return;
    }
    let raf = 0;
    const t0 = performance.now();
    const dur = 900;
    const step = (t: number) => {
      const k = Math.min(1, (t - t0) / dur);
      setN(to * (1 - Math.pow(1 - k, 3)));
      if (k < 1) raf = requestAnimationFrame(step);
    };
    raf = requestAnimationFrame(step);
    return () => cancelAnimationFrame(raf);
  }, [to]);
  return <>{n.toFixed(1)}</>;
}

/* ---------- run state ---------- */
type PlanState = "pending" | "running" | "done";

type Row =
  | { kind: "thought"; id: string; t: string; tone: LogKind; text: string }
  | { kind: "tool"; id: string; cmd: string; running: boolean; result?: string }
  | {
      kind: "finding";
      id: string;
      stage: "candidate" | "confirmed";
      sev: Sev;
      endpoint: string;
      label: string;
      code?: string;
      cvss?: number;
      proof?: string;
    };

type RunState = {
  objective: string;
  status: "thinking" | "streaming";
  phaseIdx: number;
  hosts: number;
  endpoints: number;
  score: number;
  candidates: number;
  confirmed: number;
  plan: Record<string, PlanState>;
  shown: Record<string, boolean>;
  rows: Row[];
  validating: boolean;
};

const INITIAL: RunState = {
  objective: "Bringing the run online.",
  status: "thinking",
  phaseIdx: 0,
  hosts: 0,
  endpoints: 0,
  score: 0,
  candidates: 0,
  confirmed: 0,
  plan: {
    recon: "pending",
    fingerprint: "pending",
    vuln: "pending",
    "probe-idor": "pending",
    "probe-upload": "pending",
    exploit: "pending",
    report: "pending",
  },
  shown: { "probe-idor": false, "probe-upload": false },
  rows: [],
  validating: false,
};

/* the left plan: five phases with two probe sub-tasks that reveal mid-run */
const PLAN: { key: string; label: string; sub?: boolean }[] = [
  { key: "recon", label: "Recon" },
  { key: "fingerprint", label: "Fingerprint" },
  { key: "vuln", label: "Vuln discovery" },
  { key: "probe-idor", label: "Probe IDOR on /api/documents/{id}", sub: true },
  { key: "probe-upload", label: "Probe upload on /api/attachments", sub: true },
  { key: "exploit", label: "Exploit validation" },
  { key: "report", label: "Reporting" },
];

/* ---------- events: the whole run, declared once ---------- */
type Ev =
  | { d: number; op: "obj"; text: string; status?: RunState["status"] }
  | { d: number; op: "phase"; idx: number }
  | { d: number; op: "plan"; key: string; state: PlanState; show?: boolean }
  | { d: number; op: "thought"; id: string; t: string; tone: LogKind; text: string }
  | { d: number; op: "toolStart"; id: string; cmd: string }
  | { d: number; op: "toolDone"; id: string; result: string }
  | { d: number; op: "cand"; id: string; sev: Sev; endpoint: string; label: string }
  | { d: number; op: "confirm"; id: string; code: string; cvss: number; proof: string; planDone?: string }
  | { d: number; op: "count"; hosts?: number; endpoints?: number; score?: number; candidates?: number; confirmed?: number }
  | { d: number; op: "validating" };

const EVENTS: Ev[] = [
  { d: 250, op: "plan", key: "recon", state: "running" },
  { d: 250, op: "obj", text: "Verifying authorization scope before any probe.", status: "thinking" },
  { d: 900, op: "thought", id: "l1", t: "14:02:11", tone: "ok", text: "authorization verified · signature 19e05d" },
  { d: 600, op: "obj", text: "Mapping the attack surface.", status: "streaming" },
  { d: 400, op: "toolStart", id: "t1", cmd: "subfinder -d legalplus.ma -silent" },
  { d: 1700, op: "toolDone", id: "t1", result: "34 hosts resolved" },
  { d: 400, op: "toolStart", id: "t2", cmd: "httpx -l hosts.txt -silent -status-code -title" },
  { d: 1600, op: "toolDone", id: "t2", result: "118 live endpoints" },
  { d: 500, op: "thought", id: "l2", t: "14:02:48", tone: "dim", text: "recon complete · 34 hosts, 118 endpoints" },
  { d: 150, op: "count", hosts: 34, endpoints: 118 },
  { d: 500, op: "plan", key: "recon", state: "done" },
  { d: 150, op: "plan", key: "fingerprint", state: "running" },
  { d: 200, op: "phase", idx: 1 },
  { d: 400, op: "obj", text: "Fingerprinting the stack.", status: "streaming" },
  { d: 400, op: "toolStart", id: "t3", cmd: "nuclei -tags tech -silent -u office.legalplus.co" },
  { d: 1700, op: "toolDone", id: "t3", result: "nginx 1.18 · wordpress 6.4 · express 4.17" },
  { d: 450, op: "thought", id: "l3", t: "14:06:02", tone: "dim", text: "fingerprint · nginx 1.18, wordpress 6.4, express 4.17" },
  { d: 500, op: "plan", key: "fingerprint", state: "done" },
  { d: 150, op: "plan", key: "vuln", state: "running" },
  { d: 200, op: "phase", idx: 2 },
  { d: 400, op: "obj", text: "Hunting for broken object level authorization.", status: "thinking" },
  { d: 500, op: "plan", key: "probe-idor", state: "running", show: true },
  { d: 900, op: "thought", id: "l4", t: "14:09:37", tone: "warn", text: "candidate IDOR on /api/documents/{id}" },
  { d: 300, op: "cand", id: "f-idor", sev: "crit", endpoint: "GET https://office.legalplus.co/api/documents/{id}", label: "Mass IDOR on the document service" },
  { d: 150, op: "count", candidates: 1 },
  { d: 500, op: "obj", text: "Validating the IDOR with a non-destructive read.", status: "streaming" },
  { d: 200, op: "phase", idx: 3 },
  { d: 150, op: "plan", key: "vuln", state: "done" },
  { d: 150, op: "plan", key: "exploit", state: "running" },
  { d: 400, op: "toolStart", id: "t4", cmd: "curl -s -H 'Authorization: Bearer act_904' .../api/documents/10422 -o probe.pdf" },
  { d: 1900, op: "toolDone", id: "t4", result: "200 · pdf owned by account 118, read while authenticated as account 904" },
  { d: 500, op: "thought", id: "l5", t: "14:11:04", tone: "bad", text: "confirmed · foreign object read, non-destructive probe" },
  { d: 350, op: "confirm", id: "f-idor", code: "770AB6", cvss: 9.1, proof: "Read account 118's identity document while authenticated as account 904. No write, no delete.", planDone: "probe-idor" },
  { d: 150, op: "count", confirmed: 1 },
  { d: 600, op: "obj", text: "Following the public uploads path flagged during recon.", status: "streaming" },
  { d: 400, op: "toolStart", id: "t5", cmd: "curl -s https://legalplus.ma/wp-content/uploads/2026/01/" },
  { d: 1600, op: "toolDone", id: "t5", result: "directory index served · output-28.pdf reachable with no auth" },
  { d: 300, op: "cand", id: "f-exp", sev: "crit", endpoint: "GET https://legalplus.ma/wp-content/uploads/2026/01/output-28.pdf", label: "Signed contract exposed without auth" },
  { d: 150, op: "count", candidates: 2 },
  { d: 700, op: "thought", id: "l6", t: "14:12:20", tone: "bad", text: "confirmed · countersigned engagement letter served 200" },
  { d: 300, op: "confirm", id: "f-exp", code: "580862", cvss: 9.3, proof: "output-28.pdf is a countersigned engagement letter, downloadable with no session." },
  { d: 150, op: "count", confirmed: 2 },
  { d: 600, op: "obj", text: "Testing the attachment endpoint for unrestricted upload.", status: "thinking" },
  { d: 500, op: "plan", key: "probe-upload", state: "running", show: true },
  { d: 900, op: "thought", id: "l7", t: "14:13:29", tone: "warn", text: "candidate unrestricted upload on /api/attachments" },
  { d: 300, op: "cand", id: "f-up", sev: "high", endpoint: "POST https://office.legalplus.co/api/attachments", label: "Unrestricted upload accepts php" },
  { d: 150, op: "count", candidates: 3 },
  { d: 500, op: "obj", text: "Uploading a benign probe with a spoofed content type.", status: "streaming" },
  { d: 400, op: "toolStart", id: "t6", cmd: "curl -s -F 'file=@probe.php;type=image/png' .../api/attachments" },
  { d: 1900, op: "toolDone", id: "t6", result: "stored · fetching the path executed the probe string" },
  { d: 500, op: "thought", id: "l8", t: "14:15:50", tone: "bad", text: "confirmed · php handler executed probe string" },
  { d: 350, op: "confirm", id: "f-up", code: "A11C34", cvss: 8.4, proof: "probe.php stored under a servable path and executed on fetch. the client MIME type was trusted.", planDone: "probe-upload" },
  { d: 150, op: "count", confirmed: 3 },
  { d: 600, op: "obj", text: "Checking whether registration leaks the id space.", status: "streaming" },
  { d: 400, op: "toolStart", id: "t7", cmd: "curl -s -X POST .../api/register -d @account.json" },
  { d: 1600, op: "toolDone", id: "t7", result: "201 · new ids 9041, 9042, 9043 issued in sequence" },
  { d: 300, op: "cand", id: "f-reg", sev: "high", endpoint: "POST https://office.legalplus.co/api/register", label: "Open registration plus sequential user ids" },
  { d: 150, op: "count", candidates: 4 },
  { d: 600, op: "thought", id: "l9", t: "14:16:03", tone: "bad", text: "confirmed · self registration open, ids run in sequence" },
  { d: 300, op: "confirm", id: "f-reg", code: "3F90AB", cvss: 7.5, proof: "Registration succeeds with no invite. sequential ids let the whole customer base be walked." },
  { d: 150, op: "count", confirmed: 4 },
  { d: 700, op: "obj", text: "Continuing to validate the remaining candidates.", status: "streaming" },
  { d: 400, op: "thought", id: "l10", t: "14:16:12", tone: "dim", text: "validating 6 remaining candidates…" },
  { d: 150, op: "count", candidates: 10, score: 24 },
  { d: 150, op: "validating" },
];

function apply(s: RunState, e: Ev): RunState {
  switch (e.op) {
    case "obj":
      return { ...s, objective: e.text, status: e.status ?? s.status };
    case "phase":
      return { ...s, phaseIdx: e.idx };
    case "plan":
      return {
        ...s,
        plan: { ...s.plan, [e.key]: e.state },
        shown: e.show ? { ...s.shown, [e.key]: true } : s.shown,
      };
    case "thought":
      return { ...s, rows: [...s.rows, { kind: "thought", id: e.id, t: e.t, tone: e.tone, text: e.text }] };
    case "toolStart":
      return { ...s, rows: [...s.rows, { kind: "tool", id: e.id, cmd: e.cmd, running: true }] };
    case "toolDone":
      return {
        ...s,
        rows: s.rows.map((r) =>
          r.kind === "tool" && r.id === e.id ? { ...r, running: false, result: e.result } : r,
        ),
      };
    case "cand":
      return {
        ...s,
        rows: [
          ...s.rows,
          { kind: "finding", id: e.id, stage: "candidate", sev: e.sev, endpoint: e.endpoint, label: e.label },
        ],
      };
    case "confirm":
      return {
        ...s,
        plan: e.planDone ? { ...s.plan, [e.planDone]: "done" } : s.plan,
        rows: s.rows.map((r) =>
          r.kind === "finding" && r.id === e.id
            ? { ...r, stage: "confirmed", code: e.code, cvss: e.cvss, proof: e.proof }
            : r,
        ),
      };
    case "count":
      return {
        ...s,
        hosts: e.hosts ?? s.hosts,
        endpoints: e.endpoints ?? s.endpoints,
        score: e.score ?? s.score,
        candidates: e.candidates ?? s.candidates,
        confirmed: e.confirmed ?? s.confirmed,
      };
    case "validating":
      return { ...s, validating: true };
    default:
      return s;
  }
}

function buildStateUpTo(n: number): RunState {
  let s = INITIAL;
  for (let i = 0; i < n && i < EVENTS.length; i++) s = apply(s, EVENTS[i]);
  return s;
}

/* ---------- small presentational pieces ---------- */
function Stat({ label, value, danger }: { label: string; value: ReactNode; danger?: boolean }) {
  return (
    <div className="rounded-cell bg-surface-2 p-3">
      <div className="text-fine text-ink-2">{label}</div>
      <div className={cn("mt-1 text-title u-num", danger ? "text-danger" : "text-ink")}>{value}</div>
    </div>
  );
}

function ToolRow({ r }: { r: Extract<Row, { kind: "tool" }> }) {
  return (
    <div className="relative overflow-hidden rounded-cell border-t-2 border-line bg-surface-2 p-3">
      {!r.running && <span className="u-sweep" />}
      <div className="flex items-center gap-2">
        <Terminal size={13} className="shrink-0 text-ink-2" />
        <span className="min-w-0 flex-1 break-all font-mono text-fine text-ink">{r.cmd}</span>
        <span className="shrink-0">
          {r.running ? (
            <span className="inline-flex items-center gap-2 font-mono text-fine text-ink-2">
              running
              <Dots />
            </span>
          ) : (
            <span className="inline-flex items-center gap-1 font-mono text-fine text-ok">
              <Check size={12} />
              done
            </span>
          )}
        </span>
      </div>
      {!r.running && r.result && (
        <div className="mt-1 break-all font-mono text-fine text-ink-2">{r.result}</div>
      )}
    </div>
  );
}

function FindingRow({ r }: { r: Extract<Row, { kind: "finding" }> }) {
  const confirmed = r.stage === "confirmed";
  return (
    <div
      className={cn(
        "relative overflow-hidden rounded-cell border-t-2 bg-surface-2 p-3",
        confirmed ? SEV_BORDER[r.sev] : "border-warn",
      )}
    >
      {confirmed && <span className="u-sweep" />}
      <div className="flex flex-wrap items-center gap-2">
        {confirmed ? (
          <span className={chipLike(SEV_TONE[r.sev])}>{SEV_LABEL[r.sev]}</span>
        ) : (
          <span className={chipLike("warn")}>candidate</span>
        )}
        {r.code && <Mono>{r.code}</Mono>}
      </div>
      <div className="mt-1 text-small text-ink">{r.label}</div>
      <div className="mt-1 break-all font-mono text-fine text-ink-2">{r.endpoint}</div>
      {confirmed && r.cvss != null && (
        <div className="mt-1 font-mono text-fine">
          <span className="text-ink-2">CVSS </span>
          <span className="u-num text-danger">
            <CvssCount to={r.cvss} />
          </span>
        </div>
      )}
    </div>
  );
}

function EvidenceCard({ r }: { r: Extract<Row, { kind: "finding" }> }) {
  return (
    <div className={cn("rounded-panel border-t-2 bg-well p-4 u-flash", SEV_BORDER[r.sev])}>
      <div className="flex items-center gap-2">
        <span className={chipLike(SEV_TONE[r.sev])}>{SEV_LABEL[r.sev]}</span>
        {r.code && <Mono>{r.code}</Mono>}
        <span className="ml-auto font-mono text-fine text-danger u-num">
          CVSS {r.cvss != null && <CvssCount to={r.cvss} />}
        </span>
      </div>
      <div className="mt-2 text-small text-ink">{r.label}</div>
      <div className="mt-1 break-all font-mono text-fine text-ink-2">{r.endpoint}</div>
      {r.proof && <div className="mt-2 text-fine leading-relaxed text-ink-2">{r.proof}</div>}
    </div>
  );
}

/* ---------- the screen ---------- */
export function PentasioGraph() {
  const reduced = useMemo(
    () =>
      typeof window !== "undefined" &&
      typeof window.matchMedia === "function" &&
      window.matchMedia("(prefers-reduced-motion: reduce)").matches,
    [],
  );

  const [n, setN] = useState(reduced ? EVENTS.length : 0);
  const [playing, setPlaying] = useState(!reduced);

  const scrollRef = useRef<HTMLDivElement>(null);
  const stick = useRef(true);

  /* advance one event at a time; each carries its own delay before it applies */
  useEffect(() => {
    if (!playing) return;
    if (n >= EVENTS.length) {
      setPlaying(false);
      return;
    }
    const id = window.setTimeout(() => setN((v) => v + 1), EVENTS[n].d);
    return () => window.clearTimeout(id);
  }, [playing, n]);

  const state = useMemo(() => buildStateUpTo(n), [n]);

  /* keep the transcript pinned to the newest line unless the reader scrolled up */
  useEffect(() => {
    const el = scrollRef.current;
    if (el && stick.current) el.scrollTop = el.scrollHeight;
  }, [n]);

  const onScroll = () => {
    const el = scrollRef.current;
    if (!el) return;
    stick.current = el.scrollHeight - el.scrollTop - el.clientHeight < 48;
  };

  const replay = () => {
    stick.current = true;
    setN(0);
    setPlaying(true);
  };
  const jumpToNow = () => {
    setPlaying(false);
    setN(EVENTS.length);
  };

  const live = n < EVENTS.length;
  const statusTone = state.status === "streaming" ? "success" : "info";
  const statusLabel = state.status === "streaming" ? "streaming" : "thinking";

  const planItems = PLAN.filter((p) => !p.sub || state.shown[p.key]);
  const planDone = planItems.filter((p) => state.plan[p.key] === "done").length;

  const proven = state.rows.filter(
    (r): r is Extract<Row, { kind: "finding" }> => r.kind === "finding" && r.stage === "confirmed",
  );
  const evidence = [...proven].reverse();

  const lastId = state.rows.length ? state.rows[state.rows.length - 1].id : null;

  return (
    <div className="flex flex-col gap-6">
      {/* ----- header rail ----- */}
      <Panel strong className="p-6">
        <div className="flex flex-wrap items-start justify-between gap-4">
          <div className="min-w-0">
            <Eyebrow>Autonomous run</Eyebrow>
            <div className="mt-1 flex flex-wrap items-center gap-2 text-body text-ink">
              <span className="font-medium">legalplus.ma</span>
              <span className="text-ink-2">·</span>
              <Mono>signature 19e05d</Mono>
              <span className="text-ink-2">·</span>
              <span className="text-small text-ink-2">Deep mode</span>
            </div>
          </div>
          <Status tone={statusTone} pulse className="shrink-0">
            {statusLabel}
          </Status>
        </div>

        {/* phases stepper, driven by the live phase index */}
        <div className="mt-6 grid grid-cols-2 gap-2 sm:grid-cols-5">
          {PHASES.map((p, i) => {
            const done = i < state.phaseIdx;
            const active = i === state.phaseIdx;
            const border = done ? "border-ok" : active ? "border-accent" : "border-line";
            const tone = done ? "text-ok" : active ? "text-accent" : "text-ink-2";
            const label = done ? "Done" : active ? "In progress" : "Pending";
            return (
              <div key={p} className={cn("rounded-cell border-t-2 bg-surface-2 p-3", border)}>
                <div className={cn("text-fine font-medium", tone)}>{label}</div>
                <div className="mt-1 text-small text-ink">{p}</div>
              </div>
            );
          })}
        </div>

        {/* counters: single-step reveals count up, running tallies stay plain */}
        <div className="mt-4 grid grid-cols-2 gap-3 sm:grid-cols-5">
          <Stat label="Hosts" value={<CountUp to={state.hosts} />} />
          <Stat label="Endpoints" value={<CountUp to={state.endpoints} />} />
          <Stat label="Candidates" value={state.candidates} />
          <Stat label="Confirmed" value={state.confirmed} danger />
          <Stat label="Risk score" value={<CountUp to={state.score} />} danger />
        </div>
      </Panel>

      {/* ----- body: plan / transcript / evidence ----- */}
      <div className="grid grid-cols-1 gap-6 lg:grid-cols-12">
        {/* PLAN */}
        <Panel className="flex h-[34rem] flex-col p-6 lg:col-span-3">
          <div className="flex items-center justify-between gap-2">
            <Eyebrow>Plan</Eyebrow>
            <Status tone={planDone === planItems.length ? "success" : "info"} pulse={live}>
              {planDone}/{planItems.length}
            </Status>
          </div>
          <div className="mt-4 min-h-0 flex-1 overflow-y-auto">
            {planItems.map((p) => {
              const st = state.plan[p.key];
              return (
                <div
                  key={p.key}
                  className={cn(
                    "relative flex items-start gap-3 overflow-hidden rounded-cell px-2 py-2",
                    p.sub && "ml-4 p-rise",
                  )}
                >
                  {st === "done" && <span className="u-sweep" />}
                  <span className="mt-1 shrink-0">
                    {st === "done" ? (
                      <Check size={14} className="text-ok" />
                    ) : st === "running" ? (
                      <Dots />
                    ) : (
                      <Circle size={14} className="text-ink-2" />
                    )}
                  </span>
                  <span className={cn("min-w-0 text-small", st === "pending" ? "text-ink-2" : "text-ink")}>
                    {p.label}
                  </span>
                </div>
              );
            })}
          </div>
        </Panel>

        {/* TRANSCRIPT (the star) */}
        <Panel strong className="relative flex h-[34rem] flex-col overflow-hidden lg:col-span-6">
          <div className="flex items-center justify-between gap-2 border-b border-line px-6 py-3">
            <Eyebrow>Agent transcript</Eyebrow>
            <Status tone="success" pulse>
              streaming
            </Status>
          </div>
          <div className="border-b border-line bg-well px-6 py-3">
            <div className="text-fine text-ink-2">Current objective</div>
            <div className="mt-1 flex items-center gap-2 text-small text-ink">
              {state.objective}
              {live && state.status === "thinking" && <Dots />}
            </div>
          </div>

          <div
            ref={scrollRef}
            onScroll={onScroll}
            className="u-grid-bg flex min-h-0 flex-1 flex-col gap-3 overflow-y-auto px-6 pt-4 pb-24"
          >
            {state.rows.length === 0 && (
              <div className="flex items-center gap-2 font-mono text-fine text-ink-2">
                initializing run
                <Dots />
              </div>
            )}
            {state.rows.map((r) => {
              if (r.kind === "thought") {
                const tail = live && r.id === lastId;
                return (
                  <div key={r.id} className="flex gap-3 font-mono text-small">
                    <span className="shrink-0 text-ink-2 u-num">{r.t}</span>
                    <span className={logText(r.tone)}>
                      {r.text}
                      {tail && <Dots />}
                    </span>
                  </div>
                );
              }
              if (r.kind === "tool") return <ToolRow key={r.id} r={r} />;
              return <FindingRow key={r.id} r={r} />;
            })}
            {/* persistent caret: the agent never signs off */}
            <span className="inline-block h-4 w-2 shrink-0 bg-accent animate-pulse" aria-hidden />
          </div>

          {/* run controls float over the stream, the way the graph page's tools do */}
          <ToolTower className="absolute bottom-4 right-4 z-10">
            <ToolButton label="Replay" icon={<RotateCcw size={16} />} active={playing} onClick={replay} />
            <ToolDivider />
            <ToolButton
              label="Jump to now"
              icon={<ChevronsRight size={16} />}
              active={!live}
              onClick={jumpToNow}
            />
          </ToolTower>
        </Panel>

        {/* EVIDENCE */}
        <Panel className="flex h-[34rem] flex-col p-6 lg:col-span-3">
          <div className="flex items-center justify-between gap-2">
            <Eyebrow>Proven findings</Eyebrow>
            <span className="u-num text-small text-ink">{proven.length}</span>
          </div>
          <div className="mt-4 flex min-h-0 flex-1 flex-col gap-3 overflow-y-auto">
            {evidence.length === 0 && (
              <div className="text-fine text-ink-2">No findings proven yet. Watching the run.</div>
            )}
            {evidence.map((r) => (
              <EvidenceCard key={r.id} r={r} />
            ))}
          </div>
          {state.validating && (
            <div className="mt-4 flex items-center gap-2 border-t border-line pt-4 text-fine text-ink-2">
              validating 6 remaining candidates
              <Dots />
            </div>
          )}
        </Panel>
      </div>
    </div>
  );
}

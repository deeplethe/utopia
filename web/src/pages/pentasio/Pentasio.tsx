/* Pentasio, rebuilt on Utopia's component system.

   This is a straight port of the original Pentasio security-dashboard mockup, but
   every raw element has been swapped for a Utopia primitive: <button> becomes
   Button / IconButton / Row, the hand-rolled hover / transition CSS becomes the
   motion the components already carry, Pentasio's brand blue and hard-coded hexes
   become Utopia's neutral chrome plus semantic tones, and the inline SVG icons
   become lucide glyphs drawn in currentColor. Nothing here writes a hover, a
   transition, a radius by hand, or a colour value: the design system owns all of
   it. That is the whole point of the exercise, so the difference is visible on
   every hover, focus and screen change.

   One standalone top-level route (/pentasio). All sub-navigation is local React
   state, so it renders without the app Shell, auth, or the backend. */
import { useEffect, useRef, useState } from "react";
import type { ReactNode } from "react";
import {
  Activity,
  ArrowRight,
  Bot,
  Check,
  ChevronLeft,
  ChevronRight,
  Copy,
  Download,
  FileText,
  Globe,
  LayoutGrid,
  Moon,
  Network,
  Play,
  Search,
  Send,
  Settings as SettingsIcon,
  ShieldAlert,
  Sparkles,
  Sun,
  TrendingUp,
  Upload,
} from "lucide-react";
import {
  Button,
  Chip,
  Checkbox,
  ChoiceCard,
  IconButton,
  Input,
  LinkButton,
  PageHeader,
  Panel,
  RAIL_CLS,
  Row,
  RailItem,
  ROW_HOVER,
  Segmented,
  SettingsCard,
  Status,
  Textarea,
  chipLike,
  cn,
} from "../../ui";
import { Table, TBody, THead, Th, Tr, Td } from "../../ui/table";
import { Dialog } from "../../ui/dialog";
import { PentasioGraph } from "./PentasioGraph";
import { resolvedTheme, setTheme, type Resolved } from "../../theme";
import { toast } from "../../toast";
import {
  AI_REPLY,
  COUNTS,
  CURRENT_PHASE,
  DOMAINS,
  FINDINGS,
  LOGS,
  PANEL_SEED,
  PHASES,
  REPORTS,
  SCANS,
  SCAN_TONE,
  SEV_BAR_BG,
  SEV_LABEL,
  SEV_ORDER,
  SEV_TONE,
  TOTAL_OPEN,
  type Report,
  type Sev,
} from "./data";

/* ------------------------------------------------------------------ *
 *  types + tiny presentational helpers
 * ------------------------------------------------------------------ */
type Screen =
  | "dashboard"
  | "domains"
  | "scans"
  | "scandetail"
  | "findings"
  | "finding"
  | "reports"
  | "graph"
  | "agent"
  | "settings";

type Msg = { role: "user" | "ai"; text: string };
type SevFilter = "all" | Sev;

type Ctx = {
  go: (s: Screen) => void;
  openFinding: (i: number) => void;
  openPanel: () => void;
  openModal: () => void;
  variant: "score" | "feed";
  setVariant: (v: "score" | "feed") => void;
  sel: number;
  tab: "fix" | "evidence" | "history";
  setTab: (t: "fix" | "evidence" | "history") => void;
  fStatus: "Open" | "Fixed" | "Accepted";
  setFStatus: (s: "Open" | "Fixed" | "Accepted") => void;
  markFixed: () => void;
  sevFilter: SevFilter;
  setSevFilter: (s: SevFilter) => void;
  openSev: (s: Sev) => void;
  domFilter: string;
  setDomFilter: (s: string) => void;
  scanPct: number;
  scanStopped: boolean;
  stopScan: () => void;
  msgs: Msg[];
  input: string;
  setInput: (s: string) => void;
  send: () => void;
  ask: (q: string) => void;
  settingsTab: string;
  setSettingsTab: (s: string) => void;
};

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

function Cvss({ v }: { v: string }) {
  return <Mono className="u-num">CVSS {v}</Mono>;
}

function SevChip({ sev }: { sev: Sev }) {
  return <Chip tone={SEV_TONE[sev]}>{SEV_LABEL[sev]}</Chip>;
}

function Dot({ className }: { className: string }) {
  return <span className={cn("h-1.5 w-1.5 shrink-0 rounded-full", className)} />;
}

/** The stacked severity bar: one flex segment per severity, width by count. */
function SevBar({ counts }: { counts: Record<Sev, number> }) {
  return (
    <div className="flex h-2 overflow-hidden rounded-full bg-well">
      {SEV_ORDER.filter((s) => counts[s] > 0).map((s) => (
        <div key={s} className={SEV_BAR_BG[s]} style={{ flexGrow: counts[s] }} />
      ))}
    </div>
  );
}

/** Thin progress bar. Fill is drawn in the accent token, track in the well. */
function ProgressBar({ pct }: { pct: number }) {
  return (
    <div className="h-2 w-full overflow-hidden rounded-full bg-well">
      <div className="h-full rounded-full bg-accent" style={{ width: `${pct}%` }} />
    </div>
  );
}

/** The 132px donut gauge. SVG strokes ride on currentColor, so the arc and track
    take their colour from token text classes, never a raw hex. */
function Donut({ score }: { score: number }) {
  const r = 52;
  const c = 2 * Math.PI * r;
  const offset = c * (1 - score / 100);
  return (
    <div className="relative shrink-0" style={{ width: 132, height: 132 }}>
      <svg viewBox="0 0 132 132" width={132} height={132} className="-rotate-90">
        <circle cx={66} cy={66} r={r} fill="none" strokeWidth={13} stroke="currentColor" className="text-line" />
        <circle
          cx={66}
          cy={66}
          r={r}
          fill="none"
          strokeWidth={13}
          strokeLinecap="round"
          stroke="currentColor"
          className="text-danger"
          strokeDasharray={c}
          strokeDashoffset={offset}
        />
      </svg>
      <div className="absolute inset-0 flex flex-col items-center justify-center">
        <div className="text-display u-num text-ink">{score}</div>
        <div className="text-fine text-ink-2">of 100</div>
      </div>
    </div>
  );
}

/** A card that lifts nothing but changes surface on hover. ROW_HOVER carries the
    system's hover + transition, imported rather than written, so this page never
    spells a hover: utility itself. */
function ClickCard({
  onClick,
  className,
  children,
}: {
  onClick: () => void;
  className?: string;
  children: ReactNode;
}) {
  return (
    <div
      role="button"
      tabIndex={0}
      onClick={onClick}
      onKeyDown={(e) => {
        if (e.key === "Enter" || e.key === " ") {
          e.preventDefault();
          onClick();
        }
      }}
      className={cn(
        "cursor-pointer rounded-panel border border-line bg-surface p-4",
        ROW_HOVER,
        className,
      )}
    >
      {children}
    </div>
  );
}

/* ------------------------------------------------------------------ *
 *  shell: sidebar + header
 * ------------------------------------------------------------------ */
function Sidebar({ c, screen }: { c: Ctx; screen: Screen }) {
  const riskOpen = screen === "findings" || screen === "finding" || screen === "reports";
  return (
    <aside className={cn(RAIL_CLS, "sticky top-0 flex h-screen flex-col")}>
      <div className="flex items-center gap-2 px-3 py-4">
        <span className="grid h-8 w-8 place-items-center rounded-control border border-line bg-surface-2 font-mono text-small text-ink">
          {">_"}
        </span>
        <span
          className="text-title text-ink"
          style={{ fontFamily: "var(--font-brand)", letterSpacing: "0.01em" }}
        >
          Pentasio
        </span>
      </div>

      <nav className="flex flex-1 flex-col gap-1 px-3">
        <RailItem
          icon={<LayoutGrid size={16} />}
          active={screen === "dashboard"}
          onClick={() => c.go("dashboard")}
        >
          Dashboard
        </RailItem>
        <RailItem
          icon={<Globe size={16} />}
          count={8}
          active={screen === "domains"}
          onClick={() => c.go("domains")}
        >
          Targets
        </RailItem>
        <RailItem
          icon={<Activity size={16} />}
          dot="bg-ok"
          active={screen === "scans" || screen === "scandetail"}
          onClick={() => c.go("scans")}
        >
          Activity
        </RailItem>
        <Row
          density="nav"
          icon={<ShieldAlert size={16} />}
          active={riskOpen}
          onClick={() => c.go("findings")}
          trailing={<span className={chipLike("danger", "u-num")}>2 crit</span>}
        >
          Risk
        </Row>
        {riskOpen && (
          <div className="ml-4 flex flex-col gap-1 border-l border-line pl-1 u-rise">
            <Row
              density="nav"
              flush
              active={screen === "findings" || screen === "finding"}
              onClick={() => c.go("findings")}
              trailing={<span className="u-num text-fine text-ink-2">178</span>}
            >
              Findings
            </Row>
            <Row
              density="nav"
              flush
              active={screen === "reports"}
              onClick={() => c.go("reports")}
              trailing={<span className="u-num text-fine text-ink-2">3</span>}
            >
              Reports
            </Row>
          </div>
        )}

        <RailItem
          icon={<Network size={16} />}
          active={screen === "graph"}
          onClick={() => c.go("graph")}
        >
          Attack surface
        </RailItem>

        <div className="flex-1" />

        <RailItem
          icon={<Bot size={16} />}
          active={screen === "agent"}
          onClick={() => c.go("agent")}
        >
          AI Agent
        </RailItem>
        <RailItem
          icon={<SettingsIcon size={16} />}
          active={screen === "settings"}
          onClick={() => c.go("settings")}
        >
          Settings
        </RailItem>
      </nav>

      <div className="flex items-center gap-2 border-t border-line px-3 py-3">
        <span className="grid h-9 w-9 shrink-0 place-items-center rounded-full bg-surface-3 text-small font-medium text-ink">
          TC
        </span>
        <div className="min-w-0">
          <div className="truncate text-small font-medium text-ink">Test Client</div>
          <div className="truncate text-fine text-ink-2">Test Workspace</div>
        </div>
      </div>
    </aside>
  );
}

function Header({
  c,
  theme,
  onToggleTheme,
}: {
  c: Ctx;
  theme: Resolved;
  onToggleTheme: () => void;
}) {
  const [q, setQ] = useState("");
  const [searchOpen, setSearchOpen] = useState(false);
  const query = q.trim().toLowerCase();

  const findingHits = query
    ? FINDINGS.map((f, i) => ({ f, i }))
        .filter(({ f }) =>
          [f.code, f.title, f.domain].some((v) => v.toLowerCase().includes(query)),
        )
        .slice(0, 6)
    : [];
  const domainHits = query
    ? DOMAINS.filter((d) => d.name.toLowerCase().includes(query)).slice(0, 4)
    : [];
  const scanHits = query
    ? SCANS.filter(
        (s) =>
          s.id.toLowerCase().includes(query) ||
          s.target.toLowerCase().includes(query),
      ).slice(0, 4)
    : [];
  const hasHits = findingHits.length + domainHits.length + scanHits.length > 0;

  const chooseFinding = (i: number) => {
    c.openFinding(i);
    setSearchOpen(false);
  };

  return (
    <header className="glass-strong sticky top-0 z-30 flex flex-wrap items-center gap-3 border-x-0 border-t-0 px-8 py-3">
      <div className="flex min-w-0 flex-1 items-center" style={{ maxWidth: "28rem" }}>
        <Input
          icon={<Search size={15} />}
          placeholder="Search domains, scans, findings…"
          aria-label="Search"
          value={q}
          onChange={(e) => setQ(e.target.value)}
          onFocus={() => setSearchOpen(true)}
          onKeyDown={(e) => {
            if (e.key === "Enter") {
              e.preventDefault();
              setSearchOpen(true);
            }
            if (e.key === "Escape") setSearchOpen(false);
          }}
          className="w-full"
        />
      </div>
      <div className="ml-auto flex flex-wrap items-center gap-2">
        <Status tone="success">Production</Status>
        <Button
          variant="secondary"
          size="sm"
          icon={theme === "dark" ? <Sun size={15} /> : <Moon size={15} />}
          onClick={onToggleTheme}
        >
          {theme === "dark" ? "Light" : "Dark"}
        </Button>
        <Button variant="secondary" size="sm" icon={<Sparkles size={15} />} onClick={c.openPanel}>
          Ask AI
        </Button>
        <Button variant="primary" size="sm" icon={<Play size={15} />} onClick={c.openModal}>
          New scan
        </Button>
      </div>

      <Dialog
        open={searchOpen}
        onOpenChange={setSearchOpen}
        title="Search"
        description="Jump to a domain, scan or finding."
        closeLabel="Close search"
        width="md"
        footer={
          <Button variant="secondary" size="sm" onClick={() => setSearchOpen(false)}>
            Close
          </Button>
        }
      >
        <Input
          autoFocus
          icon={<Search size={15} />}
          placeholder="Type to search…"
          aria-label="Search the workspace"
          value={q}
          onChange={(e) => setQ(e.target.value)}
          onKeyDown={(e) => {
            if (e.key === "Enter" && findingHits[0]) chooseFinding(findingHits[0].i);
          }}
          className="w-full"
        />
        <div className="mt-4 flex max-h-80 flex-col gap-1 overflow-y-auto">
          {!query ? (
            <p className="px-2 py-1 text-small text-ink-2">
              Start typing to search the workspace.
            </p>
          ) : null}

          {findingHits.map(({ f, i }) => (
            <Row key={`f-${f.code}`} density="list" onClick={() => chooseFinding(i)}>
              <span className="min-w-0 flex-1 truncate text-body text-ink">
                <span className="font-mono text-fine text-ink-2">{f.code}</span> {f.title}
              </span>
              <span className="shrink-0 text-fine text-ink-2">{f.domain}</span>
            </Row>
          ))}

          {domainHits.map((d) => (
            <Row
              key={`d-${d.name}`}
              density="list"
              onClick={() => {
                c.go("domains");
                setSearchOpen(false);
              }}
            >
              <span className="min-w-0 flex-1 truncate text-body text-ink">{d.name}</span>
              <span className="shrink-0 text-fine text-ink-2">Domain</span>
            </Row>
          ))}

          {scanHits.map((s) => (
            <Row
              key={`s-${s.id}`}
              density="list"
              onClick={() => {
                c.go(s.status === "Running" ? "scandetail" : "scans");
                setSearchOpen(false);
              }}
            >
              <span className="min-w-0 flex-1 truncate text-body text-ink">
                scan <span className="font-mono">{s.id}</span> · {s.target}
              </span>
              <span className="shrink-0 text-fine text-ink-2">{s.status}</span>
            </Row>
          ))}

          {query && !hasHits ? (
            <p className="px-2 py-1 text-small text-ink-2">
              No matches for “{q.trim()}”.
            </p>
          ) : null}
        </div>
      </Dialog>
    </header>
  );
}

/* ------------------------------------------------------------------ *
 *  dashboard
 * ------------------------------------------------------------------ */
function Dashboard({ c }: { c: Ctx }) {
  const actions = (
    <div className="flex items-center gap-2">
      <Segmented
        value={c.variant}
        onChange={c.setVariant}
        options={[
          { value: "score", label: "Score" },
          { value: "feed", label: "Triage" },
        ]}
      />
      <Button variant="secondary" size="sm" icon={<FileText size={15} />} onClick={() => c.go("reports")}>
        Latest report
      </Button>
    </div>
  );
  return (
    <div>
      <Eyebrow>Overview</Eyebrow>
      <PageHeader
        className="mb-6 mt-1"
        title="Security posture"
        sub="Wednesday, September 16 · last full sweep 2 days ago across 8 authorized targets."
        actions={actions}
      />
      {c.variant === "score" ? <ScoreDash c={c} /> : <TriageDash c={c} />}
    </div>
  );
}

function ScoreDash({ c }: { c: Ctx }) {
  const top3 = FINDINGS.slice(0, 3);
  const postureTop = DOMAINS.slice(0, 5);
  return (
    <div className="grid gap-6 lg:grid-cols-2">
      {/* left: risk score */}
      <Panel className="p-6">
        <div className="flex items-start justify-between gap-3">
          <Eyebrow>Risk score</Eyebrow>
          <Chip tone="danger">Needs attention</Chip>
        </div>
        <div className="mt-4 flex flex-wrap items-center gap-6">
          <Donut score={42} />
          <div className="min-w-0 flex-1">
            <div className="text-title text-ink">At risk</div>
            <p className="mt-1 text-small text-ink-2">
              Two critical findings on{" "}
              <span className="font-medium text-ink">legalplus.ma</span> expose customer
              identity documents without authentication.
            </p>
            <div className="mt-2 flex items-center gap-1 text-small text-danger">
              <TrendingUp size={14} /> 9 points worse than last week
            </div>
          </div>
        </div>
        <div className="my-6 border-t border-line" />
        <Eyebrow>178 open findings</Eyebrow>
        <div className="mt-3">
          <SevBar counts={COUNTS} />
        </div>
        <div className="mt-4 flex flex-wrap gap-2">
          {SEV_ORDER.map((s) => (
            <Chip key={s} tone={SEV_TONE[s]} onClick={() => c.openSev(s)}>
              <span className="inline-flex items-center gap-2">
                <Dot className={SEV_BAR_BG[s]} />
                <span className="u-num">{COUNTS[s]}</span> {SEV_LABEL[s].toLowerCase()}
              </span>
            </Chip>
          ))}
        </div>
      </Panel>

      {/* right column */}
      <div className="flex flex-col gap-6">
        <Panel className="p-6">
          <div className="flex items-start justify-between gap-3">
            <div>
              <div className="text-title text-ink">Fix these three first</div>
              <p className="mt-1 text-small text-ink-2">
                Ranked by exploitability and data exposure, not by CVSS alone.
              </p>
            </div>
            <LinkButton onClick={() => c.go("findings")}>
              <span className="inline-flex items-center gap-1">
                All findings <ArrowRight size={13} />
              </span>
            </LinkButton>
          </div>
          <div className="mt-4 flex flex-col gap-2">
            {top3.map((f, i) => (
              <div
                key={f.code}
                role="button"
                tabIndex={0}
                onClick={() => c.openFinding(i)}
                onKeyDown={(e) => {
                  if (e.key === "Enter") c.openFinding(i);
                }}
                className={cn(
                  "flex cursor-pointer items-start gap-3 rounded-cell p-3",
                  ROW_HOVER,
                )}
              >
                <span className="grid h-6 w-6 shrink-0 place-items-center rounded-full bg-surface-3 text-fine font-medium text-ink u-num">
                  {i + 1}
                </span>
                <div className="min-w-0 flex-1">
                  <div className="flex flex-wrap items-center gap-2">
                    <SevChip sev={f.sev} />
                    <Cvss v={f.cvss} />
                    <Mono>{f.domain}</Mono>
                  </div>
                  <div className="mt-1 truncate text-body font-medium text-ink">{f.title}</div>
                  <div className="mt-1 text-small text-ink-2">{f.impact}</div>
                </div>
                <ChevronRight size={16} className="mt-1 shrink-0 text-ink-2" />
              </div>
            ))}
          </div>
        </Panel>

        <div className="grid gap-6 sm:grid-cols-2">
          <Panel className="p-6">
            <div className="flex items-center justify-between gap-2">
              <div className="text-title text-ink">Targets by posture</div>
              <LinkButton onClick={() => c.go("domains")}>Manage</LinkButton>
            </div>
            <div className="mt-4 flex flex-col gap-3">
              {postureTop.map((d) => (
                <div key={d.name} className="flex items-center gap-2">
                  <Dot
                    className={cn(
                      d.band === "Healthy" ? "bg-ok" : SEV_BAR_BG[bandSev(d.band)],
                    )}
                  />
                  <span className="min-w-0 flex-1 truncate text-small text-ink">{d.name}</span>
                  <span className="text-fine text-ink-2">{d.open} open</span>
                  <span className={cn("u-num font-mono text-fine", bandText(d.band))}>
                    {d.score ?? "—"}
                  </span>
                </div>
              ))}
            </div>
          </Panel>

          <Panel className="p-6">
            <Status tone="success" pulse>
              <Eyebrow>Scan in progress</Eyebrow>
            </Status>
            <div className="mt-3 text-title text-ink">legalplus.ma · deep</div>
            <div className="mt-1 text-small text-ink-2">Phase 4 of 5, exploit validation</div>
            <div className="mt-4">
              <ProgressBar pct={c.scanPct} />
            </div>
            <div className="mt-2 flex items-center justify-between text-fine text-ink-2">
              <span className="u-num">{c.scanPct}% complete</span>
              <span>12 findings so far</span>
            </div>
            <Button
              variant="secondary"
              size="sm"
              className="mt-4 w-full"
              onClick={() => c.go("scandetail")}
            >
              Watch live
            </Button>
          </Panel>
        </div>
      </div>
    </div>
  );
}

function TriageDash({ c }: { c: Ctx }) {
  const feed = FINDINGS.slice(0, 5);
  const postureTop = DOMAINS.slice(0, 5);
  return (
    <div className="flex flex-col gap-6">
      <div className="grid gap-3" style={{ gridTemplateColumns: "repeat(auto-fit, minmax(150px, 1fr))" }}>
        {SEV_ORDER.map((s) => (
          <ClickCard key={s} onClick={() => c.openSev(s)}>
            <div className="flex items-center gap-2">
              <Dot className={SEV_BAR_BG[s]} />
              <Eyebrow>{SEV_LABEL[s]}</Eyebrow>
            </div>
            <div className="mt-2 text-display u-num text-ink">{COUNTS[s]}</div>
          </ClickCard>
        ))}
      </div>

      <div className="grid gap-6 lg:grid-cols-2">
        <Panel className="p-6">
          <div className="flex items-start justify-between gap-3">
            <div>
              <div className="text-title text-ink">Newest critical &amp; high</div>
              <p className="mt-1 text-small text-ink-2">Across all targets in your workspace</p>
            </div>
            <LinkButton onClick={() => c.go("findings")}>View all</LinkButton>
          </div>
          <div className="mt-4 flex flex-col gap-1">
            {feed.map((f, i) => (
              <div
                key={f.code}
                role="button"
                tabIndex={0}
                onClick={() => c.openFinding(i)}
                onKeyDown={(e) => {
                  if (e.key === "Enter") c.openFinding(i);
                }}
                className={cn("flex cursor-pointer items-center gap-3 rounded-cell p-3", ROW_HOVER)}
              >
                <SevChip sev={f.sev} />
                <div className="min-w-0 flex-1">
                  <div className="truncate text-body font-medium text-ink">{f.title}</div>
                  <div className="mt-1 truncate text-fine text-ink-2">
                    {f.domain} · scan {f.scan} · CVSS {f.cvss}
                  </div>
                </div>
                <span className="shrink-0 text-fine text-ink-2">{f.age}</span>
              </div>
            ))}
          </div>
        </Panel>

        <div className="flex flex-col gap-6">
          <Panel className="p-6">
            <div className="text-title text-ink">Worst targets</div>
            <div className="mt-4 flex flex-col gap-3">
              {postureTop.map((d) => (
                <div key={d.name} className="flex items-center gap-2">
                  <Dot className={cn(d.band === "Healthy" ? "bg-ok" : SEV_BAR_BG[bandSev(d.band)])} />
                  <span className="min-w-0 flex-1 truncate text-small text-ink">{d.name}</span>
                  <span className={cn("u-num font-mono text-fine", bandText(d.band))}>
                    {d.score ?? "—"}
                  </span>
                </div>
              ))}
            </div>
          </Panel>

          <div className="rounded-panel bg-ink p-6 text-ground">
            <div className="text-title text-ground">Run a fresh sweep</div>
            <p className="mt-1 text-small text-ground">
              Beta access includes one concurrent deep scan. Pick a verified target to start.
            </p>
            <Button variant="secondary" size="sm" className="mt-4" icon={<Play size={15} />} onClick={c.openModal}>
              New scan
            </Button>
          </div>
        </div>
      </div>
    </div>
  );
}

/* ------------------------------------------------------------------ *
 *  targets / domains
 * ------------------------------------------------------------------ */
function Domains({ c }: { c: Ctx }) {
  const filters = [
    { value: "All", label: "All", count: 8 },
    { value: "Authorized", label: "Authorized", count: 7 },
    { value: "Verified", label: "Verified", count: 8 },
    { value: "Pending", label: "Pending", count: 1 },
  ];
  const rows = DOMAINS.filter((d) => {
    if (c.domFilter === "Authorized") return d.status === "Authorized";
    if (c.domFilter === "Pending") return d.status === "Needs signature";
    return true;
  });
  return (
    <div>
      <Eyebrow>Targets</Eyebrow>
      <PageHeader
        className="mb-6 mt-1"
        title="Domains"
        sub="Verify ownership, sign a 90-day authorization, then scan."
        actions={
          <Button variant="primary" size="sm" icon={<Play size={15} />} onClick={c.openModal}>
            Add domain
          </Button>
        }
      />
      <div className="mb-4">
        <Segmented value={c.domFilter} onChange={c.setDomFilter} options={filters} />
      </div>
      <Panel className="overflow-hidden p-0">
        <Table>
          <THead>
            <Tr>
              <Th>Target</Th>
              <Th>Score</Th>
              <Th>Open</Th>
              <Th>Automation</Th>
              <Th className="text-right">Actions</Th>
            </Tr>
          </THead>
          <TBody>
            {rows.map((d) => (
              <Tr key={d.name}>
                <Td>
                  <Status tone={d.status === "Authorized" ? "success" : "warn"}>{d.name}</Status>
                  <div className="mt-1 text-fine text-ink-2">{d.auth}</div>
                </Td>
                <Td>
                  <div className={cn("text-title u-num", bandText(d.band))}>{d.score ?? "—"}</div>
                  <div className="text-fine text-ink-2">{d.band}</div>
                </Td>
                <Td>
                  <div className="text-small text-ink u-num">{d.open} open</div>
                  <div className="text-fine text-ink-2">{d.sevLine}</div>
                </Td>
                <Td>
                  <span className="text-small text-ink-2">Auto: {d.mode}</span>
                </Td>
                <Td>
                  <div className="flex items-center justify-end gap-2">
                    <Button variant="secondary" size="sm" onClick={() => c.go("scans")}>
                      History
                    </Button>
                    <Button
                      variant={d.status === "Authorized" ? "primary" : "secondary"}
                      size="sm"
                      onClick={c.openModal}
                    >
                      {d.status === "Authorized" ? "Launch scan" : "Authorize"}
                    </Button>
                  </div>
                </Td>
              </Tr>
            ))}
          </TBody>
        </Table>
      </Panel>
    </div>
  );
}

/* ------------------------------------------------------------------ *
 *  activity / scans
 * ------------------------------------------------------------------ */
function Scans({ c }: { c: Ctx }) {
  return (
    <div>
      <Eyebrow>Activity</Eyebrow>
      <PageHeader
        className="mb-6 mt-1"
        title="Scans"
        sub="1 running · 1 queued · 4 completed in the last 10 days"
        actions={
          <Button variant="primary" size="sm" icon={<Play size={15} />} onClick={c.openModal}>
            New scan
          </Button>
        }
      />
      <Panel className="overflow-hidden p-0">
        <Table>
          <THead>
            <Tr>
              <Th>Target</Th>
              <Th>Type</Th>
              <Th>Status</Th>
              <Th>Progress</Th>
              <Th className="text-right">Findings</Th>
              <Th>Started</Th>
              <Th />
            </Tr>
          </THead>
          <TBody>
            {SCANS.map((s) => (
              <Tr key={s.id} interactive onClick={() => s.status === "Running" && c.go("scandetail")}>
                <Td>
                  <div className="text-body font-medium text-ink">{s.target}</div>
                  <Mono>scan {s.id}</Mono>
                </Td>
                <Td>
                  <Chip tone="neutral">{s.type}</Chip>
                </Td>
                <Td>
                  <Status tone={SCAN_TONE[s.status]} pulse={s.status === "Running"}>
                    {s.status}
                  </Status>
                </Td>
                <Td>
                  <div className="w-28">
                    <ProgressBar pct={s.pct} />
                    <div className="mt-1 truncate text-fine text-ink-2">{s.phase}</div>
                  </div>
                </Td>
                <Td className="text-right">
                  <span className="u-num font-mono text-small text-ink">{s.findings}</span>
                </Td>
                <Td>
                  <span className="text-fine text-ink-2">{s.when}</span>
                </Td>
                <Td>
                  <LinkButton
                    onClick={(e) => {
                      e.stopPropagation();
                      if (s.status === "Running") c.go("scandetail");
                    }}
                  >
                    {s.cta}
                  </LinkButton>
                </Td>
              </Tr>
            ))}
          </TBody>
        </Table>
      </Panel>
    </div>
  );
}

function ScanDetail({ c }: { c: Ctx }) {
  const scanFindings = FINDINGS.filter((f) => f.scan === "19e05d");
  return (
    <div>
      <LinkButton onClick={() => c.go("scans")}>
        <span className="inline-flex items-center gap-1">
          <ChevronLeft size={14} /> All scans
        </span>
      </LinkButton>
      <div className="mt-3 flex flex-wrap items-center gap-2">
        <Status tone={c.scanStopped ? "warn" : "success"} pulse={!c.scanStopped}>
          {c.scanStopped ? "Stopped" : "Running"}
        </Status>
        <Mono>scan 19e05d</Mono>
      </div>
      <PageHeader
        className="mb-6 mt-2"
        title="legalplus.ma"
        sub={
          c.scanStopped
            ? "Deep scan · stopped by you · findings kept"
            : "Deep scan · started 14 minutes ago · est. 22 minutes remaining"
        }
        actions={
          <div className="flex items-center gap-2">
            <Button variant="secondary" size="sm" icon={<Sparkles size={15} />} onClick={c.openPanel}>
              Ask AI about this scan
            </Button>
            <Button
              variant="danger"
              size="sm"
              disabled={c.scanStopped}
              onClick={c.stopScan}
            >
              Stop scan
            </Button>
          </div>
        }
      />

      <Panel className="p-6">
        <div className="flex flex-wrap items-end justify-between gap-2">
          <div className="text-display u-num text-ink">{c.scanPct}%</div>
          <div className="text-small text-ink-2">12 findings surfaced so far · 4 need review</div>
        </div>
        <div className="mt-4">
          <ProgressBar pct={c.scanPct} />
        </div>
        <div
          className="mt-6 grid gap-2"
          style={{ gridTemplateColumns: "repeat(auto-fit, minmax(120px, 1fr))" }}
        >
          {PHASES.map((p, i) => {
            const done = i < CURRENT_PHASE;
            const active = i === CURRENT_PHASE;
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
      </Panel>

      <div className="mt-6 grid gap-6 lg:grid-cols-2">
        <Panel className="p-6">
          <div className="text-title text-ink">Findings from this scan</div>
          <div className="mt-4 flex flex-col gap-1">
            {scanFindings.map((f) => {
              const idx = FINDINGS.findIndex((x) => x.code === f.code);
              return (
                <div
                  key={f.code}
                  role="button"
                  tabIndex={0}
                  onClick={() => c.openFinding(idx)}
                  onKeyDown={(e) => {
                    if (e.key === "Enter") c.openFinding(idx);
                  }}
                  className={cn("flex cursor-pointer items-center gap-3 rounded-cell p-3", ROW_HOVER)}
                >
                  <SevChip sev={f.sev} />
                  <span className="min-w-0 flex-1 truncate text-body text-ink">{f.title}</span>
                  <Cvss v={f.cvss} />
                </div>
              );
            })}
          </div>
        </Panel>

        <div className="rounded-panel bg-well p-6">
          <div className="flex items-center justify-between gap-2">
            <Eyebrow>Live log</Eyebrow>
            <Status tone="success" pulse>
              streaming
            </Status>
          </div>
          <div className="mt-4 flex flex-col gap-2 font-mono text-fine">
            {LOGS.map((l, i) => (
              <div key={i} className="flex gap-3">
                <span className="shrink-0 text-ink-2">{l.t}</span>
                <span className={logText(l.k)}>{l.msg}</span>
              </div>
            ))}
          </div>
        </div>
      </div>
    </div>
  );
}

/* ------------------------------------------------------------------ *
 *  findings + finding detail
 * ------------------------------------------------------------------ */
function Findings({ c }: { c: Ctx }) {
  const filterOpts: { value: SevFilter; label: string; count: number }[] = [
    { value: "all", label: "All", count: TOTAL_OPEN },
    ...SEV_ORDER.map((s) => ({ value: s, label: SEV_LABEL[s], count: COUNTS[s] })),
  ];
  const rows =
    c.sevFilter === "all" ? FINDINGS : FINDINGS.filter((f) => f.sev === c.sevFilter);
  return (
    <div>
      <Eyebrow>Risk</Eyebrow>
      <PageHeader
        className="mb-6 mt-1"
        title="Findings"
        sub="178 open across 5 targets · sorted by severity, then by exposure"
        actions={
          <Button variant="secondary" size="sm" icon={<Sparkles size={15} />} onClick={c.openPanel}>
            Summarize with AI
          </Button>
        }
      />
      <div className="mb-4 overflow-x-auto">
        <Segmented value={c.sevFilter} onChange={c.setSevFilter} options={filterOpts} />
      </div>
      <Panel className="overflow-hidden p-0">
        <Table>
          <THead>
            <Tr>
              <Th>Code</Th>
              <Th>Severity</Th>
              <Th>Finding</Th>
              <Th>CVSS</Th>
              <Th>Age</Th>
              <Th />
            </Tr>
          </THead>
          <TBody>
            {rows.map((f) => {
              const idx = FINDINGS.findIndex((x) => x.code === f.code);
              return (
                <Tr key={f.code} interactive onClick={() => c.openFinding(idx)}>
                  <Td>
                    <Mono>{f.code}</Mono>
                  </Td>
                  <Td>
                    <SevChip sev={f.sev} />
                  </Td>
                  <Td>
                    <div className="text-body font-medium text-ink">{f.title}</div>
                    <div className="mt-1 text-fine text-ink-2">
                      {f.domain} · scan {f.scan}
                    </div>
                  </Td>
                  <Td>
                    <span className="u-num font-mono text-small text-ink">{f.cvss}</span>
                  </Td>
                  <Td>
                    <span className="text-fine text-ink-2">{f.age}</span>
                  </Td>
                  <Td>
                    <Button variant="secondary" size="sm" onClick={() => c.openFinding(idx)}>
                      Open
                    </Button>
                  </Td>
                </Tr>
              );
            })}
          </TBody>
        </Table>
      </Panel>
    </div>
  );
}

function FindingDetail({ c }: { c: Ctx }) {
  const f = FINDINGS[Math.min(c.sel, FINDINGS.length - 1)];

  const copyForJira = () => {
    const text = [
      `## ${f.code} ${f.title}`,
      `Severity: ${SEV_LABEL[f.sev]} · CVSS ${f.cvss}`,
      `Target: ${f.domain}`,
      `Endpoint: ${f.endpoint}`,
      "",
      f.impactLong,
      "",
      "### Fix",
      ...f.fix.map((step, i) => `${i + 1}. ${step.title} — ${step.body}`),
    ].join("\n");
    const p = navigator.clipboard?.writeText(text);
    if (!p) {
      toast.error("Clipboard is not available in this browser");
      return;
    }
    void p.then(
      () => toast.success("Copied for Jira"),
      () => toast.error("Could not copy to the clipboard"),
    );
  };
  return (
    <div style={{ maxWidth: "65rem" }}>
      <LinkButton onClick={() => c.go("findings")}>
        <span className="inline-flex items-center gap-1">
          <ChevronLeft size={14} /> All findings
        </span>
      </LinkButton>

      <div className="mt-3 flex flex-wrap items-center gap-2">
        <SevChip sev={f.sev} />
        <Mono>{f.code}</Mono>
        <span className="text-small text-ink-2">first seen {f.age}</span>
      </div>
      <h1 className="u-title mt-2 text-display text-ink" style={{ maxWidth: "34rem" }}>
        {f.title}
      </h1>

      <div className="mt-4 flex flex-wrap items-center gap-2">
        <Segmented
          value={c.fStatus}
          onChange={c.setFStatus}
          options={[
            { value: "Open", label: "Open" },
            { value: "Fixed", label: "Fixed" },
            { value: "Accepted", label: "Accepted" },
          ]}
        />
        <Button variant="secondary" size="sm" icon={<Sparkles size={15} />} onClick={c.openPanel}>
          Ask AI to draft the fix PR
        </Button>
      </div>

      <div
        className="mt-6 grid gap-3"
        style={{ gridTemplateColumns: "repeat(auto-fit, minmax(190px, 1fr))" }}
      >
        {[
          { k: "Severity", v: `${SEV_LABEL[f.sev]} · CVSS ${f.cvss}` },
          { k: "Target", v: f.domain },
          { k: "Found by", v: `scan ${f.scan}` },
          { k: "Status", v: c.fStatus },
        ].map((m) => (
          <Panel key={m.k} className="p-4">
            <Eyebrow>{m.k}</Eyebrow>
            <div className="mt-1 text-body text-ink">{m.v}</div>
          </Panel>
        ))}
      </div>

      <div className="mt-6">
        <Segmented
          value={c.tab}
          onChange={c.setTab}
          options={[
            { value: "fix", label: "Fix" },
            { value: "evidence", label: "Evidence" },
            { value: "history", label: "History" },
          ]}
        />
      </div>

      {c.tab === "fix" && (
        <div className="mt-4 grid gap-6 lg:grid-cols-2">
          <Panel className="p-6">
            <div className="text-title text-ink">How to fix it</div>
            <p className="mt-1 text-small text-ink-2">
              Estimated effort: {f.effort} · verified automatically by the next re-scan
            </p>
            <ol className="mt-4 flex flex-col gap-3">
              {f.fix.map((step, i) => (
                <li key={i} className="flex gap-3">
                  <span className="grid h-6 w-6 shrink-0 place-items-center rounded-full bg-surface-3 text-fine font-medium text-ink u-num">
                    {i + 1}
                  </span>
                  <div className="min-w-0">
                    <div className="text-body font-medium text-ink">{step.title}</div>
                    <div className="mt-1 text-small text-ink-2">{step.body}</div>
                  </div>
                </li>
              ))}
            </ol>
            <pre className="u-scroll mt-4 overflow-x-auto whitespace-pre-wrap rounded-cell bg-well p-4 font-mono text-fine text-ink-2">
              {f.snippet}
            </pre>
            <div className="mt-4 flex flex-wrap gap-2">
              <Button variant="primary" size="sm" icon={<Check size={15} />} onClick={c.markFixed}>
                Mark as fixed &amp; re-scan
              </Button>
              <Button variant="secondary" size="sm" icon={<Copy size={15} />} onClick={copyForJira}>
                Copy for Jira
              </Button>
            </div>
          </Panel>
          <div className="flex flex-col gap-6">
            <Panel className="p-6">
              <div className="text-title text-ink">Business impact</div>
              <p className="mt-2 text-small text-ink-2">{f.impactLong}</p>
            </Panel>
            <Panel className="p-6">
              <div className="text-title text-ink">Affected endpoint</div>
              <div className="u-scroll mt-2 overflow-x-auto rounded-cell bg-well p-4 font-mono text-fine text-ink-2" style={{ wordBreak: "break-all" }}>
                {f.endpoint}
              </div>
            </Panel>
          </div>
        </div>
      )}

      {c.tab === "evidence" && (
        <Panel className="mt-4 p-6">
          <div className="text-title text-ink">Reproduction</div>
          <p className="mt-1 text-small text-ink-2">
            Captured by the agent during exploit validation. Probes are non-destructive.
          </p>
          <pre className="u-scroll mt-4 overflow-x-auto whitespace-pre-wrap rounded-cell bg-well p-4 font-mono text-fine text-ink-2">
            {f.evidence}
          </pre>
        </Panel>
      )}

      {c.tab === "history" && (
        <Panel className="mt-4 p-6">
          <div className="text-title text-ink">Timeline</div>
          <div className="mt-4 flex flex-col gap-4">
            {[
              { when: "8 weeks ago", what: "Discovered during deep scan 19e05d", who: "Pentasio agent" },
              { when: "8 weeks ago", what: "Exploitability confirmed with a non-destructive probe", who: "Pentasio agent" },
              { when: "7 weeks ago", what: "Included in the legalplus.ma standard report", who: "Automatic" },
              { when: "Today", what: "Still open, re-confirmed on the running scan", who: "Pentasio agent" },
            ].map((h, i) => (
              <div key={i} className="flex gap-3">
                <Dot className="mt-2 bg-ink-2" />
                <div className="min-w-0">
                  <div className="text-body text-ink">{h.what}</div>
                  <div className="mt-1 text-fine text-ink-2">
                    {h.when} · {h.who}
                  </div>
                </div>
              </div>
            ))}
          </div>
        </Panel>
      )}
    </div>
  );
}

/* ------------------------------------------------------------------ *
 *  reports
 * ------------------------------------------------------------------ */
function reportMarkdown(r: Report): string {
  return [
    `# Pentasio report — ${r.domain}`,
    "",
    `Scan: ${r.scan}`,
    `Type: ${r.type}`,
    `Generated: ${r.when}`,
    `Pages: ${r.pages}`,
    "",
    "## Findings by severity",
    ...r.counts.map(([sev, n]) => `- ${SEV_LABEL[sev]}: ${n}`),
    "",
    "Generated by Pentasio. This is a beta dashboard demo report.",
  ].join("\n");
}

function Reports({ c }: { c: Ctx }) {
  const [reports, setReports] = useState<Report[]>(REPORTS);
  const [preview, setPreview] = useState<Report | null>(null);

  const generate = () => {
    const report: Report = {
      domain: DOMAINS[0]?.name ?? "workspace",
      type: "Standard",
      scan: Math.random().toString(16).slice(2, 8),
      when: "just now",
      pages: 12,
      counts: SEV_ORDER.map((s) => [s, COUNTS[s]] as [Sev, number]),
    };
    setReports((prev) => [report, ...prev]);
    toast.success("Report generated");
  };

  const download = (r: Report) => {
    const blob = new Blob([reportMarkdown(r)], {
      type: "text/markdown;charset=utf-8",
    });
    const url = URL.createObjectURL(blob);
    const a = document.createElement("a");
    a.href = url;
    a.download = `pentasio-${r.domain}-${r.scan}.md`;
    document.body.appendChild(a);
    a.click();
    a.remove();
    window.setTimeout(() => URL.revokeObjectURL(url), 0);
    toast.success("Report downloaded");
  };

  return (
    <div>
      <Eyebrow>Risk</Eyebrow>
      <PageHeader
        className="mb-6 mt-1"
        title="Reports"
        sub="Client-ready reports generated from completed scans"
        actions={
          <Button
            variant="primary"
            size="sm"
            icon={<FileText size={15} />}
            onClick={generate}
          >
            Generate report
          </Button>
        }
      />
      <div className="grid gap-6" style={{ gridTemplateColumns: "repeat(auto-fill, minmax(340px, 1fr))" }}>
        {reports.map((r) => (
          <Panel key={r.scan} className="p-6">
            <div className="flex items-center justify-between gap-2">
              <Chip tone="neutral">{r.type}</Chip>
              <Mono>scan {r.scan}</Mono>
            </div>
            <div className="mt-3 text-title text-ink">{r.domain}</div>
            <div className="mt-1 text-fine text-ink-2">
              Generated {r.when} · {r.pages} pages
            </div>
            <div className="mt-4 flex flex-wrap gap-2">
              {r.counts.map(([sev, n]) => (
                <Chip key={sev} tone={SEV_TONE[sev]}>
                  <span className="inline-flex items-center gap-1">
                    <span className="u-num">{n}</span> {SEV_LABEL[sev].toLowerCase()}
                  </span>
                </Chip>
              ))}
            </div>
            <div className="mt-6 flex flex-wrap gap-2">
              <Button
                variant="primary"
                size="sm"
                icon={<Download size={15} />}
                onClick={() => download(r)}
              >
                Download report
              </Button>
              <Button variant="secondary" size="sm" onClick={() => setPreview(r)}>
                Preview
              </Button>
            </div>
          </Panel>
        ))}
      </div>

      <Dialog
        open={preview !== null}
        onOpenChange={(open) => {
          if (!open) setPreview(null);
        }}
        title={preview ? `${preview.domain} report` : "Report preview"}
        description="Generated from a completed scan."
        closeLabel="Close report preview"
        width="md"
        footer={
          <>
            <Button variant="secondary" size="sm" onClick={() => setPreview(null)}>
              Close
            </Button>
            <Button
              variant="primary"
              size="sm"
              icon={<Download size={15} />}
              onClick={() => preview && download(preview)}
            >
              Download
            </Button>
          </>
        }
      >
        {preview ? (
          <div className="flex flex-col gap-3">
            <div className="flex flex-wrap items-center gap-2">
              <Chip tone="neutral">{preview.type}</Chip>
              <Mono>scan {preview.scan}</Mono>
              <span className="text-fine text-ink-2">{preview.pages} pages</span>
            </div>
            <div className="flex flex-wrap gap-2">
              {preview.counts.map(([sev, n]) => (
                <Chip key={sev} tone={SEV_TONE[sev]}>
                  <span className="u-num">{n}</span> {SEV_LABEL[sev].toLowerCase()}
                </Chip>
              ))}
            </div>
            <p className="text-small text-ink-2">
              This beta preview is generated from the dashboard demo data; the download is a Markdown report.
            </p>
          </div>
        ) : null}
      </Dialog>
    </div>
  );
}

/* ------------------------------------------------------------------ *
 *  AI agent
 * ------------------------------------------------------------------ */
const AGENT_PROMPTS = [
  { title: "Explain the top critical finding", sub: "root cause and real impact" },
  { title: "Draft a fix PR description", sub: "markdown, ready for GitHub" },
  { title: "Find the same pattern elsewhere", sub: "across all my targets" },
  { title: "Write a customer disclosure note", sub: "if data was accessed" },
];

function Bubble({ m }: { m: Msg }) {
  const mine = m.role === "user";
  return (
    <div className={cn("flex", mine ? "justify-end" : "justify-start")}>
      <div
        className={cn(
          "max-w-[80%] rounded-panel px-4 py-3 text-small",
          mine ? "bg-ink text-ground" : "bg-surface-2 text-ink",
        )}
      >
        {m.text}
      </div>
    </div>
  );
}

function Composer({ c }: { c: Ctx }) {
  return (
    <Panel strong className="p-4 u-lift">
      <Textarea
        bare
        value={c.input}
        onChange={(e) => c.setInput(e.target.value)}
        onKeyDown={(e) => {
          if (e.key === "Enter" && !e.shiftKey) {
            e.preventDefault();
            c.send();
          }
        }}
        placeholder="Ask about a finding, request a fix, summarize a scan…"
        className="w-full"
        rows={2}
      />
      <div className="mt-2 flex items-center justify-between">
        <div className="flex items-center gap-3 text-fine text-ink-2">
          <span>
            <span className="font-mono">⏎</span> send
          </span>
          <span>
            <span className="font-mono">⇧⏎</span> new line
          </span>
        </div>
        <Button variant="primary" size="sm" icon={<Send size={15} />} onClick={c.send}>
          Send
        </Button>
      </div>
    </Panel>
  );
}

function Agent({ c }: { c: Ctx }) {
  return (
    <div className="mx-auto" style={{ maxWidth: "55rem" }}>
      <div className="flex flex-col items-center text-center">
        <span className="grid h-12 w-12 place-items-center rounded-panel bg-surface-2 text-ink">
          <Bot size={22} />
        </span>
        <h1 className="u-title mt-4 text-display text-ink">What do you want to know?</h1>
        <p className="mt-2 text-body text-ink-2" style={{ maxWidth: "34rem" }}>
          The agent reads every scan, finding and report in this workspace. It cannot launch or
          stop a scan without your confirmation.
        </p>
      </div>

      {c.msgs.length > 0 ? (
        <div className="mt-8 flex flex-col gap-3">
          {c.msgs.map((m, i) => (
            <Bubble key={i} m={m} />
          ))}
        </div>
      ) : (
        <div
          className="mt-8 grid gap-3"
          style={{ gridTemplateColumns: "repeat(auto-fit, minmax(240px, 1fr))" }}
        >
          {AGENT_PROMPTS.map((p) => (
            <ClickCard key={p.title} onClick={() => c.ask(p.title)}>
              <div className="text-body font-medium text-ink">{p.title}</div>
              <div className="mt-1 text-small text-ink-2">{p.sub}</div>
            </ClickCard>
          ))}
        </div>
      )}

      <div className="mt-6">
        <Composer c={c} />
      </div>
    </div>
  );
}

/* ------------------------------------------------------------------ *
 *  settings
 * ------------------------------------------------------------------ */
const SETTINGS_PERSONAL = ["Profile", "Account", "Notifications", "Sessions"];
const SETTINGS_WORKSPACE = ["Plan & billing", "Branding", "API keys", "Webhooks"];

function SettingsNavGroup({
  label,
  items,
  c,
}: {
  label: string;
  items: string[];
  c: Ctx;
}) {
  return (
    <div>
      <div className="px-3 pb-1">
        <Eyebrow>{label}</Eyebrow>
      </div>
      <div className="flex flex-col gap-1">
        {items.map((it) => (
          <Row
            key={it}
            density="nav"
            flush
            active={c.settingsTab === it}
            onClick={() => c.setSettingsTab(it)}
          >
            {it}
          </Row>
        ))}
      </div>
    </div>
  );
}

const SETTINGS_COPY: Record<string, { hint: string; note: string }> = {
  Profile: {
    hint: "How you appear across the workspace.",
    note: "Changes apply the next time you sign in.",
  },
  Account: {
    hint: "Your sign-in identity and workspace access.",
    note: "Changes apply the next time you sign in.",
  },
  Notifications: {
    hint: "Choose what Pentasio sends you.",
    note: "Critical security alerts are always delivered in-app.",
  },
  Sessions: {
    hint: "Devices currently signed in to this account.",
    note: "Signing out other sessions keeps this device signed in.",
  },
  "Plan & billing": {
    hint: "Your current plan and seats.",
    note: "Beta terms apply until general availability.",
  },
  Branding: {
    hint: "The mark that appears on reports and client emails.",
    note: "Square PNG, JPG or WEBP up to 2 MB.",
  },
  "API keys": {
    hint: "Keys that let agents read findings in this workspace.",
    note: "Treat keys as passwords; rotate them if they leak.",
  },
  Webhooks: {
    hint: "Send new findings to an endpoint you control.",
    note: "Failed deliveries are retried for 24 hours during beta.",
  },
};

function SettingsScreen({ c }: { c: Ctx }) {
  const [name, setName] = useState("Test Client");
  const [email, setEmail] = useState("test@pentasio.com");
  const [logoName, setLogoName] = useState<string | null>(null);
  const [notify, setNotify] = useState({ digest: true, critical: true, product: false });
  const [brand, setBrand] = useState("Pentasio");
  const [webhook, setWebhook] = useState("");
  const fileRef = useRef<HTMLInputElement>(null);

  const tab = c.settingsTab;
  const isProfile = tab === "Profile" || tab === "Account";
  const meta = SETTINGS_COPY[tab] ?? SETTINGS_COPY.Profile;

  const upload = () => fileRef.current?.click();
  const save = (what: string) => toast.success(`${what} saved`);
  const discard = () => {
    setName("Test Client");
    setEmail("test@pentasio.com");
    setLogoName(null);
    toast.info("Changes discarded");
  };

  const action = isProfile ? (
    <>
      <Button variant="ghost" size="sm" onClick={discard}>
        Discard
      </Button>
      <Button variant="primary" size="sm" onClick={() => save(tab)}>
        Save changes
      </Button>
    </>
  ) : tab === "Notifications" || tab === "Branding" ? (
    <Button variant="primary" size="sm" onClick={() => save(tab)}>
      Save changes
    </Button>
  ) : undefined;

  return (
    <div>
      <Eyebrow>Account</Eyebrow>
      <PageHeader className="mb-6 mt-1" title="Settings" />
      <div className="grid gap-6" style={{ gridTemplateColumns: "minmax(0, 15rem) minmax(0, 1fr)" }}>
        <nav className="flex flex-col gap-6">
          <SettingsNavGroup label="Personal" items={SETTINGS_PERSONAL} c={c} />
          <SettingsNavGroup label="Workspace" items={SETTINGS_WORKSPACE} c={c} />
        </nav>

        <div style={{ maxWidth: "45rem" }}>
          <SettingsCard title={tab} hint={meta.hint} note={meta.note} action={action}>
            {isProfile ? (
              <>
                <div className="flex items-center gap-4">
                  <span className="grid h-16 w-16 shrink-0 place-items-center rounded-panel bg-surface-3 text-title font-medium text-ink">
                    {logoName ? "IMG" : "TC"}
                  </span>
                  <div className="min-w-0">
                    <div className="text-body font-medium text-ink">Profile picture</div>
                    <div className="mt-1 text-fine text-ink-2">
                      {logoName ?? "PNG, JPG or WEBP up to 2 MB. Square renders best."}
                    </div>
                    <Button
                      variant="secondary"
                      size="sm"
                      className="mt-2"
                      icon={<Upload size={15} />}
                      onClick={upload}
                    >
                      Upload
                    </Button>
                  </div>
                </div>
                <div className="mt-6 flex flex-col gap-4">
                  <label className="block">
                    <span className="mb-1 block text-small text-ink">Full name</span>
                    <Input
                      value={name}
                      onChange={(e) => setName(e.target.value)}
                      className="w-full"
                    />
                  </label>
                  <label className="block">
                    <span className="mb-1 block text-small text-ink">Email</span>
                    <Input
                      value={email}
                      onChange={(e) => setEmail(e.target.value)}
                      className="w-full"
                    />
                    <span className="mt-1 block text-fine text-ink-2">
                      Used for sign-in and security notices. Email us to change it during beta.
                    </span>
                  </label>
                </div>
              </>
            ) : null}

            {tab === "Notifications" ? (
              <div className="flex flex-col gap-4">
                <Checkbox
                  label="Weekly digest"
                  hint="A Monday summary of posture, new findings and fixes."
                  checked={notify.digest}
                  onChange={(v) => setNotify({ ...notify, digest: v })}
                />
                <Checkbox
                  label="Critical finding alerts"
                  hint="Immediate email when a critical is confirmed."
                  checked={notify.critical}
                  onChange={(v) => setNotify({ ...notify, critical: v })}
                />
                <Checkbox
                  label="Product updates"
                  hint="Occasional notes about new Pentasio capabilities."
                  checked={notify.product}
                  onChange={(v) => setNotify({ ...notify, product: v })}
                />
              </div>
            ) : null}

            {tab === "Sessions" ? (
              <div className="flex flex-col gap-4">
                <div className="flex flex-col gap-2">
                  <div className="flex items-center justify-between gap-3 rounded-cell bg-surface-2 px-3 py-2">
                    <span className="text-body text-ink">Chrome · macOS</span>
                    <Status tone="success">This device</Status>
                  </div>
                  <div className="flex items-center justify-between gap-3 rounded-cell bg-surface-2 px-3 py-2">
                    <span className="text-body text-ink">Safari · iOS</span>
                    <span className="text-fine text-ink-2">2 days ago</span>
                  </div>
                </div>
                <Button
                  variant="danger"
                  size="sm"
                  className="self-start"
                  onClick={() => toast.success("Other sessions signed out")}
                >
                  Sign out other sessions
                </Button>
              </div>
            ) : null}

            {tab === "Plan & billing" ? (
              <div className="flex flex-col gap-4">
                <div className="flex items-center justify-between gap-3 rounded-cell bg-surface-2 px-4 py-3">
                  <div>
                    <div className="text-body font-medium text-ink">Beta</div>
                    <div className="text-small text-ink-2">5 seats · unlimited scans during beta</div>
                  </div>
                  <Chip tone="success">Active</Chip>
                </div>
                <Button
                  variant="secondary"
                  size="sm"
                  className="self-start"
                  onClick={() => toast.info("Billing portal is not wired in this beta.")}
                >
                  Manage billing
                </Button>
              </div>
            ) : null}

            {tab === "Branding" ? (
              <div className="flex flex-col gap-4">
                <div className="flex items-center gap-4">
                  <span className="grid h-16 w-16 shrink-0 place-items-center rounded-panel bg-surface-3 text-title font-medium text-ink">
                    {logoName ? "IMG" : "P"}
                  </span>
                  <div className="min-w-0">
                    <div className="text-body font-medium text-ink">Workspace mark</div>
                    <div className="mt-1 text-fine text-ink-2">
                      {logoName ?? "Square PNG, JPG or WEBP up to 2 MB."}
                    </div>
                    <Button
                      variant="secondary"
                      size="sm"
                      className="mt-2"
                      icon={<Upload size={15} />}
                      onClick={upload}
                    >
                      Upload
                    </Button>
                  </div>
                </div>
                <label className="block">
                  <span className="mb-1 block text-small text-ink">Brand name</span>
                  <Input
                    value={brand}
                    onChange={(e) => setBrand(e.target.value)}
                    className="w-full"
                  />
                </label>
              </div>
            ) : null}

            {tab === "API keys" ? (
              <div className="flex flex-col gap-4">
                <div className="u-scroll overflow-x-auto rounded-cell bg-well px-4 py-3 font-mono text-small text-ink-2">
                  ps_live_••••••••8f2a
                </div>
                <Button
                  variant="secondary"
                  size="sm"
                  className="self-start"
                  onClick={() => toast.success("API key created")}
                >
                  Create key
                </Button>
              </div>
            ) : null}

            {tab === "Webhooks" ? (
              <div className="flex flex-col gap-4">
                <label className="block">
                  <span className="mb-1 block text-small text-ink">Endpoint URL</span>
                  <Input
                    value={webhook}
                    onChange={(e) => setWebhook(e.target.value)}
                    placeholder="https://hooks.example.com/pentasio"
                    className="w-full"
                  />
                </label>
                <Button
                  variant="secondary"
                  size="sm"
                  className="self-start"
                  onClick={() => toast.success("Webhook endpoint added")}
                >
                  Add endpoint
                </Button>
              </div>
            ) : null}

            <input
              ref={fileRef}
              type="file"
              accept="image/png,image/jpeg,image/webp"
              className="hidden"
              onChange={(e) => {
                const file = e.target.files?.[0];
                if (!file) return;
                setLogoName(file.name);
                toast.success("Logo selected");
              }}
            />
          </SettingsCard>
        </div>
      </div>
    </div>
  );
}

/* ------------------------------------------------------------------ *
 *  overlays: Ask AI slide-over + New scan modal
 * ------------------------------------------------------------------ */
function AskPanel({
  c,
  context,
  onClose,
}: {
  c: Ctx;
  context: string;
  onClose: () => void;
}) {
  return (
    <div className="fixed inset-0 z-50 flex justify-end">
      <div className="u-modal-scrim absolute inset-0" onClick={onClose} aria-hidden />
      <div
        className="relative flex h-full w-full flex-col glass-strong u-lift-strong u-arrive"
        style={{ maxWidth: "28.75rem" }}
      >
        <div className="flex items-center gap-3 border-x-0 border-t-0 border-b border-line px-6 py-4">
          <span className="grid h-9 w-9 shrink-0 place-items-center rounded-control bg-surface-2 text-ink">
            <Bot size={18} />
          </span>
          <div className="min-w-0 flex-1">
            <div className="text-body font-medium text-ink">AI Agent</div>
            <div className="truncate text-fine text-ink-2">Context: {context}</div>
          </div>
          <IconButton label="Close" size="sm" variant="ghost" onClick={onClose}>
            <ChevronRight size={16} />
          </IconButton>
        </div>

        <div className="u-scroll flex flex-1 flex-col gap-3 overflow-y-auto px-6 py-4">
          <div className="rounded-panel bg-surface-2 px-4 py-3 text-small text-ink">{PANEL_SEED}</div>
          {c.msgs.map((m, i) => (
            <Bubble key={i} m={m} />
          ))}
        </div>

        <div className="border-x-0 border-b-0 border-t border-line px-6 py-4">
          <div className="mb-3 flex flex-wrap gap-2">
            {["Outline the patch", "What is the blast radius?", "Draft the Jira ticket"].map((q) => (
              <Chip key={q} tone="neutral" onClick={() => c.ask(q)}>
                {q}
              </Chip>
            ))}
          </div>
          <div className="flex items-center gap-2">
            <Input
              value={c.input}
              onChange={(e) => c.setInput(e.target.value)}
              onKeyDown={(e) => {
                if (e.key === "Enter") c.send();
              }}
              placeholder="Reply to the agent…"
              aria-label="Message the agent"
              className="w-full"
            />
            <IconButton label="Send" variant="primary" onClick={c.send}>
              <Send size={16} />
            </IconButton>
          </div>
        </div>
      </div>
    </div>
  );
}

const STEP_TITLES = ["Pick a target", "Confirm authorization", "Choose intensity"];
const STEP_SUBS = [
  "Only verified, authorized domains can be scanned.",
  "This is recorded with the scan and included in the report.",
  "Beta access includes one concurrent deep scan.",
];
const INTENSITY_OPTS = [
  { value: "Quick", time: "~10 min", sub: "Surface checks: headers, TLS, exposed files." },
  { value: "Standard", time: "~40 min", sub: "Full crawl with authenticated checks and injection testing." },
  { value: "Deep", time: "~90 min", sub: "Everything in standard plus agent-driven exploit validation." },
];

function NewScanModal({
  step,
  setStep,
  consent,
  setConsent,
  newTarget,
  setNewTarget,
  intensity,
  setIntensity,
  onClose,
  onLaunch,
}: {
  step: number;
  setStep: (n: number) => void;
  consent: boolean;
  setConsent: (b: boolean) => void;
  newTarget: string;
  setNewTarget: (s: string) => void;
  intensity: string;
  setIntensity: (s: string) => void;
  onClose: () => void;
  onLaunch: () => void;
}) {
  const targets = DOMAINS.filter((d) => d.status === "Authorized").slice(0, 4);
  const canNext = step !== 1 || consent;
  const nextLabel = step === 2 ? "Launch scan" : "Continue";
  const next = () => {
    if (step < 2) setStep(step + 1);
    else onLaunch();
  };
  return (
    <Dialog
      open
      onOpenChange={(o) => {
        if (!o) onClose();
      }}
      title={STEP_TITLES[step]}
      description={STEP_SUBS[step]}
      closeLabel="Close"
      width="md"
      footer={
        <>
          {step > 0 && (
            <Button variant="secondary" size="sm" className="mr-auto" onClick={() => setStep(step - 1)}>
              <span className="inline-flex items-center gap-1">
                <ChevronLeft size={14} /> Back
              </span>
            </Button>
          )}
          {step === 2 && (
            <span className="self-center text-small text-ink-2">
              {newTarget} · {intensity}
            </span>
          )}
          <Button variant="primary" size="sm" disabled={!canNext} onClick={next}>
            {nextLabel}
          </Button>
        </>
      }
    >
      <div className="mb-4 flex items-center justify-between gap-3">
        <Eyebrow>New scan · step {step + 1} of 3</Eyebrow>
        <div className="flex gap-1">
          {[0, 1, 2].map((i) => (
            <span
              key={i}
              className={cn("h-1.5 w-8 rounded-full", i <= step ? "bg-accent" : "bg-well")}
            />
          ))}
        </div>
      </div>

      {step === 0 && (
        <div className="flex flex-col gap-2">
          {targets.map((d) => (
            <ChoiceCard
              key={d.name}
              label={d.name}
              checked={newTarget === d.name}
              onChange={() => setNewTarget(d.name)}
            >
              <div className="flex w-full items-center gap-3">
                <div className="min-w-0 flex-1">
                  <div className="text-body font-medium text-ink">{d.name}</div>
                  <div className="mt-1 truncate text-fine text-ink-2">
                    {d.auth} · last scan {d.mode.toLowerCase()}
                  </div>
                </div>
                <Chip tone="success">Authorized</Chip>
              </div>
            </ChoiceCard>
          ))}
        </div>
      )}

      {step === 1 && (
        <div className="flex flex-col gap-4">
          <p className="text-small text-ink-2">
            You confirm you are authorized to test{" "}
            <span className="font-medium text-ink">{newTarget}</span>. Pentasio runs
            non-destructive checks only, rate-limited to 20 requests per second, within your
            current authorization window.
          </p>
          <div className="rounded-panel border border-line bg-surface p-4">
            <Checkbox
              checked={consent}
              onChange={setConsent}
              label="I am the owner or an authorized agent for this domain"
            />
          </div>
        </div>
      )}

      {step === 2 && (
        <div className="flex flex-col gap-2">
          {INTENSITY_OPTS.map((o) => (
            <ChoiceCard
              key={o.value}
              label={o.value}
              checked={intensity === o.value}
              onChange={() => setIntensity(o.value)}
            >
              <div className="flex w-full items-center gap-3">
                <div className="min-w-0 flex-1">
                  <div className="text-body font-medium text-ink">{o.value}</div>
                  <div className="mt-1 text-fine text-ink-2">{o.sub}</div>
                </div>
                <span className="shrink-0 text-small text-ink-2">{o.time}</span>
              </div>
            </ChoiceCard>
          ))}
        </div>
      )}
    </Dialog>
  );
}

/* ------------------------------------------------------------------ *
 *  small mapping helpers (band -> tone class)
 * ------------------------------------------------------------------ */
function bandSev(band: string): Sev {
  if (band === "Critical") return "crit";
  if (band === "At risk") return "high";
  if (band === "Watch") return "med";
  return "info";
}
function bandText(band: string): string {
  if (band === "Critical") return "text-danger";
  if (band === "At risk") return "text-contest";
  if (band === "Watch") return "text-warn";
  if (band === "Healthy") return "text-ok";
  return "text-ink-2";
}
function logText(k: string): string {
  if (k === "bad") return "text-danger";
  if (k === "warn") return "text-warn";
  if (k === "ok") return "text-ok";
  return "text-ink-2";
}

/* ------------------------------------------------------------------ *
 *  root
 * ------------------------------------------------------------------ */
export function Pentasio() {
  const [screen, setScreen] = useState<Screen>("dashboard");
  const [variant, setVariant] = useState<"score" | "feed">("score");
  const [panel, setPanel] = useState(false);
  const [modal, setModal] = useState(false);
  const [step, setStep] = useState(0);
  const [consent, setConsent] = useState(false);
  const [sel, setSel] = useState(0);
  const [tab, setTab] = useState<"fix" | "evidence" | "history">("fix");
  const [fStatus, setFStatus] = useState<"Open" | "Fixed" | "Accepted">("Open");
  const [sevFilter, setSevFilter] = useState<SevFilter>("all");
  const [domFilter, setDomFilter] = useState("All");
  const [scanPct, setScanPct] = useState(64);
  const [scanStopped, setScanStopped] = useState(false);
  const [msgs, setMsgs] = useState<Msg[]>([]);
  const [input, setInput] = useState("");
  const [settingsTab, setSettingsTab] = useState("Profile");
  const [newTarget, setNewTarget] = useState("rahadoc.com");
  const [intensity, setIntensity] = useState("Standard");
  const [theme, setThemeState] = useState<Resolved>(() => resolvedTheme());

  // live scan progress: while on the scan detail screen, tick toward completion.
  useEffect(() => {
    if (screen !== "scandetail" || scanStopped) return;
    const id = window.setInterval(() => {
      setScanPct((p) => (p < 91 ? p + 1 : p));
    }, 1100);
    return () => window.clearInterval(id);
  }, [screen, scanStopped]);

  const toggleTheme = () => {
    const next: Resolved = resolvedTheme() === "dark" ? "light" : "dark";
    setTheme(next);
    setThemeState(next);
  };

  const say = (text: string) => {
    const t = text.trim();
    if (!t) return;
    setMsgs((m) => [...m, { role: "user", text: t }, { role: "ai", text: AI_REPLY }]);
    setInput("");
  };

  const c: Ctx = {
    go: (s) => {
      if (s === "findings") setSevFilter("all");
      setScreen(s);
    },
    openFinding: (i) => {
      setSel(i);
      setTab("fix");
      setFStatus("Open");
      setScreen("finding");
    },
    openPanel: () => setPanel(true),
    openModal: () => {
      setStep(0);
      setConsent(false);
      setModal(true);
    },
    variant,
    setVariant,
    sel,
    tab,
    setTab,
    fStatus,
    setFStatus,
    markFixed: () => setFStatus("Fixed"),
    sevFilter,
    setSevFilter,
    openSev: (s) => {
      setSevFilter(s);
      setScreen("findings");
    },
    domFilter,
    setDomFilter,
    scanPct,
    scanStopped,
    stopScan: () => {
      setScanStopped(true);
      toast.info("Scan stopped. Findings already captured are kept.");
    },
    msgs,
    input,
    setInput,
    send: () => say(input),
    ask: (q) => say(q),
    settingsTab,
    setSettingsTab,
  };

  const panelContext =
    screen === "finding"
      ? FINDINGS[Math.min(sel, FINDINGS.length - 1)].code
      : screen === "scandetail"
        ? "scan 19e05d"
        : "this workspace";

  return (
    <div className="u-arrive flex min-h-screen bg-ground text-ink">
      <Sidebar c={c} screen={screen} />
      <div className="flex min-w-0 flex-1 flex-col">
        <Header c={c} theme={theme} onToggleTheme={toggleTheme} />
        <main className="min-w-0 flex-1 px-8 py-8">
          <div key={screen} className="u-rise">
            {screen === "dashboard" && <Dashboard c={c} />}
            {screen === "domains" && <Domains c={c} />}
            {screen === "scans" && <Scans c={c} />}
            {screen === "scandetail" && <ScanDetail c={c} />}
            {screen === "findings" && <Findings c={c} />}
            {screen === "finding" && <FindingDetail c={c} />}
            {screen === "reports" && <Reports c={c} />}
            {screen === "graph" && <PentasioGraph />}
            {screen === "agent" && <Agent c={c} />}
            {screen === "settings" && <SettingsScreen c={c} />}
          </div>
        </main>
      </div>

      {panel && <AskPanel c={c} context={panelContext} onClose={() => setPanel(false)} />}
      {modal && (
        <NewScanModal
          step={step}
          setStep={setStep}
          consent={consent}
          setConsent={setConsent}
          newTarget={newTarget}
          setNewTarget={setNewTarget}
          intensity={intensity}
          setIntensity={setIntensity}
          onClose={() => setModal(false)}
          onLaunch={() => {
            setModal(false);
            setScanStopped(false);
            setScanPct(4);
            setScreen("scandetail");
          }}
        />
      )}
    </div>
  );
}

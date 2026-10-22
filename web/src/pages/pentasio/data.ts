/* Pentasio security-dashboard data, ported verbatim from the original mockup's
   DOMAINS / SCANS / FINDINGS / REPORTS / LOGS / PHASES arrays. This file is only
   data + the maps that translate Pentasio's own vocabulary (severity keys, posture
   bands, scan states, log kinds) onto Utopia's semantic tones. No colours, no copy
   with em-dashes: everything user-facing reads as plain punctuation. */
import type { ChipTone } from "../../ui";

/* ---------- severity ---------- */
export type Sev = "crit" | "high" | "med" | "low" | "info";

export const SEV_LABEL: Record<Sev, string> = {
  crit: "Critical",
  high: "High",
  med: "Medium",
  low: "Low",
  info: "Info",
};

/* Pentasio's five severities land on Utopia's semantic tones in a descending
   hierarchy: danger is loudest, neutral is the long informational tail. */
export const SEV_TONE: Record<Sev, ChipTone> = {
  crit: "danger",
  high: "contest",
  med: "warn",
  low: "violet",
  info: "neutral",
};

export const SEV_ORDER: Sev[] = ["crit", "high", "med", "low", "info"];

/* The filled bar segments read as a background class rather than a tone, so the
   neutral tail sits on the muted ink swatch instead of a transparent chip tint. */
export const SEV_BAR_BG: Record<Sev, string> = {
  crit: "bg-danger",
  high: "bg-contest",
  med: "bg-warn",
  low: "bg-violet",
  info: "bg-ink-2",
};

/* ---------- posture bands ---------- */
export type Band = "Critical" | "At risk" | "Watch" | "Healthy" | "Not scanned";

export const BAND_TONE: Record<Band, ChipTone> = {
  Critical: "danger",
  "At risk": "contest",
  Watch: "warn",
  Healthy: "success",
  "Not scanned": "neutral",
};

/* ---------- domains / targets ---------- */
export type DomainStatus = "Authorized" | "Needs signature";
export type Intensity = "Quick" | "Standard" | "Deep";

export type Domain = {
  name: string;
  status: DomainStatus;
  score: number | null;
  band: Band;
  open: number;
  sevLine: string;
  auth: string;
  mode: Intensity;
};

export const DOMAINS: Domain[] = [
  { name: "legalplus.ma", status: "Authorized", score: 24, band: "Critical", open: 61, sevLine: "2 critical · 7 high", auth: "authorized through 9/20/2026", mode: "Deep" },
  { name: "rahadoc.com", status: "Authorized", score: 48, band: "At risk", open: 38, sevLine: "12 high · 6 medium", auth: "authorized through 10/2/2026", mode: "Standard" },
  { name: "1337.ma", status: "Authorized", score: 55, band: "At risk", open: 97, sevLine: "9 high · 4 medium", auth: "authorized through 9/28/2026", mode: "Standard" },
  { name: "geocara.com", status: "Authorized", score: 71, band: "Watch", open: 38, sevLine: "3 high · 1 medium", auth: "authorized through 10/6/2026", mode: "Deep" },
  { name: "companioncare.fr", status: "Authorized", score: 80, band: "Healthy", open: 16, sevLine: "informational only", auth: "authorized through 10/1/2026", mode: "Quick" },
  { name: "vaultpay.io", status: "Authorized", score: 88, band: "Healthy", open: 7, sevLine: "2 medium · 2 low", auth: "authorized through 9/20/2026", mode: "Standard" },
  { name: "inkway.ma", status: "Authorized", score: 92, band: "Healthy", open: 0, sevLine: "no open findings", auth: "authorized through 10/4/2026", mode: "Deep" },
  { name: "hello.com", status: "Needs signature", score: null, band: "Not scanned", open: 0, sevLine: "awaiting authorization", auth: "added 7/22/2026", mode: "Standard" },
];

/* ---------- scans ---------- */
export type ScanStatus = "Running" | "Queued" | "Completed" | "Failed";

export const SCAN_TONE: Record<ScanStatus, ChipTone> = {
  Running: "success",
  Queued: "info",
  Completed: "neutral",
  Failed: "danger",
};

export type Scan = {
  target: string;
  id: string;
  type: Intensity;
  status: ScanStatus;
  phase: string;
  pct: number;
  findings: number;
  when: string;
  cta: string;
};

export const SCANS: Scan[] = [
  { target: "legalplus.ma", id: "19e05d", type: "Deep", status: "Running", phase: "Exploit validation", pct: 64, findings: 12, when: "14 min ago", cta: "Watch" },
  { target: "rahadoc.com", id: "fdc238", type: "Standard", status: "Queued", phase: "Waiting for a slot", pct: 0, findings: 0, when: "2 min ago", cta: "View" },
  { target: "1337.ma", id: "f8933b", type: "Standard", status: "Completed", phase: "Finished in 41 min", pct: 100, findings: 97, when: "2 days ago", cta: "Report" },
  { target: "geocara.com", id: "2d3c0e", type: "Deep", status: "Completed", phase: "Finished in 1 h 12 m", pct: 100, findings: 38, when: "4 days ago", cta: "Report" },
  { target: "vaultpay.io", id: "d71ab5", type: "Standard", status: "Completed", phase: "Finished in 28 min", pct: 100, findings: 7, when: "6 days ago", cta: "Report" },
  { target: "companioncare.fr", id: "6fa349", type: "Quick", status: "Completed", phase: "Finished in 9 min", pct: 100, findings: 16, when: "8 days ago", cta: "Report" },
  { target: "aicrafters.com", id: "73535d", type: "Standard", status: "Failed", phase: "DNS resolution failed", pct: 12, findings: 0, when: "9 days ago", cta: "Retry" },
];

/* ---------- findings ---------- */
export type FixStep = { title: string; body: string };

export type Finding = {
  code: string;
  sev: Sev;
  title: string;
  domain: string;
  scan: string;
  cvss: string;
  age: string;
  effort: string;
  impact: string;
  impactLong: string;
  endpoint: string;
  snippet: string;
  evidence: string;
  fix: FixStep[];
};

export const FINDINGS: Finding[] = [
  {
    code: "770AB6",
    sev: "crit",
    title: "Mass IDOR: any user can read every user's identity documents",
    domain: "legalplus.ma",
    scan: "19e05d",
    cvss: "9.1",
    age: "8 weeks",
    effort: "about half a day",
    impact: "Any logged-in account can enumerate and download other customers' KYC files.",
    impactLong:
      "Every uploaded identity document is reachable by any authenticated account. That makes this a reportable personal-data breach, and the endpoint has been exposed for at least 8 weeks.",
    endpoint: "GET https://office.legalplus.co/api/documents/{id}",
    snippet:
      "// vulnerable: id comes straight from the URL\nconst doc = await Documents.findById(id)\n\n// fixed: scope every lookup to the caller\nconst doc = await Documents.findOne({ _id: id, ownerId: req.user.id })\nif (!doc) return res.sendStatus(404)",
    evidence:
      "GET /api/documents/10422 returns a PDF that belongs to account 118 while the requester is authenticated as account 904. The document service runs no ownership check before streaming the file.",
    fix: [
      { title: "Scope every lookup to the caller", body: "Filter each query by the authenticated owner id, not just the object id from the request." },
      { title: "Return 404, never 403, for foreign objects", body: "A 403 confirms the record exists. Answer with 404 so the id space cannot be walked." },
      { title: "Add a regression test per role", body: "Assert that account A cannot read account B's document, and run it on every deploy." },
    ],
  },
  {
    code: "580862",
    sev: "crit",
    title: "Sensitive document publicly exposed without authentication",
    domain: "legalplus.ma",
    scan: "19e05d",
    cvss: "9.3",
    age: "8 weeks",
    effort: "about 2 hours",
    impact: "A signed client contract is served from the public uploads directory with listing on.",
    impactLong:
      "The uploads directory is world readable and directory listing is enabled, so signed contracts and other private files can be found and downloaded by anyone who guesses the path or reads the index.",
    endpoint: "GET https://legalplus.ma/wp-content/uploads/2026/01/output-28.pdf",
    snippet:
      "# nginx: keep private files off the public web root\nlocation ^~ /wp-content/uploads/ {\n  internal;\n  autoindex off;\n}",
    evidence:
      "Requesting /wp-content/uploads/2026/01/ returns a full directory index. output-28.pdf is a countersigned engagement letter served with a 200 and no auth.",
    fix: [
      { title: "Move private files out of the web root", body: "Serve them through an authenticated handler instead of a static path." },
      { title: "Disable directory listing", body: "Turn autoindex off so the folder cannot be browsed even if a path leaks." },
      { title: "Rotate the exposed document", body: "Treat the leaked contract as compromised and reissue it." },
    ],
  },
  {
    code: "A11C34",
    sev: "high",
    title: "Unrestricted file upload: PHP webshells accepted and stored",
    domain: "legalplus.ma",
    scan: "19e05d",
    cvss: "8.4",
    age: "8 weeks",
    effort: "about 1 day",
    impact: "The attachment endpoint trusts the client-supplied MIME type, which opens a path to remote code execution.",
    impactLong:
      "Because the server accepts the extension and content type sent by the client, a .php payload can be uploaded to a directory the web server will execute, turning a file field into remote code execution.",
    endpoint: "POST https://office.legalplus.co/api/attachments",
    snippet:
      "// reject anything not on a server-side allow-list\nconst ok = ['pdf', 'png', 'jpg', 'docx']\nif (!ok.includes(realExtension(file))) return res.sendStatus(422)\n// and store the file outside the web root",
    evidence:
      "Uploading shell.php with Content-Type: image/png is accepted and stored under a servable path. Requesting it back executes the probe string.",
    fix: [
      { title: "Validate the real extension server-side", body: "Sniff the actual file type and check it against an allow-list. Never trust the client MIME." },
      { title: "Store uploads outside the web root", body: "Keep uploaded files where the web server cannot execute them." },
      { title: "Strip execute permissions on the directory", body: "Mount the upload path noexec so a stray script cannot run." },
    ],
  },
  {
    code: "6479D0",
    sev: "high",
    title: "Subdomain takeover: griffon.rahadoc.com",
    domain: "rahadoc.com",
    scan: "fdc238",
    cvss: "7.2",
    age: "7 weeks",
    effort: "about 30 minutes",
    impact: "A dangling CNAME points at an unclaimed host that an attacker can register.",
    impactLong:
      "griffon.rahadoc.com still resolves to a hosting provider account that no longer exists. Anyone who claims that account serves content from your subdomain, which enables phishing and cookie theft on the parent domain.",
    endpoint: "CNAME griffon.rahadoc.com -> unclaimed.hosting-provider.net",
    snippet:
      "; remove the stale record\ngriffon 300 IN CNAME unclaimed.hosting-provider.net.   ; delete this\n; or re-claim unclaimed.hosting-provider.net before anyone else does",
    evidence:
      "The CNAME target returns the provider's unclaimed-host page. Registering that host would let an attacker serve arbitrary content from griffon.rahadoc.com.",
    fix: [
      { title: "Delete the dangling DNS record", body: "Remove the CNAME if the service behind it is gone." },
      { title: "Or reclaim the host first", body: "If you still need the subdomain, re-register the provider account before deleting anything." },
      { title: "Audit the rest of the zone", body: "Sweep every record for other targets that point at hosts you no longer own." },
    ],
  },
  {
    code: "3F90AB",
    sev: "high",
    title: "Open registration plus sequential user-ID disclosure",
    domain: "legalplus.ma",
    scan: "19e05d",
    cvss: "7.5",
    age: "8 weeks",
    effort: "about 4 hours",
    impact: "Anyone can self-register, and user ids are sequential, so the whole customer base can be enumerated.",
    impactLong:
      "Unauthenticated registration combined with predictable, sequential user ids lets an attacker create an account and walk the id space to map every customer, which feeds the IDOR above.",
    endpoint: "POST https://office.legalplus.co/api/register",
    snippet:
      "// issue opaque ids, not counters\nconst id = crypto.randomUUID()\n// and gate registration behind email verification + a rate limit",
    evidence:
      "Registration succeeds with no invite or verification. New accounts receive ids 9041, 9042, 9043 in order, which exposes account count and ordering.",
    fix: [
      { title: "Gate registration", body: "Require an invite or verified email before an account becomes usable." },
      { title: "Use opaque identifiers", body: "Replace sequential ids with UUIDs so the id space cannot be enumerated." },
      { title: "Rate-limit enumeration", body: "Throttle registration and lookup endpoints per ip and per account." },
    ],
  },
  {
    code: "88EE21",
    sev: "high",
    title: "Stored XSS in the client notes field",
    domain: "1337.ma",
    scan: "f8933b",
    cvss: "7.1",
    age: "2 days",
    effort: "about 3 hours",
    impact: "Client notes are rendered as raw HTML, so a stored script runs in every viewer's session.",
    impactLong:
      "The notes field is written to the page with dangerouslySetInnerHTML. A note saved by one user executes JavaScript in the browser of any staff member who opens that client, which can hijack sessions.",
    endpoint: "POST https://1337.ma/api/clients/{id}/notes",
    snippet:
      "// stop rendering raw HTML\n// before: <div dangerouslySetInnerHTML={{ __html: note }} />\n// after:\n<div>{note}</div>",
    evidence:
      "Saving an image tag with an onerror handler that runs a script fires when the client record is opened by another account.",
    fix: [
      { title: "Stop rendering raw HTML", body: "Render notes as text, or sanitize with an allow-list if formatting is required." },
      { title: "Add a strict Content-Security-Policy", body: "Block inline script so an injected payload cannot run." },
      { title: "Set HttpOnly on session cookies", body: "Keep the session token out of reach of any script that does slip through." },
    ],
  },
  {
    code: "5521CD",
    sev: "med",
    title: "No rate limit on the password-reset endpoint",
    domain: "vaultpay.io",
    scan: "d71ab5",
    cvss: "5.3",
    age: "6 days",
    effort: "about 1 hour",
    impact: "Reset tokens can be brute-forced and the endpoint can be used to mail-bomb a victim.",
    impactLong:
      "With no throttling, an attacker can request resets in a loop to flood a user's inbox and, if tokens are short, guess a valid one before it expires.",
    endpoint: "POST https://vaultpay.io/api/auth/reset",
    snippet:
      "// throttle by ip and by account, and use long single-use tokens\nconst token = crypto.randomBytes(32).toString('hex')\n// return the same response whether or not the address exists",
    evidence:
      "500 reset requests for one address in a minute all return 200 and each sends an email. Tokens are 6 digits with a 60 minute lifetime.",
    fix: [
      { title: "Throttle by ip and by account", body: "Cap reset requests per address and per source so the endpoint cannot be looped." },
      { title: "Use long, single-use tokens", body: "Issue high-entropy tokens that expire quickly and cannot be reused." },
      { title: "Return a constant response", body: "Answer the same way for known and unknown addresses to avoid leaking which accounts exist." },
    ],
  },
  {
    code: "70B4A2",
    sev: "med",
    title: "Session cookie issued without the Secure flag",
    domain: "geocara.com",
    scan: "2d3c0e",
    cvss: "4.8",
    age: "4 days",
    effort: "about 15 minutes",
    impact: "The session cookie can travel over plaintext, where it can be intercepted.",
    impactLong:
      "Without Secure and SameSite, the session cookie can be sent over http and attached to cross-site requests, which exposes it to network interception and CSRF.",
    endpoint: "Set-Cookie: session=... (geocara.com)",
    snippet:
      "Set-Cookie: session=...; Secure; HttpOnly; SameSite=Lax; Path=/",
    evidence:
      "The login response sets the session cookie with no Secure, HttpOnly or SameSite attribute. It is replayed over an http request in the capture.",
    fix: [
      { title: "Add Secure, HttpOnly and SameSite", body: "Set all three so the cookie stays on https, out of script, and off cross-site requests." },
      { title: "Force HSTS at the edge", body: "Send Strict-Transport-Security so browsers never try http for this host." },
      { title: "Rotate existing sessions", body: "Invalidate current sessions so any already-leaked cookie stops working." },
    ],
  },
  {
    code: "9C0D41",
    sev: "low",
    title: "Verbose server banner discloses framework versions",
    domain: "geocara.com",
    scan: "2d3c0e",
    cvss: "3.1",
    age: "4 days",
    effort: "about 15 minutes",
    impact: "Response headers advertise exact framework versions, which helps an attacker pick exploits.",
    impactLong:
      "Server and X-Powered-By headers name the exact stack and version. On its own this is informational, but it lets an attacker target known CVEs for those versions without any probing.",
    endpoint: "Server / X-Powered-By response headers",
    snippet:
      "# nginx\nserver_tokens off;\n# express\napp.disable('x-powered-by')",
    evidence:
      "Responses include Server: nginx/1.18.0 and X-Powered-By: Express, naming the exact versions in use.",
    fix: [
      { title: "Suppress version headers", body: "Turn off server tokens and the powered-by header across every service." },
      { title: "Review the whole header set", body: "Strip any other header that names internal software or infrastructure." },
      { title: "Keep patching on schedule", body: "Hiding the version is not a fix. Stay current on the frameworks themselves." },
    ],
  },
  {
    code: "4410FF",
    sev: "info",
    title: "TLS 1.1 still enabled on a legacy endpoint",
    domain: "companioncare.fr",
    scan: "6fa349",
    cvss: "0.0",
    age: "8 days",
    effort: "about 30 minutes",
    impact: "An old endpoint still negotiates TLS 1.1, a deprecated protocol.",
    impactLong:
      "One legacy endpoint accepts TLS 1.1. There is no active exploit here, but the deprecated protocol should be retired before it becomes a compliance finding.",
    endpoint: "TLS handshake on api.companioncare.fr:443",
    snippet:
      "ssl_protocols TLSv1.2 TLSv1.3;",
    evidence:
      "The handshake on api.companioncare.fr:443 completes with TLS 1.1 offered and accepted.",
    fix: [
      { title: "Restrict to TLS 1.2 and 1.3", body: "Disable everything older at the edge." },
      { title: "Check client analytics first", body: "Confirm no real client still needs 1.1 before you turn it off." },
      { title: "Re-scan to confirm", body: "Run the check again so the informational item drops off the report." },
    ],
  },
];

/* ---------- live scan phases ---------- */
export const PHASES = ["Recon", "Fingerprint", "Vuln discovery", "Exploit validation", "Reporting"];
export const CURRENT_PHASE = 3;

/* ---------- live log ---------- */
export type LogKind = "ok" | "dim" | "warn" | "bad";

export const LOG_TONE: Record<LogKind, ChipTone> = {
  ok: "success",
  dim: "neutral",
  warn: "warn",
  bad: "danger",
};

export type Log = { t: string; k: LogKind; msg: string };

export const LOGS: Log[] = [
  { t: "14:02:11", k: "ok", msg: "authorization verified · signature 19e05d" },
  { t: "14:02:48", k: "dim", msg: "recon complete · 34 hosts, 118 endpoints" },
  { t: "14:06:02", k: "dim", msg: "fingerprint · nginx 1.18, wordpress 6.4, express 4.17" },
  { t: "14:09:37", k: "warn", msg: "candidate IDOR on /api/documents/{id}" },
  { t: "14:11:04", k: "bad", msg: "confirmed · foreign object read, non-destructive probe" },
  { t: "14:13:29", k: "warn", msg: "candidate unrestricted upload on /api/attachments" },
  { t: "14:15:50", k: "bad", msg: "confirmed · php handler executed probe string" },
  { t: "14:16:12", k: "dim", msg: "validating 6 remaining candidates…" },
];

/* ---------- reports ---------- */
export type Report = {
  domain: string;
  type: Intensity;
  scan: string;
  when: string;
  pages: number;
  counts: [Sev, number][];
};

export const REPORTS: Report[] = [
  { domain: "legalplus.ma", type: "Standard", scan: "19e05d", when: "8 weeks ago", pages: 34, counts: [["crit", 2], ["high", 7], ["med", 9], ["low", 2]] },
  { domain: "vaultpay.io", type: "Standard", scan: "d71ab5", when: "6 days ago", pages: 18, counts: [["med", 2], ["low", 2], ["info", 3]] },
  { domain: "companioncare.fr", type: "Quick", scan: "6fa349", when: "8 days ago", pages: 11, counts: [["info", 16]] },
];

/* ---------- global aggregates ---------- */
export const COUNTS: Record<Sev, number> = { crit: 2, high: 31, med: 11, low: 4, info: 130 };
export const TOTAL_OPEN = 178;

/* The agent's canned answer. One authorization fix closes both criticals; the
   upload issue is separate. (The original copy used an em-dash; replaced here.) */
export const AI_REPLY =
  "Both critical findings on legalplus.ma trace to one missing authorization check on the document service. Patch that, then re-scan. The upload issue is separate and needs server-side extension validation.";

export const PANEL_SEED =
  "Two critical findings on legalplus.ma both stem from unauthenticated object access. Fixing the authorization check at the document service closes both. Want the patch outline?";

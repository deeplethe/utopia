# The interface reads tokens and never guesses

Records: [0004] (language), [0005] (alerts), [0038] (theme), `web/DESIGN.md` (the six rules), with
the interface cuts named in [0017], [0019], [0021], [0025], [0031], [0037], [0040], [0041].

## What it does today

**Language follows the reader of each text.** UI strings are a per-person browser setting resolved
in one function, with a Chinese bundle typed as `typeof S` so a missing key fails compilation; the
browser language is not guessed while the bundle trails; user-reachable server errors are codes
(`AppError::Invalid` with `code`) rendered by the client, and the server produces no display text;
`label` is data, `description` follows the base's `ontology_lang`, LLM text for people takes
`locale` from the request, and extracted data stays verbatim [0004].

**Theme.** Dark by default, light by choice, system as a third, stored as `utopia.theme` in the
browser and set on `<html data-theme>` before first paint; only the token block changes, alpha
describes hierarchy and the rgb triplet is what swaps; the canvas has one reader
(`graphVisuals.ts`) that re-reads tokens and rebuilds the graph on a switch, edges flattened over
the ground on light because the WebGL shader can only brighten; a `raw-colour` guard fails CI on a
colour value outside the token blocks and the two reader files; the entity palette is the same in
both themes and identical to the server's copy [0038].

**Design rules** (`web/DESIGN.md`, enforced by `pnpm guard`): five type sizes by name, six spacing
steps, four radii by role derived from one `--radius`, colour as tokens with a status as a dot and
plain words, shadcn/ui behind a thin shell. The merge rule for interface work is look first, not
green first: four defects in the theme cut passed every check and were found by looking [0038].

**Alerts.** One row per failure, never updated; the read side folds adjacent rows of one `(kb, kind)`
into an episode; read state is per person; visibility by role; a row names one subject and carries
no display text, titles assembled by kind on the client; one global SSE stream tells clients to
refetch; classification is a pure function on error types after retries are exhausted; a bell with
a red dot and a popover; a group can carry the action that closes its loop ("Run those again"); rows
purge after 30 days [0005]. Kinds: `source.sync_failed`, `llm.unreachable`, `llm.rate_limited`,
`llm.out_of_credit`, `data_source.schema_sync_failed`, `document.needs_reader`,
`governance.tripped` [0005, 0025, 0040].

**Browse pages.** The graph draws an untyped entity in grey rather than dropping it, an edge outside
the ontology in lighter grey with "the source's wording" on hover, an unnamed relation in italics,
a contested edge in the alert colour (`--u-contest`, coral), a blocked derivation as a ghost edge
behind the Derived toggle, and derived edges gold [0009, 0010, 0017, 0002]. The entity panel lists
names apart from facts, marks derived, stale, corrected and disputed rows, shows a rule's premises
back to the passage, and rewinds derivations with the assertions [0041, 0017, 0021, 0019]. Review
cards show the original sentence above the statements, the agent's proposal chip, a held reason and
a rationale input [0015, 0025, 0026, 0027]. An open statement renders as an unnamed relation
labelled by its phrase [#731]. A page subtitle says the action the screen does, in one sentence.

## Why

- **One switch cannot serve five kinds of text** with different readers and sources; a quotation is
  not copy [0004].
- **A string left in Rust is permanently untranslatable** once the locale lives in the client
  [0004].
- **Colour belongs to data and the chrome is grey**; forty values outside the tokens each said "the
  ground is dark" [0038].
- **`prefers-color-scheme` alone fails** because the choice is the reader's, not the OS's, and two
  token sets fork within a month [0038].
- **Aggregation belongs to the view**: a stored aggregate lies as things recover; a self-healing flag
  needs every kind to define "fixed" [0005].
- **Nobody can read an alert away for someone else** [0005].
- **A bell, not a page**, because a page pulls people from their work and then nobody goes [0005].
- **A ring on the node cannot be told from a grey edge** in peripheral vision, so a contested edge
  changes colour whole [0017].
- **Capability first, interface after**: every record lands the capability and names its interface
  cut separately, and the cut is looked at on a running build before it merges.

## Proposed and not built

- A layer marker for open statements; qualifiers on the canvas label; a rule-classified entity's
  marker; an event drawn as a point; the `as_of` control on the graph page; the anchor beside a
  blank start; a rules-feed-rules page [#731, 0037, 0021, 0031, 0019, 0022, 0030].
- The origin and anchor on evidence rows, opening the page, image or recording; the reader settings
  cards [0040].
- The entity panel's names with sources and validity; evidence lines on cards [0041].
- Review cards assembled from the ledger with zero model calls, one template per queue [#725].
- A paper-tuned entity palette; a theme that follows the account; `styles.css` under the guard
  [0038]; Chinese descriptions and `navigator.language` back in `detect()` [0016 C3, 0004].

## Open questions

- Mixed-language corpora in one base, and bundle drift when English wording changes [0004].
- Whether the `as_of` control is a second slider or a mode switch [0019].
- Too many ghost edges on a large base [0017].

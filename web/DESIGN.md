# The interface, in five rules

Utopia's chrome is neutral dark glass: Geist for text, Marcellus for the wordmark, no hue in the chrome, colour reserved for data and for three semantic states. That language is written down in `web/src/styles.css` and has been since the first screen. What was missing was enforcement — a page could pick any of twelve pixel sizes, any of fourteen paddings, any grey. These rules close that gap. They are checked by `pnpm guard` in CI; a page that breaks them does not merge.

## 1. Five type sizes, by name

| name | size / line | for |
|---|---|---|
| `text-fine` | 11 / 16 | metadata, chip text, table headers, hints under a control |
| `text-small` | 12 / 18 | secondary text, dense rows, captions |
| `text-body` | 13 / 20 | everything else: prose, controls, menus |
| `text-title` | 15 / 22 | section and dialog titles |
| `text-display` | 20 / 28 | the page title, and only that |

No `text-xs`/`text-sm`, no `text-[11px]`. If a size between two steps seems necessary, the step is wrong for the element, not the scale for the size. Weight is `font-medium` for controls and titles, `font-semibold` only on the primary button; `font-bold` is not used in chrome. Numbers in chrome are Geist with `u-num` (tabular figures), never monospace; `font-mono` is for keys, ids, code and URLs.

## 2. Six spacing steps

`1 2 3 4 6 8` (4, 8, 12, 16, 24, 32 px), for padding, margin and gap alike. No half steps, no pixels. Controls carry their own padding — a page never sets padding on a button or an input. Page gutters are `6` or `8`; the gap between two related controls is `2`; between two groups, `4`; between two sections, `6`.

## 3. Two radii

`rounded-lg` (8 px) on anything you press or type into; `rounded-xl` (12 px) on any surface — panel, popover, dialog, card; `rounded-full` on pills and round buttons. Nothing else.

## 4. Colour is a token, never a value

Text is `text-ink`, `text-ink-2`, `text-ink-3` — three levels, primary to faint. Lines are `border-line` and `border-line-strong`. Fills are `bg-surface` (rest), `bg-surface-2` (hover), `bg-surface-3` (selected). Meaning is `ok`, `warn`, `danger`, `contest`, `violet`, and those five appear only where they mean something — a status, a contested edge, a destructive action — never as decoration. `neutral-500`, `white/10`, `rose-400`, `[var(--u-…)]` do not appear in a page; the tokens are defined once in `styles.css` and exposed as Tailwind colours, and that is the only door.

Glass is a surface treatment, not a colour: `glass` for a panel in peripheral vision, `glass-strong` for one being read, and both go solid under the pointer (see the note above `--u-surface-strong-hover`). A page does not write `backdrop-blur`.

## 5. State lives in the component

Hover, focus, active, disabled and motion are defined once, in `web/src/ui/`, and a page never writes `hover:`, `focus:`, `transition` or `duration-`. Every control shows a visible focus ring for keyboard users (`--u-ring`); every disabled control is `opacity-40` with `cursor-not-allowed`; every hover settles in `--u-fast` (120 ms) and leaves in `--u-base` (260 ms). A page that needs a control that does not exist adds it to `ui/`, with all five states, and then uses it.

Concretely, a page renders no raw `<button>`, `<input>`, `<textarea>` or `<select>`; it renders `Button`, `IconButton`, `Input`, `Textarea`, `NativeSelect`, `Dropdown`, `SearchSelect`. Confirmation is `DangerConfirm` or `Dialog`, never `window.confirm`. A hint on hover is `Tooltip`, not a bare `title=` on a span (a `title` on a button that already has a visible label is fine).

## How this is enforced

`web/scripts/style-guard.mjs` scans `web/src/**/*.tsx` for the patterns above and fails CI on any hit outside `web/style-guard.baseline.json`. The baseline lists the pages that have not been migrated yet; a migration PR removes its pages from the baseline and cannot be merged until they pass. New files are never exempt. When the baseline is empty, the file goes.

## Migration order

By weight of `className` sites: Ontology, Graph, Library, Review, Settings; then the rest. A migration PR changes classes and swaps raw controls for components, and touches no logic — that is what makes it reviewable by diff alone.

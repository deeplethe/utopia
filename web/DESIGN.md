# The interface, in six rules

Utopia's chrome is neutral glass in two themes, dark and light: Geist for text, Manrope for the wordmark, no hue in the chrome, colour reserved for data and for a handful of semantic states. The components underneath are [shadcn/ui](https://ui.shadcn.com) (Radix base, Nova preset) behind a thin shell in `web/src/ui/` that keeps this project's vocabulary — see *Where the looks come from* at the end. The language is written down in `web/src/styles.css`. What these rules add is enforcement: without them a page could pick any of twelve pixel sizes, any of fourteen paddings, any grey. They are checked by `pnpm guard` in CI; a page that breaks them does not merge.

## 1. Five type sizes, by name

| name | size / line | for |
|---|---|---|
| `text-fine` | 11 / 16 | metadata, chip text, table headers, hints under a control |
| `text-small` | 12 / 18 | secondary text, dense rows, captions |
| `text-body` | 14 / 22 | everything else: prose, controls, menus |
| `text-title` | 16 / 24 | section and dialog titles; chat reads at this step |
| `text-display` | 20 / 28 | the page title, and only that |

No `text-xs`/`text-sm`, no `text-[11px]`. If a size between two steps seems necessary, the step is wrong for the element, not the scale for the size. Weight is `font-medium` for controls and titles, `font-semibold` only on the primary button; `font-bold` is not used in chrome. Numbers in chrome are Geist with `u-num` (tabular figures), never monospace; `font-mono` is for keys, ids, code and URLs.

## 2. Six spacing steps

`1 2 3 4 6 8` (4, 8, 12, 16, 24, 32 px), for padding, margin and gap alike. No half steps, no pixels. Values of `12` and above are layout, not rhythm — clearance under a floating bar, a footer's breathing room — and are allowed for that. Controls carry their own padding — a page never sets padding on a button or an input. Page gutters are `6` or `8`; the gap between two related controls is `2`; between two groups, `4`; between two sections, `6`.

## 3. Four radii, named by role

A corner is `rounded-cell`, `rounded-control`, `rounded-panel` or `rounded-overlay`, and which one it is follows from what the thing is: a chip, a table cell, a `kbd`, a small icon target is a cell; a button, an input, a select, a segmented group is a control; a card, a list, a dialog body, a code block is a panel; a menu, a popover, a toast, a floating dock — anything that hovers over the page — is an overlay. `rounded-full` only on things that are actually circles: an avatar, a status dot, a colour swatch, a graph node.

The names are the point. `rounded-panel` says what the box is, the way `text-ink-2` says what the grey is for, and that is what the guard can check — a number cannot be wrong, only a role can. Four rather than one because the same absolute radius is not the same roundness at every size: 8 px reads as generously rounded on a 24 px chip and as nearly square on a 300 px panel.

The four values are not picked one by one. They derive from shadcn's single `--radius` (10 px) — cell ×0.8, control ×1, overlay ×1, panel ×1.4 — following shadcn's own scale, in which a card is rounder than the popover that floats over it. One knob sets the roundness of the whole interface. When the four were chosen by hand (4, 6, 8, 12) next to shadcn's 10 px buttons, a button came out rounder than the card it sat in.

That scale has a consequence. A control inside a panel still steps inwards, but an overlay is flatter than a panel, so **a panel does not go inside an overlay** — its corners would step outwards. Rule 6 already rules it out: the form in a dialog is not a panel.

## 4. Colour is a token, never a value

Text is `text-ink` or `text-ink-2` — two levels: the content, and what is said about the content. There is no third, fainter level; a caption, a timestamp or a placeholder is already marked as secondary by where it sits and how big it is, and dimming it again only makes it harder to read. Lines are `border-line` and `border-line-strong`. Fills are `bg-surface` (rest), `bg-surface-2` (hover), `bg-surface-3` (selected). Meaning is `ok`, `warn`, `danger`, `contest`, `violet`, and those five appear only where they mean something — a status, a contested edge, a destructive action — never as decoration. `neutral-500`, `white/10`, `rose-400`, `[var(--u-…)]` do not appear in a page; the tokens are defined once in `styles.css` and exposed as Tailwind colours, and that is the only door.

**A status is a coloured dot and plain words**, not a filled pill in the status's colour: `Status`, with the colour on the dot and the text in `text-ink-2`. A column of filled pills is a band of colour that the eye reads before the words. `Chip` is for things that are not states — a count, a base's name, a mark such as "derived" — which are labels stuck onto content and should have a box around them.

Glass is a surface treatment, not a colour: `glass` for a panel in peripheral vision, `glass-strong` for one being read, and both go solid under the pointer (see the note above `--u-surface-strong-hover`). A page does not write `backdrop-blur`.

The same rule holds outside class names. A colour value — `#rrggbb`, `rgba(…)` — does not appear in a `.ts` or `.tsx` file, nor in a rule in `styles.css`: every value lives in a token block, and a page never knows which theme it is in. There are two families of tokens:

- **This project's** (`--u-*`): ink, lines, surfaces, meaning, and everything the canvas draws with. Dark is in `:root`, light in `:root[data-theme="light"]`.
- **shadcn's** (`--background`, `--border`, `--input`, `--ring` and the rest): what the shadcn components read. Light is in `:root`, dark in `.dark`.

`theme.ts` sets the theme on `<html>` both ways at once, `data-theme` and the `.dark` class. That way a component taken from the shadcn registry follows the theme without an edit, and the canvas and the semantic colours keep reading `data-theme`.

Two files read token values, and they are the only ones allowed to hold a value. `pages/graphVisuals.ts` reads the tokens the canvas needs through `getComputedStyle`, because canvas cannot resolve `var()`, and re-reads them when the theme changes. `palette.ts` holds the entity colours, which are data and must stay byte-identical to `crates/utopia-store/src/palette.rs`. A value read back comes out the way the minifier left it: `#ffffff` as `#fff`, `rgba(176,120,20,0.6)` as `#b0781499`. So the readers parse every CSS colour spelling. A reader that only knows six-digit hex falls back to grey without a word, and that is how every node in the light theme once rendered as a grey blob. A shade of white or black is `rgba(var(--u-ink-rgb), α)` / `rgba(var(--u-ground-rgb), α)` with the alpha left where it was; alpha describes hierarchy, and the triplet is what the theme swaps. `rgba(0,0,0,0)` is transparency, not a colour, and passes.

## 5. State lives in the component

Hover, focus, active, disabled and motion are defined once, in the components, and a page never writes `hover:`, `focus:`, `transition` or `duration-`. Every control shows a visible focus ring for keyboard users; every disabled control is dimmed (`opacity-50`) and does not respond; every hover settles in `--u-fast` (120 ms) and leaves in `--u-base` (260 ms). A page that needs a control that does not exist adds it to `ui/`, with all five states, and then uses it.

Concretely, a page renders no raw `<button>`, `<input>`, `<textarea>` or `<select>`. It renders `Button`, `IconButton`, `Input`, `Textarea`, `Dropdown`, `SearchSelect`, or `MenuSelect` for a "label: value" row inside a menu. There is no native `<select>` anywhere: its popup is drawn by the operating system and cannot be themed, so one page would show two kinds of dropdown. A small bounded enum is a `Dropdown`; a list of hundreds — the classes of an ontology, the people in a deployment — is a `SearchSelect`. Confirmation is `DangerConfirm` or `Dialog`, never `window.confirm`. A hint on hover is `Tooltip`, not a bare `title=` on a span (a `title` on a button that already has a visible label is fine).

## 6. A panel is a slot for content

The first five rules say what a panel looks like. This one says when there is one, and where the things around it go.

A panel holds **several things of the same kind** — the rows of a table, the items of a list. A group of form fields, a block of prose, the only content in a page's main region: no panel. The page is already their container, and a border, a fill and a radius each claim "this is an object separate from its surroundings" — spent on a single object, they say nothing and flatten the hierarchy of everything around them.

A list is **one panel with rows**, not one card per item. Cards per item put seven or eight boxes on a page at the same level, and each card ends up being both the panel and the clickable thing — which is how a slot acquires a hover state it has no business having. With rows, hover belongs to the row (`hover:bg-surface-2`, already the pattern in `ui/table.tsx`) and the panel never responds to the pointer.

**A list of records is a table.** If every row carries the same fields — members, tokens, knowledge bases, rules — it is read across the rows, and the same fact has to land in the same column: `Table`, `Th`, `Td`, with counts right-aligned in `u-num`. The alternative stacks each row as a name, some chips and a line of dot-separated small print, which puts one fact at a different horizontal position on every row and leaves nothing to compare.

**A row's actions are one icon, not standing text.** Actions on every row, written out, make "remove" and "deactivate" the loudest words on a page people mostly come to read. They collapse into a single icon at the end of the row. It appears while the pointer is on that row (`REVEAL`, with `group` on the row) and stays while its menu is open. It opens either a menu (`DropdownMenu`, as on the conversation list) or a dialog (`FormDialog`, as behind the member table's pencil). A row whose only action is the obvious one, such as Restore on a deactivated account, may show it as a button.

Controls that operate on a panel's contents — filter, search, sort, pagination — sit **outside** it, in the page header or above it. They are not content, and when a filter empties the list the panel has to become an empty state without taking the only way to change the filter with it. A filter bar is **one row**: the search box first, `w-64` with its magnifier, then the dropdowns. A count in a dropdown label goes in parentheses, `Pending (3)`, not after a separator dot. The dot means "and", and a count is not a second field.

A card that ends in **a decision** — the cards of the Review queue — puts its actions in one footer shape, `CARD_ACTIONS`. The actions sit bottom-left, on the card's own left edge, so a column of cards lines every decision up vertically and the pointer barely moves between them. The footer wraps, because some cards have five or six options. The destructive action goes last, because the leftmost place is the one the pointer reaches first. Whatever explains the card — why this pair is in the queue, its stage, what the agent suggests — goes **above** the content, where it is read before the decision, not in a corner beside the buttons.

The other thing a panel may be is **the reach of one action**: a settings card (`SettingsCard`) whose footer holds the Save that applies to exactly what the border encloses, and nothing else. The border earns its keep by answering "what does this button send?" — so a page of them is a page of small independent saves, not one long form with a single button at the bottom that quietly ships every field on the screen. A page with only one such card does not need it; the page is already the boundary.

Settings and other read-a-column-of-fields pages are centred and width-limited (`mx-auto max-w-4xl`), not stretched to the window.

Exempt: the floating panels on Graph and Ontology. Those are `glass-strong` surfaces over a canvas, and their job is to hold the canvas down so they can be read — a different problem from this one.

A floating panel **shows**; it does not edit. A class, a property, an entity, a fact's interval are read there, and every change — creating, editing, deleting, connecting — opens a `FormDialog`. That dialog has one title and one form, with Cancel and Save at the bottom right and destructive actions on their own at the bottom left. Usually that is one action, and it can be several when they differ in weight: a member's Deactivate cuts them off across the deployment, while Remove only takes them out of this workspace. The pencil beside the panel's close key is the way in. A form inside the panel put half-edited fields next to the definition being read and Save beside Delete; a dialog gives the change its own frame, its own Esc, and leaves the panel to say what the thing is. The dedicated dialogs live in `pages/ontologyDialogs.tsx` and `pages/graphDialogs.tsx`.

## How this is enforced

`web/scripts/style-guard.mjs` scans `web/src/**/*.{ts,tsx}` for the patterns above and fails CI on any hit (the `web` job runs `pnpm build`, and the guard runs first in it). Block comments are ignored, so a rule can be quoted where it is explained. The `raw-colour` rule skips the two reader files named in rule 4 and `*.test.ts(x)` fixtures. `src/components/ui/`, the shadcn components as generated, is skipped entirely (`VENDOR` in the guard). They are written in Tailwind's native scale rather than in these names, and `shadcn add` regenerates them, so they are not edited by hand. The pages that use them are checked like any other. A new file is checked from its first commit.

## Where the looks come from

`web/src/ui/index.tsx` is a shell; `web/src/components/ui/*` is shadcn. Pages call the shell by what an action weighs, `Button variant="primary" | "secondary" | "ghost" | "danger"`, and the shell maps that onto shadcn's variants (`secondary` becomes shadcn's `outline`, `danger` becomes `destructive`). The pages therefore did not change when the looks moved to shadcn, and would not change again if the preset did. Where a component cannot be used, the shell exports the class string instead (`buttonLike`, `chipLike`).

What the shell keeps that shadcn does not have: the icon slot and the `bare` form of `Input`, the solid fill behind a field that sits over the graph canvas, `Status`, `MenuSelect`, `CARD_ACTIONS`, and the one-or-several danger actions of `FormDialog`. A new control is added to the shell with all five states and then used. A new shadcn component is added with `shadcn add` — never pasted in and edited, because the next `add` would overwrite the edit.

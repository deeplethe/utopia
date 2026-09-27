# 0038 · The interface has a light side

- **Status**: Implemented (#599) · open: a palette for paper, a theme that follows the account
- **Written**: 2026-09-11 (conventions in the [README](README.md))
- **Related**: [0004](0004-language-and-localization.md) put the reader's language in the
  reader's hands; the theme is the same kind of choice and lives in the same place, the
  browser. `web/DESIGN.md` rule 4 said colour belongs to data and the chrome is grey; this
  record is what it took to make that sentence true when the grey flips.

> The chrome was dark from the first screen, and was written as if that were the only case.
> Forty-odd values sat outside the token block: `#ffffff` in a page rule, `rgba(255,255,255,
> 0.08)` in a constant that feeds the graph renderer, a shadow string in a component. Each
> was a small statement of "the ground is dark". A light theme done by changing tokens alone
> was not possible, because the values were not all tokens. An enterprise floor is lit, and
> the people on it look at a screen for eight hours; the interface should have a face for
> that room.

## The problem

Three things were coupled that should not have been.

**Values and rules.** `styles.css` defined the tokens once and then, in a hundred rules
below, sometimes used them and sometimes wrote the number again. A rule that says
`rgba(255, 255, 255, 0.06)` is correct on dark ground and invisible on paper, and there is
no way to find it except by looking.

**Chrome and canvas.** The graph is drawn by sigma.js on WebGL and by hand on a 2D canvas
(the grid, the label slabs, the login scene). Neither can resolve `var(--u-line)`; they take
a colour string. So the canvas colours were constants in TypeScript, computed at module
load, and baked into the graph's node and edge attributes when the graph was built. A
stylesheet change did nothing to them.

**Alpha and colour.** The dark chrome is layers of white at low alpha over near-black. The
hierarchy — a line is `0.08`, a strong line `0.14`, a surface `0.03` — is carried by the
alpha, and the alpha was written together with the white it multiplied. On paper the same
hierarchy is layers of black at the same alphas. The number that should survive the flip was
fused to the number that should not.

## Dead ends

**A `prefers-color-scheme` block.** The obvious shape is two media-query blocks in CSS and
no JavaScript. It fails on the first requirement: the choice is the reader's, not the
operating system's, and a person on a light desktop may want the dark interface. A CSS block
per source of truth (`data-theme` for the choice, the media query for "system") would be the
same forty tokens written twice more, and two copies of a token set fork within a month.
The stylesheet knows one attribute, `data-theme`, with two values; "system" is resolved to
one of the two in JavaScript, once at start and again when the OS preference changes.

**Rebuilding the canvas colours by hand.** The first pass read the tokens once before
building the graph, and the theme switch worked on the chrome and not on the graph: the
colours were already in the graph attributes. Re-reading the tokens is not enough; the graph
has to be rebuilt from them. The switch now bumps a counter that the build effect depends
on, so the same code path that builds the graph on data change builds it on theme change.

**Trusting alpha on WebGL.** With the chrome and the graph attributes both reading tokens,
the edges on paper came out *white*. sigma's edge shader blends with premultiplied alpha but
writes un-premultiplied colour, which means an edge at alpha can only brighten what is under
it, never darken it. On a dark ground that is invisible; on paper every edge is a pale line.
The fix is not in the shader: the edge colours are flattened over the ground colour at read
time and handed to sigma opaque. The light-theme edge tokens are therefore written as "how
deep after flattening", not as an alpha the screen will see; the comment beside them says
so.

**A guard that only read `.tsx`.** The style guard scanned page files. The grid colour was a
hard-coded white in a `.ts` file that the guard did not open, and it stayed white on paper
until someone looked. The guard now walks `.ts` as well, ignores block comments (so a rule
can be quoted where it is explained), and skips only the two files that are allowed to hold
a value.

## Decisions

1. **Dark is the default; light is a choice; system is a third.** Stored as `utopia.theme`
   in the browser, never sent to the backend. A script in `index.html` runs before first
   paint and sets `data-theme` from storage, so a refresh does not flash dark before turning
   light. The menu under the avatar offers the three.

2. **Only the tokens change.** `:root[data-theme="light"]` redefines every token that carries
   a colour and nothing else; not one rule below the token blocks is rewritten. Ground is
   paper (`#fafafa`), not white; text is near-black, not black; lines and surfaces go from
   white-at-alpha to black-at-alpha with the alpha unchanged. Semantic colours (ok, warn,
   danger, contest, derived) become a deeper shade of the same hue, because the pale ones
   built for dark ground cannot be read on paper.

3. **Alpha describes hierarchy; the triplet is what the theme swaps.** A shade of white or
   black in a rule is `rgba(var(--u-ink-rgb), α)` or `rgba(var(--u-ground-rgb), α)`. The
   `-rgb` triplets exist for this: they let a rule keep its alpha and let the theme keep the
   colour. The rgb-triplet tokens for the semantic colours exist for the same reason.

4. **The canvas has one reader.** `pages/graphVisuals.ts` reads the tokens the canvas needs
   through `getComputedStyle`, exports them as live bindings, and re-reads them in
   `refreshPalette()` before every graph build and on every theme change. The graph pages
   subscribe to the switch, refresh the palette, rebuild the graph, push the label colours
   into sigma's settings (they are captured at construction, not read per frame), and redraw
   the grid. Edge colours pass through the flattening above when the theme is light.

5. **A colour value has two homes.** The token blocks in `styles.css`, and the two reader
   files (`graphVisuals.ts`, and `palette.ts` for the entity colours, which are data and must
   stay byte-identical to `palette.rs` on the server). The `raw-colour` rule in the style
   guard fails CI on a `#rrggbb` or `rgba(…)` anywhere else in `web/src`, `.ts` and `.tsx`
   alike; `rgba(0,0,0,0)` is transparency, not a colour, and passes. `styles.css` itself
   holds no value outside its token blocks; that is checked by eye, not by the guard, and is
   the one gap here.

6. **The entity palette does not change with the theme.** Colour belongs to data (rule 4),
   and the same entity type must be the same colour on every screen and in every export.
   The palette was tuned on dark ground and some hues are pale on paper; a paper-tuned set
   would have to change the server's copy too, and that is a separate decision.

## What the look found (2026-09-11)

The first build of this cut passed the guard, the type check and the tests, and was looked
at in both themes before it merged. Four things were wrong that no check could have caught:
the grid was gone on paper (a white line in an exempt file), the edges were white (the
shader, above), the labels on the graph followed the old theme (captured settings), and a
small graph stopped showing its predicates after the switch — the label threshold was
written for a large graph at a zoomed-out ratio, and a graph of nine edges at ratio 1 should
show them regardless. The threshold now has a size clause. Each of these is why the merge
rule for interface work is "look first" and not "green first".

## Open questions

- **A paper palette.** Whether the entity colours get a second set for light ground, held on
  both the client and the server, or the dark set is adjusted once so that it reads on both.
- **Following the account.** The theme is a property of the browser today. If a reader moves
  between machines and expects the choice to follow, it becomes a user setting on the
  backend; language will face the same question first (0004).
- **`styles.css` under the guard.** The token blocks are the only place a value may appear
  in the stylesheet; nothing enforces it. A rule that scans the stylesheet below its token
  blocks is a small addition when it is next touched.

## Status history

The status line as it stood on 2026-09-27, before status lines were cut to one line:

> Implemented (#599) · a `data-theme` on `<html>`, a second token block in `styles.css`, the canvas reads its colours from the tokens and re-reads them on a switch, a `raw-colour` rule in the style guard · not in this cut: an entity palette tuned for paper (the data colours are the same in both themes and must stay equal to the server's copy), a theme choice that follows the account across browsers

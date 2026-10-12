# Design

The game's UI follows one look, "ember lobby": dark angular panels, an ember-orange accent, a
hazard band along the top, condensed display titles and monospace numbers. Tokens, fonts, shapes
and the shared widgets live in `src/ui.rs`; no other file defines a UI colour or font.

## Tokens (`src/ui.rs`)

| Token | Use |
|---|---|
| `INK` | primary text, titles |
| `TEXT` | button labels, list rows |
| `DIM` | secondary text, captions, column heads |
| `MUTE` | key hints, unlit indices, disabled labels |
| `ACCENT` | ember orange: selection, the main action, lit segments, section indices |
| `GOLD` | prices, bounty, the ring of a chosen tile |
| `ON_ACCENT` | text on the accent |
| `DANGER` | damage, death, low health, errors, prices you cannot afford |
| `OK` | armour, confirmations, owned and equipped items, better stats |
| `RED`, `BLUE` | teams |
| `BACKDROP` | the screen behind the menus |
| `SURFACE` | panels |
| `RAISED` | tiles, rows and wells on a panel |
| `BUTTON` | buttons on a panel |
| `EDGE` | unlit segments, empty tracks, tile outlines |
| `PLATE` | HUD slabs over the game |
| `LINE` | hairlines |
| `SCRIM`, `SHADOW` | overlay dimming, drop shadows |

## Type

Big Shoulders Display ExtraBold (`ui::display`) for titles, tabs, big values and the START label;
Chakra Petch (`ui::body` Medium for sentences, `ui::compact` SemiBold for rows, `ui::strong` Bold for
buttons and names); JetBrains Mono Bold (`ui::mono`, `ui::digits`) for every number, index, key cap
and caption. Captions (`ui::caption`) are letter-spaced mono capitals (`ui::tracking(0.14, size)`).
All SIL OFL, `assets/fonts`. Nothing in the HUD is smaller than 13 px.

## Layout

Menus, the pause and end screens, the loading screen and the settings overlays are laid out on a
1920 x 1080 canvas that `menu::fit` scales to the window; the match HUD is laid out at 1280 x 720
(`menu::fit_hud`). Every menu page sits between the same header (hazard band, logo, F1-F7 tabs,
bounty, medals, XP bar, level badge) and footer (ESC QUIT, the match summary, JOIN LAN, HOST LAN,
START). A page is one to three panels side by side, each opening with a numbered section title
(`ui::section(1, "MAP")`).

## Shape and motion

Angular, never rounded: `ui::Cut::Chamfer` for panels and hero frames, `Notch` for tiles, rows and
secondary buttons, `Slant` for choice buttons; `ui::shape` draws them, so a shaped node keeps its
`BackgroundColor` clear. A selected choice is filled with the accent and its label turns
`ON_ACCENT`; values on a scale are segment bars (`ui::segs`). Faint diagonal hatching
(`ui::hatch`) fills empty art frames and section rules; the hazard band (`ui::hazard`) marks the
top edge and the SELECTED flag. One entrance per event with `ui::ease_out` (transform and alpha
only, 150 ms or less), then hold and fade. Feedback for an action (saved, purchased, sold) is a
short toast, not a permanent line.

## HUD placement

Vitals bottom left, weapon and ammo bottom right, clock and scores top centre, kill feed top right,
crosshair with hit markers and the kill confirmation in the centre, round banners upper centre.

## Code

Spawn once, update in place, write a component only when its value changes. Reuse `ui::button`,
`ui::primary`, `ui::flat_cut`, `ui::panel`, `ui::section`, `ui::caption`, `ui::key`, `ui::segs` and
`ui::hover` (labels marked `ui::OnFill` follow their button's state).

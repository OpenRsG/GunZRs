# Design

The game's UI follows one look, "tactical": a modern military-shooter HUD in GunZ's amber. Tokens,
fonts and the shared widgets live in `src/ui.rs`; no other file defines a UI colour or font.

## Tokens (`src/ui.rs`)

| Token | Use |
|---|---|
| `INK` | primary text |
| `DIM` | secondary text, captions |
| `ACCENT` | amber: selection, the main action, the player's own name and row |
| `ON_ACCENT` | text on the accent |
| `DANGER` | damage, death, low health, errors |
| `OK` | armour, confirmations |
| `RED`, `BLUE` | teams |
| `SURFACE` | menu panels |
| `RAISED` | buttons and rows on a panel |
| `PLATE` | HUD slabs over the game |
| `LINE` | hairlines |
| `SCRIM`, `SHADOW` | overlay dimming, drop shadows |

## Type

Barlow (SIL OFL, `assets/fonts`). `ui::body` (Barlow Medium) for sentences only; `ui::compact`,
`ui::strong` and `ui::display` (Barlow Condensed Medium, SemiBold, Bold) for rows, buttons and
titles; `ui::digits` (condensed bold, tabular figures) for every number that changes in place.
Captions, buttons, headings and banners are capitals. Nothing in the HUD is smaller than 13 px.

## Shape and motion

Sharp corners (0 to 3 px), 1 px hairlines, 2 to 3 px accent bars for state; no rounded cards in the
HUD, no glows, no gradient text. One entrance per event with `ui::ease_out` (transform and alpha
only, 150 ms or less), then hold and fade.

## HUD placement

Vitals bottom left, weapon and ammo bottom right, clock and scores top centre, kill feed top right,
crosshair with hit markers and the kill confirmation in the centre, round banners upper centre.

## Code

Spawn once, update in place, write a component only when its value changes. Reuse `ui::button`,
`ui::primary`, `ui::panel`, `ui::heading` and `ui::hover`.

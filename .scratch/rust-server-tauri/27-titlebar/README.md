# Ticket 27, as a photograph

What the window looks like with no frame on it. Kept because the ticket's
acceptance is "one bar rather than two", which is a thing you look at — and
because the two most expensive mistakes in building it were both visible here
and in nothing else.

| File | |
| --- | --- |
| `window.png` | The whole window, captured with `tools/ui-driver/window-shot.ps1`. No operating-system titlebar above the topbar |
| `topbar-right-corner.png` | The right-hand end at 4x: the header's trailing controls, the two panel toggles, then the three caption buttons, every glyph on one line |

The first mistake was the buttons being 40px tall — Electron's overlay height —
inside a 52px bar, which puts the caption glyphs six pixels above everything
beside them. The second was the topbar's padding and `--workspace-controls-right`
crossing over, which left one pixel between "Commit & push" and the panel
toggles. Both were reported by a person looking at the window and neither had a
test that could have failed.

`tools/ui-driver/titlebar-boxes.mjs` is the numbers behind the second one, and
is the reproducible half of this directory: it prints where the three things in
that corner actually are, and `--plain` prints the same for an ordinary browser
tab to compare against.

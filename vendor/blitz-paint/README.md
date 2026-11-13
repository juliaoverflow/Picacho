# Blitz Paint inline-gradient patch

Source: `blitz-paint` 0.3.0-beta.4 from crates.io, https://github.com/DioxusLabs/blitz.
License: MIT OR Apache-2.0.

Two source changes, both for gradient backgrounds that upstream drops:
`draw_inline_backgrounds` in `src/text.rs`, and the row and row-group colors
in `draw_table_row_backgrounds` (`src/render/background.rs`). Upstream
paints an inline box's `background-color` but not its `background-image`.
Emails' dark styles commonly set `background: linear-gradient(c, c)` on an
element whose light `background-color` comes from a later rule; browsers paint
the gradient over that color, so the element is dark. Without the gradient,
a `<span>` such as a tracking number showed white text on a white box. The
patch paints an inline box's gradient layers over its background color, and
gives a table row or row group whose top layer is a one-color gradient that
color (behind a cell's rounded corners, the light color showed as triangles).

Remove this patch when upstream paints inline background images.

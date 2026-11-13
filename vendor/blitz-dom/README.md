# Blitz DOM patches

Source: `blitz-dom` 0.3.0-beta.4 from crates.io, https://github.com/DioxusLabs/blitz.
License: MIT OR Apache-2.0.

## Unresolvable URLs

`Document::resolve_url` in `src/document.rs`. Upstream panics when an
`img src`, stylesheet `href` or form `action` can't be resolved against the
document's base URL. Picacho gives emails the base `about:blank` (so relative
resources have no sender-controlled origin), against which every relative
address fails, as does any malformed one such as `http://[`. An email could
therefore stop itself from being shown. The patch resolves such addresses to
`about:invalid`, which Picacho's resource and link providers refuse.

Remove when upstream stops panicking on unresolvable URLs.

## Tables restacked for narrow screens

Emails restack their layout for narrow screens by giving tables or cells
another `display` in a media query, which upstream laid out wrongly:

- A row group or row outside a table (its table made `display: block`) is
  wrapped in an anonymous table in CSS. Upstream laid it out as a flex-like
  flow, putting its rows side by side. It is now laid out as a table of its
  own (`is_orphan_table_part` in `src/layout/construct.rs`; a row as the root
  in `build_table_context`, `src/layout/table.rs`).
- A block or inline box directly in a row (a cell made `display: block`) is
  wrapped in an anonymous cell in CSS. Upstream dropped it, so the row's
  content vanished. It is now laid out as that cell (`collect_table_cells`).
  Unlike CSS, consecutive inline boxes each get a cell rather than sharing one,
  and the anonymous table is as wide as its container, not shrink-to-fit.

## Legacy table attributes

`synthesize_presentational_hints_for_legacy_attributes` in `src/stylo.rs` maps
`cellspacing` (table `border-spacing`), `cellpadding` (padding of the table's
own cells) and `valign` (`vertical-align` of table parts). Nearly every HTML
email sets `cellspacing="0" cellpadding="0"`; without them colored cells sat
apart, with seams of the page between them.

Remove these when upstream supports them.

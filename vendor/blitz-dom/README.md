# Blitz DOM unresolvable-URL patch

Source: `blitz-dom` 0.3.0-beta.4 from crates.io, https://github.com/DioxusLabs/blitz.
License: MIT OR Apache-2.0.

The only source change is `Document::resolve_url` in `src/document.rs`. Upstream
panics when an `img src`, stylesheet `href` or form `action` can't be resolved
against the document's base URL. Picacho gives emails the base `about:blank`
(so relative resources have no sender-controlled origin), against which every
relative address fails, as does any malformed one such as `http://[`. An email
could therefore stop itself from being shown. The patch resolves such addresses
to `about:invalid`, which Picacho's resource and link providers refuse.

Remove this patch when upstream stops panicking on unresolvable URLs.

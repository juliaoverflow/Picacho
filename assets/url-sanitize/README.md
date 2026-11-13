# url-sanitize rule catalog

`catalog.json` is the merged rule catalog embedded in the `url-sanitize`
2.4.1 release (https://github.com/antonio-orionus/url-sanitize), copied
unchanged from that crate. Picacho compiles it with `url-sanitize-core` to
remove tracking from links in mail (`src/manage/tracking.rs`).

The url-sanitize code is MIT (`LICENSES/MIT-url-sanitize.txt`). The rule
data keeps its sources' licenses, as `NOTICE` explains:

- ClearURLs Rules: LGPL-3.0-only (`LICENSES/LGPL-3.0-only.txt`)
- Brave adblock lists and Firefox Query Stripping: MPL-2.0
  (`LICENSES/MPL-2.0.txt`)

`picacho-rules.json` holds Picacho's own rules in the same format (source
`custom`), for trackers in mail the catalog doesn't know, such as Best Buy's
MessageGears click addresses. They are added to the catalog's rules when
it is loaded. Add rules there rather than in `catalog.json`.

To update, replace `catalog.json` with the one from a newer `url-sanitize`
crate (`catalog/catalog.json`) and bump `url-sanitize-core` to match.

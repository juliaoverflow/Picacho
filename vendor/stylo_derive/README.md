# Stylo derive compatibility patch

Source: `stylo_derive` 0.22.0 from crates.io, https://github.com/servo/stylo.
License: MPL-2.0 (the original source headers are retained).

The only source change is in `to_css.rs`: generated `Ok(())` expressions explicitly
name `std::fmt::Error`. GPUI enables log's serde support, which adds
`From<serde_fmt::Error> for std::fmt::Error`; otherwise Stylo's generated closures
have ambiguous error types and the combined application does not compile.

Remove this patch when upstream's ToCss derive emits explicit error types.

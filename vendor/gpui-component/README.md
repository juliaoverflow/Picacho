# GPUI Component patches

Source: `gpui-component` 0.5.1 from crates.io,
https://github.com/longbridge/gpui-component.
License: Apache-2.0.

## Window resize edges over the content

`resize_edge` in `src/window_border.rs`. With client-side decorations (Linux
and BSD), upstream found the resize edge under the pointer against
`window.window_bounds().get_bounds()`. While the window is maximized those are
its restore bounds, smaller than the surface, so everything below (and right
of) the restored size counted as an edge: the pointer showed a resize cursor
there and a click started a window resize instead of reaching the list,
sidebar or reader. Edges are now measured against `window.viewport_size()`,
and tiled or maximized sides, which have no shadow to grab, are never edges.

Remove when upstream measures edges against the drawn surface.

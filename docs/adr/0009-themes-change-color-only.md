# Themes change color only

A Consort theme sets about 25 color roles for a dark mode and a light mode. Type, spacing, corners, motion and icons are fixed in `design/structure.json` and are the same in every theme. One fixed mapping derives all 99 GPUI Kit color keys and 40 highlight keys from those roles. So no theme can give one component a style of its own, and every theme, built in or made by a user, keeps the same contrast rules.

## Considered options

- Let themes set GPUI Kit keys directly, as GPUI Kit's own theme files do. Rejected because a theme could then style one component differently from the rest, and the UI would stop being one system.
- Let themes also change fonts or corner sizes. Rejected because type and corners carry most of the product's identity, and the layouts are tuned to one font's metrics.

## Consequences

- The theme loader rejects a theme that fails a contrast rule, and it shows the role and the measured ratio.
- A user who wants a larger text size gets it from an app setting that scales the type, not from a theme.

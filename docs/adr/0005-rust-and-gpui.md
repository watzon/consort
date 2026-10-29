# Rust for the hub, the worker and the desktop app, with GPUI and GPUI Kit for the desktop UI

All system code is Rust, and the desktop app uses GPUI with GPUI Kit. GPUI is for the desktop only, although GPUI has experimental mobile support.

## Considered options

- TypeScript for all parts. Rejected by the user.

## Consequences

GPUI Kit 0.8.1 depends on `gpui-pre` 0.3.10 with an exact version. `gpui-pre` is a snapshot of Zed's GPUI that the GPUI Kit maintainer publishes, because the `gpui` crate on crates.io stopped at 0.2.2 in October 2025. We pin exact versions of both crates and accept the upgrade risk that comes with a snapshot that one person publishes.

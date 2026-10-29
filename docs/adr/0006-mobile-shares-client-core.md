# Mobile apps render a shared Rust client core

The mobile front ends are Expo or native Swift and Kotlin, not GPUI. All client logic except drawing lives in the `client-core` crate: the connection, authentication, the local cache, sync and the view state. The public API of `client-core` uses only types that UniFFI can export. So both choices stay open until a spike decides: Expo through `uniffi-bindgen-react-native`, or native through UniFFI. Element X follows the same pattern with matrix-rust-sdk.

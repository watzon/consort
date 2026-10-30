//! The hub's integration tests, in one test binary.
//!
//! Each file in `tests/` is a module here, so cargo builds and links one
//! binary instead of one per file. Run them with `cargo nextest run`, which
//! runs each test in its own process, as the separate binaries did.

mod common;

mod accounts;
mod admin_revoke;
mod assistant_limits;
mod attachments;
mod backup;
mod channel_tags;
mod channels_and_routines;
mod connectors;
mod coordinator;
mod coordinator_acted;
mod coordinator_model;
mod coordinator_recovery;
mod data_perms;
mod frame_size;
mod heartbeat;
mod memory_and_blobs;
mod offline_and_events;
mod pairing_and_auth;
mod phase4_tasks;
mod proactivity;
mod refused_turns;
mod reopened_and_decisions;
mod review_fixes;
mod revoke_and_handshake;
mod routines_and_memory;
mod session_end;
mod size_budgets;
mod skills;
mod smaller_items;
mod snapshot_and_feed;
mod start_failures;
mod task_lifecycle;
mod task_session_guards;
mod wake_queue;
mod workspace_release;

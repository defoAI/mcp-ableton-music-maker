//! Supabase-backed music-creation trajectory dataset recording. Opt-in and
//! off by default: nothing is recorded until telemetry is enabled *and* the
//! user has said yes.

pub mod consent;
pub mod hierarchy;
pub mod passive_poller;
pub mod recorder;
pub mod schema;
pub mod snapshot;
pub mod supabase;
pub mod trajectory;

pub use consent::{consent_state, needs_prompt, record_consent};
pub use recorder::{dataset_enabled, get_recorder, reset_recorder_for_tests, SessionRecorder};

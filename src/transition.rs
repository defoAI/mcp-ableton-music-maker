//! Transitions on a jump: composed into the cue primitives the script
//! already runs (ramps, clip fires and stops, note rewrites into the target
//! row). Filled in by phase 2 of the sections-and-songs story.

use crate::connection::LiveState;
use crate::performance::{CueStep, PerfState};
use crate::song::Section;
use serde_json::Value;

/// Expand a `transition` object into cue steps that go with the jump to
/// `target` at `bar`, plus one display line per primitive.
pub fn expand(
    _live: &LiveState,
    _state: &PerfState,
    _secs: &[Section],
    _target: &Section,
    _bar: i64,
    _t: &Value,
) -> Result<(Vec<CueStep>, Vec<String>), String> {
    Err("transition: not available yet".into())
}

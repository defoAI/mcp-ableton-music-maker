//! Shared test doubles.
#![allow(dead_code)]

use mcp_ableton_music_maker::connection::{LiveBridge, LiveError, LiveResult, LiveState};
use mcp_ableton_music_maker::tools::Server;
use rmcp::model::CallToolResult;
use serde_json::{json, Value};
use std::collections::{HashMap, VecDeque};
use std::sync::{Arc, Mutex};

/// Stand-in for the Live bridge. Records every command sent and returns a
/// canned response (or error) so tool logic is tested in isolation.
pub struct FakeBridge {
    pub response: Mutex<LiveResult<Value>>,
    pub sent: Mutex<Vec<(String, Value)>>,
    /// Fail every command from the Nth one on (0-based), to test partial
    /// progress in multi-command bodies.
    pub fail_from: Mutex<Option<(usize, LiveError)>>,
    /// Per-command response sequences; the last one repeats. Commands not
    /// scripted get `response`.
    pub scripted: Mutex<HashMap<String, VecDeque<Value>>>,
    /// A clock to attach to every response, as the Remote Script does while
    /// a performance runs.
    pub clock: Mutex<Option<Value>>,
}

impl FakeBridge {
    pub fn responding(response: Value) -> Arc<Self> {
        Arc::new(Self {
            response: Mutex::new(Ok(response)),
            sent: Mutex::new(Vec::new()),
            fail_from: Mutex::new(None),
            scripted: Mutex::new(HashMap::new()),
            clock: Mutex::new(None),
        })
    }

    /// Attach this clock to every response from now on (None to stop).
    pub fn set_clock(&self, clock: Option<Value>) {
        *self.clock.lock().unwrap() = clock;
    }

    /// Answer `command` with these responses in order; the last one repeats.
    pub fn script(&self, command: &str, responses: Vec<Value>) {
        self.scripted
            .lock()
            .unwrap()
            .insert(command.to_string(), responses.into());
    }

    pub fn failing(error: LiveError) -> Arc<Self> {
        Arc::new(Self {
            response: Mutex::new(Err(error)),
            sent: Mutex::new(Vec::new()),
            fail_from: Mutex::new(None),
            scripted: Mutex::new(HashMap::new()),
            clock: Mutex::new(None),
        })
    }

    pub fn set_response(&self, response: Value) {
        *self.response.lock().unwrap() = Ok(response);
    }

    pub fn fail_from(&self, nth: usize, error: LiveError) {
        *self.fail_from.lock().unwrap() = Some((nth, error));
    }

    pub fn sent(&self) -> Vec<(String, Value)> {
        self.sent.lock().unwrap().clone()
    }

    pub fn commands(&self) -> Vec<String> {
        self.sent().into_iter().map(|(c, _)| c).collect()
    }
}

impl LiveBridge for FakeBridge {
    fn send_command(&self, command_type: &str, params: Option<Value>) -> LiveResult<Value> {
        let n = {
            let mut sent = self.sent.lock().unwrap();
            sent.push((
                command_type.to_string(),
                params.unwrap_or_else(|| json!({})),
            ));
            sent.len() - 1
        };
        mcp_ableton_music_maker::connection::note_exchange_clock(
            self.clock.lock().unwrap().clone(),
        );
        if let Some((from, err)) = self.fail_from.lock().unwrap().as_ref() {
            if n >= *from {
                return Err(err.clone());
            }
        }
        if let Some(queue) = self.scripted.lock().unwrap().get_mut(command_type) {
            if queue.len() > 1 {
                return Ok(queue.pop_front().unwrap());
            }
            if let Some(last) = queue.front() {
                return Ok(last.clone());
            }
        }
        self.response.lock().unwrap().clone()
    }
}

/// A server wired to the fake bridge, with every capability advertised and
/// the activity log off, so tests never write into the developer's home.
pub fn server_with(bridge: Arc<FakeBridge>) -> Server {
    let live = Arc::new(LiveState::with_activity(
        bridge,
        mcp_ableton_music_maker::activity::Activity::disabled(),
    ));
    live.script.assume_all_capabilities();
    Server::new(live)
}

pub fn text_of(result: &CallToolResult) -> String {
    result
        .content
        .iter()
        .filter_map(|c| c.as_text().map(|t| t.text.clone()))
        .collect::<Vec<_>>()
        .join("")
}

pub fn is_error(result: &CallToolResult) -> bool {
    result.is_error.unwrap_or(false)
}

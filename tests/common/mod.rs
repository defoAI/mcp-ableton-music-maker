//! Shared test doubles.
#![allow(dead_code)]

use mcp_ableton_music_maker::connection::{LiveBridge, LiveError, LiveResult, LiveState};
use mcp_ableton_music_maker::tools::Server;
use rmcp::model::CallToolResult;
use serde_json::{json, Value};
use std::sync::{Arc, Mutex};

/// Stand-in for the Live bridge. Records every command sent and returns a
/// canned response (or error) so tool logic is tested in isolation.
pub struct FakeBridge {
    pub response: Mutex<LiveResult<Value>>,
    pub sent: Mutex<Vec<(String, Value)>>,
}

impl FakeBridge {
    pub fn responding(response: Value) -> Arc<Self> {
        Arc::new(Self {
            response: Mutex::new(Ok(response)),
            sent: Mutex::new(Vec::new()),
        })
    }

    pub fn failing(error: LiveError) -> Arc<Self> {
        Arc::new(Self {
            response: Mutex::new(Err(error)),
            sent: Mutex::new(Vec::new()),
        })
    }

    pub fn set_response(&self, response: Value) {
        *self.response.lock().unwrap() = Ok(response);
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
        self.sent.lock().unwrap().push((
            command_type.to_string(),
            params.unwrap_or_else(|| json!({})),
        ));
        self.response.lock().unwrap().clone()
    }
}

/// A server wired to the fake bridge, with every capability advertised.
pub fn server_with(bridge: Arc<FakeBridge>) -> Server {
    let live = Arc::new(LiveState::new(bridge));
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

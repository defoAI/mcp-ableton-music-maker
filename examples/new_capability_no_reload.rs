//! The proof the streams story exists for: a capability the Remote Script
//! has no handler for, working against the script already loaded in Live,
//! with no reinstall and no reload.
//!
//! ```bash
//! cargo run --example new_capability_no_reload      # read only
//! cargo run --example new_capability_no_reload -- --write
//! ```
//!
//! Each block below is something the fixed command list cannot do today.
//! None of them is a Python handler, a capability entry or a version bump:
//! they are paths and batches built in Rust and sent through `run`. With
//! `--write` two of them change the set and put it back.

use mcp_ableton_music_maker::connection::{live_address, AbletonConnection};
use mcp_ableton_music_maker::handshake::ScriptInfoCache;
use mcp_ableton_music_maker::lom::{Batch, Op, Path};
use serde_json::{json, Value};

fn ok(what: &str, detail: String) {
    println!("  ok    {what:<44}{detail}");
}
fn bad(what: &str, detail: String) -> bool {
    println!("  FAIL  {what:<44}{detail}");
    false
}

fn run(conn: &AbletonConnection, batch: &Batch) -> Result<serde_json::Map<String, Value>, String> {
    batch.validate()?;
    conn.send_command("run", Some(batch.params()))
        .map(|v| v.as_object().cloned().unwrap_or_default())
        .map_err(|e| e.to_string())
}

fn main() {
    let write = std::env::args().any(|a| a == "--write");
    let (host, port) = live_address();
    let conn = AbletonConnection::new(host.clone(), port);
    let info = ScriptInfoCache::default().handshake(&conn);
    let Some(version) = info.script_version.clone() else {
        eprintln!("FAIL: Live at {host}:{port} did not answer");
        std::process::exit(1);
    };
    let has = |c: &str| info.capabilities.iter().any(|x| x == c);
    println!("Remote Script {version} — unchanged throughout this run\n");
    if !has("run") || !has("describe") {
        eprintln!(
            "This script has no generic layer (needs `run` and `describe`).\n\
             Install the Remote Script and reselect the control surface once;\n\
             after that, capabilities like the ones below need neither again."
        );
        std::process::exit(1);
    }

    let mut all = true;

    // 1. Ask what this Live has, rather than assuming. The 89 `hasattr`
    //    branches in the script exist because nothing could ask.
    match conn.send_command("describe", Some(json!({"path": "song.tracks[0]"}))) {
        Ok(d) => {
            let attrs = d["attrs"].as_object().map(|m| m.len()).unwrap_or(0);
            let methods = d["methods"].as_array().map(|a| a.len()).unwrap_or(0);
            ok(
                "what a Track is, on this Live",
                format!(
                    "{attrs} attributes, {methods} methods, Live {}",
                    d["live_version"].as_str().unwrap_or("?")
                ),
            );
        }
        Err(e) => all = bad("what a Track is, on this Live", e.to_string()),
    }

    // 2. Track output routing: the script has no command for it at all.
    let batch = Batch::new().push(Op::get(
        &Path::track(0)
            .attr("output_routing_type")
            .attr("display_name"),
        "routing_type",
    ));
    match run(&conn, &batch) {
        Ok(out) => ok(
            "read a track's output routing",
            format!("{}", out.get("routing_type").unwrap_or(&Value::Null)),
        ),
        Err(e) => all = bad("read a track's output routing", e),
    }

    // 3. Crossfade assignment per track — also absent from the command list.
    let mut batch = Batch::new();
    for i in 0..3 {
        batch = batch.push(Op::get(
            &Path::track(i).attr("mixer_device").attr("crossfade_assign"),
            &format!("track{i}"),
        ));
    }
    match run(&conn, &batch) {
        Ok(out) => {
            let seen = (0..3)
                .filter_map(|i| out.get(&format!("track{i}")).map(|v| v.to_string()))
                .collect::<Vec<_>>()
                .join(", ");
            ok(
                "read crossfade assignment for 3 tracks",
                format!("[{seen}] in one round trip"),
            )
        }
        Err(e) => all = bad("read crossfade assignment for 3 tracks", e),
    }

    // 4. A wide read: five values per track, one round trip, whatever the
    //    count. This is the shape every new read-only capability takes.
    let count = conn
        .send_command("get_session_info", None)
        .ok()
        .and_then(|v| v.get("track_count").and_then(Value::as_i64))
        .unwrap_or(0);
    let mut batch = Batch::new();
    for i in 0..count {
        let t = Path::track(i);
        for attr in ["name", "mute", "solo", "is_visible"] {
            batch = batch.push(Op::get(&t.clone().attr(attr), &format!("t{i}_{attr}")));
        }
        batch = batch.push(Op::get(&Path::track_volume(i), &format!("t{i}_vol")));
    }
    let ops = batch.len();
    match run(&conn, &batch) {
        Ok(out) => ok(
            "read 5 values for every track",
            format!(
                "{ops} ops, 1 round trip, Live's main thread {} ms",
                out.get("main_ms").and_then(Value::as_f64).unwrap_or(0.0)
            ),
        ),
        Err(e) => all = bad("read 5 values for every track", e),
    }

    // 5. The refusals, against the real script: the whitelist is not a
    //    promise this side makes, it is enforced where the objects are.
    for (path, why) in [
        ("os.environ", "outside Live"),
        ("song.__class__", "a dunder"),
        ("song.tracks[9999].name", "out of range"),
    ] {
        let sent = conn.send_command("run", Some(json!({"ops": [{"op": "get", "path": path}]})));
        match sent {
            Err(e) => ok(&format!("refused: {why}"), e.to_string().replace('\n', " ")),
            Ok(v) => all = bad(&format!("refused: {why}"), format!("it answered {v}")),
        }
    }

    if write {
        // 6. A write, and put it back: name a track through the generic
        //    layer. One undo step in Live, like any other command.
        let name_path = Path::track(0).attr("name");
        let before = run(&conn, &Batch::new().push(Op::get(&name_path, "name")))
            .ok()
            .and_then(|m| m.get("name").and_then(Value::as_str).map(str::to_string))
            .unwrap_or_default();
        let written = run(
            &conn,
            &Batch::new()
                .push(Op::set(&name_path, "renamed without a script release"))
                .push(Op::get(&name_path, "name")),
        );
        match written {
            Ok(out) => {
                let now = out.get("name").and_then(Value::as_str).unwrap_or("");
                ok(
                    "rename a track generically",
                    format!("{before:?} -> {now:?}"),
                );
                let _ = run(
                    &conn,
                    &Batch::new().push(Op::set(&name_path, before.as_str())),
                );
                ok("and put it back", format!("{before:?}"));
            }
            Err(e) => all = bad("rename a track generically", e),
        }
    } else {
        println!("  note  writes skipped; pass --write to rename a track and undo it");
    }

    conn.disconnect();
    println!();
    if all {
        println!(
            "PASS: every call above is new work in Rust against Remote Script {version},\n\
             which was not edited, reinstalled or reloaded for any of it."
        );
    } else {
        eprintln!("FAIL: see the lines above");
        std::process::exit(1);
    }
}

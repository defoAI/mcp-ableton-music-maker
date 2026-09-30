# How it is built

- **`src/`** is the server: tool bodies are plain functions, `fn(&LiveState, &Params) -> Result<String, String>`, so the whole suite runs against a fake Live. Compact note forms, the sound vocabulary, sections and songs, transitions, capture measurement and the library index are each a module of pure functions with their own tests.
- **The Remote Script** is two Python files, compatible with the Python Live bundles, embedded into the binary. `__init__.py` is the loader Live holds — the socket, the tick, the executor — and `body.py` is every handler. A new build hands Live the body and tells it to re-read it, so a fix lands with the transport running and the set unsaved; the loader is the one file whose change still asks for a restart, and the server says which case it is. It touches Live only from Live's main thread, in bounded slices, and reports what each command cost. The list of commands lives in the server, in one place; each half reads its own handlers back to say what it serves, the server checks that before every call, and a test fails the build if they ever disagree.
- **`app/`** is the Mac app. Its Rust core links the crate by path; the tap of Live's audio is one Objective-C file compiled by `cc`, with everything newer than macOS 12 weak-imported behind `@available`.

```bash
cargo test                                          # the server, against a fake Live
cargo clippy --all-targets -- -D warnings
scripts/render-readme-images.sh                     # the images on this page, from .github/readme/src
cd app/src-tauri && cargo test                      # add -- --ignored with Live open: two tests drive a real tap
cd app/src && node --test listen.test.mjs visual.test.mjs
cd app/src-tauri && cargo run --example listen_probe  # taps Live for a few seconds and prints what arrived
docker build --target test .                        # the same suite inside the image
```

CI is two jobs, both on macOS: the Rust gate, and the app with its verified disk image. A `v*` tag builds the signed, notarised one.

---

[← README](../README.md)

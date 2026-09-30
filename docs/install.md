# Install

Two pieces ship together: the server, `ableton-music-maker`, a single Rust binary that speaks the [Model Context Protocol](https://modelcontextprotocol.io) to your client; and the **AbletonMusicMaker** Remote Script, a control surface that runs inside Live because Live loads control surfaces only through its own Python. The binary carries the script, installs it, and keeps it current in place — a new server hands Live its new handlers while the transport runs.

```
Claude client ──stdio──▶ ableton-music-maker ──TCP 9877, loopback──▶ Live + AbletonMusicMaker
```

## With the Mac app

Installs the script, connects your client, and shows you whether the three are talking.

**[Download Ableton Music Maker for Mac](https://github.com/defoAI/mcp-ableton-music-maker/releases/latest/download/Ableton-Music-Maker-arm64.dmg)** — Apple Silicon, always the latest release. Open the disk image and drag the app to **Applications**.

> [!IMPORTANT]
> The Mac app is **not yet signed or notarised by Apple**, so macOS will say it is damaged or cannot be checked. After dragging it to **Applications**, run this once in Terminal:
>
> ```bash
> xattr -dr com.apple.quarantine "/Applications/Ableton Music Maker.app"
> ```


**Setup** walks four steps and checks each against what is really there: install into Live's User Library; restart Live and pick **AbletonMusicMaker** in a **Control Surface** slot under **Settings → Link, Tempo & MIDI**, Input and Output **None**; add the server to Claude Desktop (the app edits the config with a backup beside it) or copy the command for Claude Code and Cursor; run a test call.

<details>
<summary><b>From source</b> — or a build of <code>main</code></summary>

```bash
git clone https://github.com/defoAI/mcp-ableton-music-maker.git
cd mcp-ableton-music-maker/app && npm install && npm run dev      # Rust 1.85+, Xcode command line tools
```

Every push to `main` builds and verifies an Apple Silicon disk image, kept as the `mac-app` artifact of the [CI run](https://github.com/defoAI/mcp-ableton-music-maker/actions/workflows/ci.yml). It is ad-hoc signed: a Mac that downloads it refuses it until `xattr -dr com.apple.quarantine "/Applications/Ableton Music Maker.app"`. A release is the same image until the Developer ID certificate is in the release pipeline; then releases are signed and notarised and need none of this.
</details>

## The binary alone

```bash
cargo install --path . --locked
ableton-music-maker-install-script          # the Remote Script into Live's User Library, with a .bak of what was there
ableton-music-maker-install-script --reload # …and, after a rebuild, hand the new handlers to a running Live
claude mcp add AbletonMusicMaker ableton-music-maker
```

Restart Live and select the control surface as above. Claude Desktop takes `{"command": "ableton-music-maker"}` under `mcpServers.AbletonMusicMaker`; Cursor takes the same command under **Settings → MCP**. `ABLETON_HOST` and `ABLETON_PORT` override where Live is. Run one server at a time across all clients.

<details>
<summary><b>Docker</b> — built where it is used, not published</summary>

The image is hardened by contract and `docker/verify-image.sh` proves it against an image you built: distroless, non-root, read-only root filesystem, `/state` the only writable path, no upload code in the binary, under 50 MB.

```bash
docker compose build && docker/verify-image.sh mcp-ableton-music-maker:local
docker compose --profile install run --rm install-script
claude mcp add AbletonMusicMaker -- docker run --rm -i --read-only --security-opt no-new-privileges:true --cap-drop ALL -v ableton-music-maker-state:/state mcp-ableton-music-maker:local
```

For Claude Desktop, the same `docker run …` line becomes `command` and `args`. Never add `-t` (it breaks stdio) or `-p` (the container listens on nothing). Docker Desktop must be running whenever the client starts the server; opening this repository in Claude Code offers the container through [`.mcp.json`](../.mcp.json).
</details>

Something not connecting? [Troubleshooting](troubleshooting.md).

---

[← README](../README.md)

# flicker-net

The engine's client-side network seam — the crate that owns every conversation flicker
has with the outside world. Two things run through it today: an **anonymous release-update
check** against GitHub, and the **clay-chat client** that connects a scene to the
clay-engine chat server. It is also the declared home for the ecosystem work that has not
landed yet — account **auth / entitlements** and live **world-state sync** — so expect the
crate to grow those modules beside the two below rather than a new crate appearing.

Both live features share one idiom: a background OS thread owns a tokio runtime and does
the socket work, while the game loop stays fully synchronous and drains a plain channel
each frame. Nothing here blocks a frame, and no `async` leaks into a caller.

> Design of record — why it is shaped this way, decisions, history — lives in the project's
> MCP memory, not here. This file documents how to use the crate.

## Where it sits

- **Cluster:** `Alpha/crates/net/`. A leaf crate — nothing in the engine builds *on top of*
  it; it is the edge where bytes leave the machine.
- **Builds on:** `tokio` (a current-thread runtime, confined to the background threads this
  crate spawns), `reqwest` (the one HTTPS call in `update`), `serde` (deserializing the
  release JSON). *No engine crate is a runtime dependency* — see finding on unused deps.
- **Reached as `flicker::net`:** the `flicker` core crate re-exports this crate
  (`pub use flicker_net as net;`), so consumers call `flicker::net::update::…` and
  `flicker::net::chat::…` rather than depending on `flicker-net` directly.
- **Used by:**
  - `flicker-shell` (`src/shell.rs`) — calls `update::check_github_latest` at boot and turns
    the result into the main-menu "UPDATE AVAILABLE" chip (that chip's Model keys and route
    are owned by the shell — see `Alpha/crates/frontend/flicker-shell/src/shell.rs`; the
    crate has no README yet).
  - `flicker-pocclusters` (`src/lib.rs`) — owns a `chat::ChatClient` for the Cluster Editor's
    in-world chat panel (connect on scene enter, drop on exit).

### External endpoints and what crosses them

| Endpoint | Module | Transport | What is sent | What comes back |
|---|---|---|---|---|
| `https://api.github.com/repos/{owner}/{repo}/releases/latest` | `update` | HTTPS GET, anonymous (no token) | `owner`/`repo` in the URL path only (the shell passes `elide-us`/`flicker`); a `User-Agent: prism-alpha/<ver>` header. **No player data, in the URL or anywhere.** | `tag_name` + `html_url`, nothing else read |
| `127.0.0.1:6667` (clay-chatd) | `chat` | **raw TCP, plaintext** (the v0 line protocol) | your chosen nick, channel names, and message text — in cleartext | one text line per server event |

The chat address is **hard-coded** and the connection is **unencrypted** — both are v0-POC
facts a caller must know; see *Sharp edges*.

## Public API

Reached as `flicker::net::update::*` and `flicker::net::chat::*`.

### `update` — one-shot release check

| Item | What it is for | The one thing to know |
|---|---|---|
| `check_github_latest(owner, repo, current) -> Receiver<UpdateInfo>` | Ask GitHub whether a newer release than `current` exists. | Fire once at boot; poll the returned `std::sync::mpsc::Receiver` with `try_recv` per frame. It yields **at most one** `UpdateInfo`, and **only** if the latest tag parses to a version strictly newer than `current`. **Every** failure (offline, rate-limited, changed API shape, unparsable tag, unparsable `current`) is silent — the receiver simply never yields. |
| `struct UpdateInfo { version: String, url: String }` | The newer release: bare version (`"0.1.2"`) and its release-page URL. | `version` is normalized to `major.minor.patch` (any `v` prefix and `-pre`/`+build` suffix dropped). `url` is GitHub's `html_url` for the release. |

`current` accepts `0.1.2`, `v0.1.2`, `v0.1.2-alpha.1`, `v0.1.2+build7`; a pre-release/build
suffix is ignored for ordering. Four dotted parts, or anything non-numeric, parses to
nothing and disables the check (no panic, no chip).

### `chat` — clay-chat client

| Item | What it is for | The one thing to know |
|---|---|---|
| `ChatClient::connect(nick: String) -> ChatClient` | Open a session to clay-chatd and set your nick. | Returns **immediately**; the socket opens on a background thread. Watch for `ChatEvent::Connected` / `Disconnected` to learn the outcome. |
| `ChatClient::send(&self, cmd: ChatCommand)` | Queue one command to the server. | Non-blocking. A no-op (logged at `debug`) if the connection has already gone — it does **not** error or surface anything to you. |
| `ChatClient::try_recv(&self) -> Option<ChatEvent>` | Pop the next inbound event. | Call in a `while let Some(ev) = client.try_recv() { … }` loop each frame. `None` = nothing pending. |
| `Drop for ChatClient` | Disconnect. | Dropping the client sends `QUIT` and lets the background thread wind down on its own; it never joins/blocks the game loop. Own one per scene. |
| `enum ChatCommand` | The things you can tell the server. | Variants: `Join(ch)`, `Part(ch)`, `Nick(name)`, `Msg { channel, text }`, `Names(ch)`, `Quit`. Each is exactly one wire line. |
| `enum ChatEvent` | The things the server (or the client) tells you. | See the table below. |
| `chat::codec::encode(&ChatCommand) -> String` / `decode(&str) -> ChatEvent` | Pure, I/O-free translation to/from wire lines. | You rarely call these — `ChatClient` does. They exist isolated (no tokio) so the wire contract is exhaustively unit-testable; when the binary protocol replaces the line protocol, only `codec` changes. |

`ChatEvent` variants — most are decoded from one server line; `Connected`/`Disconnected`
are synthesized by the client for the net-level transitions the wire has no line for:

| Variant | Meaning |
|---|---|
| `Connected` | TCP is up (synthesized). |
| `Disconnected(Option<String>)` | Connection closed (synthesized); `Some(reason)` on error, `None` on clean EOF. **This is the loud channel for connection loss.** |
| `Chat { channel, from, text }` | A chat message. |
| `Joined { nick, channel }` / `Parted { nick, channel }` | A member joined / left. |
| `Renamed { old, new }` | A peer renamed (server broadcast). |
| `Names { channel, names }` | A channel roster (reply to `Names`). |
| `NickAck(String)` | **Your own** nick was accepted — the authoritative spelling. |
| `Channels(Vec<String>)` | The channel list (counts stripped). |
| `Notice(String)` | Any other server text (welcome / help / bye / unmatched `* …`). |
| `Error(String)` | A server error notice (wire `! …`). |
| `Pong(Option<String>)` | Keepalive reply. |

The **wire protocol itself** — the exact bytes of each line — is the clay-engine spec
(MCP `D9FE6CE0`, *clay-chat POC wire protocol v0*), not this README; `codec` is tested
line-for-line against it. That contract is owned by the server's repo, so treat MCP as its
source of truth.

## Interactions

- **Signals / intents:** **None.** flicker-net has no input surface. A *signal* (flicker's
  name for an abstract, device-independent input event — the vocabulary lives in
  [`flicker-input-core`](../../input/flicker-input-core/README.md)) is captured by the
  *scene* or *shell* that owns the UI; that consumer turns it into a `ChatCommand` or a menu
  route. This crate only moves bytes.
- **Model keys:** flicker-net publishes **none** itself. (The *Model* is the per-frame
  key→value table the engine hands the scene's Lua.) Its outputs are turned into Model keys
  by the consumer, not by this crate:
  - `update`: `flicker-shell` reads `UpdateInfo` and publishes `app_version` (text, `"vX.Y.Z"`)
    and `update_available` (bool) for `Main.scene.json` to bind, plus the route arm
    `open_update_page`. Those keys and that route are **owned by the shell**, documented there.
  - `chat`: `flicker-pocclusters` folds `ChatEvent`s into its own per-channel logs and rosters
    and renders them through its chat panel. Owned by that scene.
- **What it hands other crates:** a `Receiver<UpdateInfo>`; a `ChatClient` handle; the
  `ChatCommand` / `ChatEvent` value types; the `codec` free functions.
- **Threads:** each entry point (`check_github_latest`, `ChatClient::connect`) spawns **one**
  OS thread running a current-thread tokio runtime. Callers stay synchronous and poll a
  `std::sync::mpsc` receiver. `chat` additionally uses a tokio unbounded channel for the
  outbound side, whose `send` is non-async so the sync caller needs no runtime of its own.

## Gates

`cargo test -p flicker-net` — 8 tests, the drift gates for both modules:

| Test | Breaks if… |
|---|---|
| `update::versions_parse_and_order` | version parsing or ordering changes (v-prefix, pre-release/build suffixes, triple compare). |
| `update::bad_current_version_yields_nothing` | an unparsable `current` ever panics instead of silently disabling the check. |
| `codec::encode_exact_wire_strings` | an outbound command stops matching its exact wire line. |
| `codec::encode_sanitizes_nick_like_the_server` | nick sanitization (strip whitespace/`#`, truncate to 24) drifts from the server's. |
| `codec::decode_chat_line` | `#chan <from> text` parsing breaks (including messages that themselves contain `<`/`>`). |
| `codec::decode_system_lines_most_specific_first` | any `* …` / `!` / `PONG` line decodes to the wrong `ChatEvent`. |
| `codec::decode_ambiguous_notices_fall_through` | a notice containing "joined"/"left"/"is now known as" is mis-decoded as a real event instead of falling through to `Notice`. |
| `codec::decode_round_trips_a_sent_message_echo` | a sent `Msg` and its server echo stop agreeing. |

## Not here yet — the forward surface

The crate summary names capabilities that are **planned, not built**. They are spec-ward
(there is a ratified direction for each) — listed here so a caller knows the trajectory and
does not go looking for an API that has no code behind it:

- **World-state sync** ("state sync") — connecting a scene to the clay-engine world-state
  server (`clay-worldstated`) with a sim clock, prediction and reconciliation. Handoff spec
  in MCP (`A044483D`). **No module exists yet.**
- **Auth / entitlements** ("auth handshake") — browser OAuth, token storage, and which
  scenes/features an account unlocks. Ratified to arrive with the **launcher** + IdP work
  (`9F1288EC`), which may host it rather than this crate. **No module exists yet.**

## Sharp edges

- **The chat server address is hard-coded** to `127.0.0.1:6667`. There is no host
  discovery, config key, or environment override — pointing chat at any other server means
  recompiling. (v0-POC; host discovery is slated for the web/auth side.)
- **Chat is plaintext, unauthenticated TCP.** Your nick and message text travel in the
  clear. Harmless against localhost; a remote server would expose them until the binary
  protocol + auth land.
- **The update check is silent on every failure — by design.** An offline game must never
  nag, so there is no error surfaced to the UI. To diagnose a *missing* chip, read the
  `tracing` output from target `flicker_net::update`: `warn` (the running version didn't
  parse), `debug` (offline / unexpected response shape / unparsable tag), `info` (an update
  was found). No log line means the request itself hasn't returned.
- **`ChatClient::send` after a disconnect is a silent no-op** (logged at `debug`), not an
  error. Connection loss *is* surfaced loudly — as `ChatEvent::Disconnected(reason)` — so
  drive UI state from the event stream, not from whether `send` "worked".
- **Nick is optimistic.** `encode` sanitizes the nick you pass (strips whitespace and `#`,
  truncates to 24 chars) to match the server; the authoritative spelling comes back as
  `ChatEvent::NickAck`. If sanitizing changed your nick, what you typed and what you *are*
  differ until that ack.
- **`ChatEvent::Channels` and `ChatEvent::Pong` are decoded but unused by the one shipped
  consumer** (`flicker-pocclusters` ignores them). They are available to the next consumer,
  not dead — the codec produces them from real server lines.

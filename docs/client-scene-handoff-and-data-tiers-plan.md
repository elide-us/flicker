# Handoff — unified scene handoff, thin clients, and the data-tier model

Written 2026-07-19 at a context boundary. **No code was written for this plan yet** —
this is the run-book for the next session. Read the §"Rules" links first, then §1
(the concrete task), then §2 (the architecture it forces).

The UI work that precedes this is **done + verified**: the Prism vector-UI primitive,
the menu / "Sanctum" pause / Settings workbench redesign, the loading screen, the scene
depth-band fix, and full 2D sRGB colour-correctness. See memory `ui-vector-primitives`.

---

## 0. Rules & guidance to re-read before starting (the user insists)
- **CLAUDE.md** — §1 (ecosystem: ClayEngine + the live **IOCP MMO servers**; flicker is the
  **client** POC), §2 (load-bearing invariants), §6 (deferred ideas inventory), §8
  (conventions: stay out of git, user verifies the app, scope discipline, thin slices,
  **big work → `docs/*-handoff.md`**), §9 (memory + MCP).
- **Behavioral laws** (memory bank): `do-not-reinvent-existing-systems` (enhance, don't
  fork), `less-code-every-calculation-counts`, `clarify-intent-before-building`,
  `canon-values-align-everywhere`, `user-verifies-app-themselves`.
- **`memory_coderules` + `memory_search`** (MCP) before creating any crate/module — the
  grand-vision scene system may already cover part of this: `docs/scene-system-and-content-pipeline-spec.md`
  and memory `scene-system-content-pipeline`. **Do NOT fork it — extend it.**
- Relevant memories: `flicker-shell-service`, `crate-cluster-taxonomy`,
  `promote-examples-to-shell-clients`, `clicktrainer-2d-blend`, `paperdoll-inventory-lua-ui`,
  `ui-vector-primitives`, `two-server-batch-model`, `conservation-and-voxel-container`.

---

## 1. The immediate task (what triggered this handoff)

**Add an additional button to the main menu — in EVERY client — that launches the Click
Trainer scene.** The **START (top) button stays** as "launch this client's own
`game_scene`" (the factory each client passes to `flicker_shell::run`). The Click-Trainer
button is a *second, shared* launch entry.

Because the menu is unified across all 10 clients (the shared `flicker-shell` embeds
`modal.lua` + `ui_elements.json`), "apply to all instances" means the shell itself must be
able to construct a Click-Trainer scene. That forces two things:

1. **Re-home the Click-Trainer scene into a LIBRARY crate.** Today it is a binary-only
   `ClickTrainer` struct in `Alpha/flicker-clicktrainer/src/main.rs` (a `flicker::scene::Scene`,
   ~284 lines: a 2D sprite target + `draw_text` HUD + Esc→PauseScene). Move the *scene* into a
   lib the shell (or a shared scene registry) can call; the binary shrinks to a thin `main()`.
2. **Generalize the menu launch into a standard "start-handoff to any scene" mechanism** —
   not a hardcoded `game_scene` + a hardcoded Click-Trainer button, but a small registry of
   launchable scenes the menu renders as buttons. START = the client's game scene; extra
   entries = shared/registered scenes (Click Trainer being the first). See §2.2.

### While re-homing, finish the ORIGINAL blend goal for the Click Trainer
The Click Trainer is "our most basic 2D testing application… demonstrate the main required
functionality." Port its HUD to the **vector UI** and prove **click routing** — the exact
pattern paperdoll already uses (do NOT reinvent it; mirror it):
- **Scene** (`Alpha/flicker-paperdoll/src/main.rs:1680-1713`): run the Lua HUD's `update`,
  read `over_hud = res.is_on("hud_hit")`, and gate the game click:
  `if input.mouse_left_pressed && !over_hud { …game hit-test… }`. The camera/scene only get
  the mouse when the HUD didn't claim it.
- **hud.lua** (`Alpha/flicker-paperdoll/scripts/hud.lua`): `point_in` over each panel rect →
  return `hud_hit=true`; a slider drag in progress also holds the mouse.
- So the Click Trainer HUD = a vector **panel** (stats: hits/misses/accuracy + reaction
  last/best/avg) + a **RESET** button (a UI click that resets, proving a click over the UI is
  absorbed, not scored as a miss) + the "Esc: menu" hint; reports `hud_hit` + `reset`.
- Wiring pattern (mirror paperdoll / flicker-world `scene.rs:49-51,442-446`):
  `HUD_SCRIPT_PATH = concat!(env!("CARGO_MANIFEST_DIR"), "/scripts/hud.lua")`,
  `HUD_UI_ELEMENTS = concat!(…, "/../content/resources/ui_elements.json")` (or the re-homed
  lib's own path), `ScriptHost::from_file` → `load_ui_json` → `load_widgets`; per frame
  `set_model` → `update` → route → `render_hud(renderer, &cmds, white, &[])`. Add a
  `clicktrainer` section to `ui_elements.json` (tokens-only colours; a `hud_hit` panel).
- **Remember the sRGB + layering invariants** from `ui-vector-primitives`: panels/rects now
  sRGB-decode; scenes get a depth **band** (`SCENE_LAYER_STRIDE=100`) — a HUD over a 2D game
  is fine, but any sub-layering must stay < 100.

### Per-app save state / settings
`ShellConfig.settings_dir` already gives each client its own `settings.json` (video/audio/
input) — this is the **player-config tier** (§2.3, tier 3). Formalise it while here: one clear
per-client location for the player's local config + keybinds, kept distinct from content and
from server data. Decide (with the user) whether keybind persistence (still BLOCKED on
`InputMap` serde — memory note) gets unblocked as part of this.

---

## 2. The architecture this forces (the big picture)

> **This is the CLIENT of a TRUE MMO, not the game engine.** The authority is a Windows
> **IOCP** server running the real physics simulation for **thousands** of players (CLAUDE.md
> §1: "flicker is part of ClayEngine, alongside … the live MMO servers"). "TRUE MMO — not 100
> players × 1000 instances." **The client is in the hands of the enemy — virtually nothing
> here is trusted.** The client is a thin renderer + input toolkit for players; the server
> owns and simulates the game.

### 2.1 Thin client / library generalization (the standing direction)
- **Clients are thin `main()`s.** Everything reusable lives in libraries; a client just wires
  a `ShellConfig` and hands off scenes. (Matches `promote-examples-to-shell-clients` +
  `flicker-shell-service` + `crate-cluster-taxonomy`: `Alpha/crates/<cluster>/<flicker-crate>`.)
- **Unified scene management + a standard start-handoff to any scene.** Generalize logic and
  classes into libs; don't duplicate per client. The Scene System is already a
  design-of-record (`docs/scene-system-and-content-pipeline-spec.md`, memory
  `scene-system-content-pipeline`: "Scene System as a CORE ENGINE capability") — **extend
  that**, don't build a parallel one.

### 2.2 The scene-handoff mechanism (design sketch — confirm with the user)
Options to weigh next session (pick one WITH the user; do not just build):
- **A. `ShellConfig` scene registry.** `ShellConfig { game_scene, extra_scenes: Vec<{label,
  factory}>, settings_dir }`. START launches `game_scene`; each `extra_scene` becomes a menu
  button. The shell provides a **default** `extra_scenes` containing the Click Trainer (so it
  appears in every client) via a dependency on the re-homed click-trainer lib. Clients can
  append their own.
- **B. Shell built-in "diagnostics/test" scenes.** The shell owns a small set of built-in
  scenes (Click Trainer = the 2D test) always present in the menu, plus the client's
  `game_scene` as START. Simpler menu contract; shell depends on the click-trainer lib.
- Either way the menu items stop being purely static in `ui_elements.json`: the shell must
  publish the launch entries (label + a stable action id) into the `Model`, and `modal.lua`
  renders a button per entry (the `start` action → `game_scene`; each entry id → its factory).
  Keep the **strict data-only Lua boundary** (label + id are text/number only — no handles).
- This is the "standard start handoff to any scene" the user asked for: one code path that
  the menu, and later any in-game portal/return, uses to swap the active scene.

### 2.3 Data tiers — LOAD-BEARING (record in memory)
The user's classification. "Any data here now is either content or data — if **data**, assume
it will **NOT be stored locally** in the end." Four tiers:

1. **Source data** — authoring artifacts: **FBX** (Meshy exports, `Alpha/content/source/**`).
   Never shipped to the client; the toolchain bakes them into tier 2.
2. **Runtime local CONTENT** — the shippable, derived, mostly read-only client payload:
   **meshes, rigs, materials, animations, Lua UI elements, 2D content (sprites/textures)**.
   Local to the client (part of the install/patch stream). This is "content."
3. **Runtime PLAYER data** — the player's local, writable config: **`settings.json`
   (video/audio/input), keybinds, and the client's Lua-script/UI customization**. Local &
   per-player. ("A client's configuration and Lua scripts are local to the client.")
4. **SERVER data** — the true game runtime: **character info, inventory, gameplay/simulation
   state**. Lives on the **IOCP server**; the client **renders** it but does not own, trust,
   or persist it. **Not stored locally in the end.**

**Consequences to honour going forward:**
- Anything that is *game state* (a character's equipped items, stats, position, the paperdoll's
  equipped/inventory selection) is **tier 4** — design it as server-authoritative and
  transient on the client, even where a POC currently fakes it locally. The MESHES/RIGS/MATS
  for those characters are **tier 2 content**; the *equip/inventory state* is **tier 4**.
- The client trusts nothing for gameplay; `flicker-net` (client-side transport/state-sync
  stub — CLAUDE.md §4) is where tier-4 data enters, from the *separate* server repos.
- Keep clients thin: local install = content + player config only. No server data at rest.

---

## 3. Suggested step order for the next session
1. Re-read §0 rules; `memory_search`/`memory_coderules` for "scene", "shell", "click"; read
   `docs/scene-system-and-content-pipeline-spec.md` to align (extend, not fork).
2. **Confirm the scene-handoff design (§2.2 A vs B) WITH the user before building** — this is
   a shared-shell API change (`clarify-intent-before-building`).
3. Re-home `ClickTrainer` → a lib crate (pick the cluster per `crate-cluster-taxonomy`);
   thin the binary to `main()`.
4. Blend the Click Trainer HUD (vector panel + RESET, `hud_hit` routing) — mirror paperdoll.
5. Generalize the menu launch (registry/built-ins) + `modal.lua` renders launch buttons from
   the Model; START → `game_scene`, Click-Trainer entry → its factory. Unify across clients.
6. Formalise the per-client player-config location (tier 3).
7. Build + `cargo test -p flicker-shell` (extend the smoke test to cover the new menu entries)
   + `cargo build --workspace`; user verifies visually.
8. Update memory + this doc.

## 4. References
- Task origin: `Alpha/flicker-clicktrainer/src/main.rs` (the scene to re-home),
  `Alpha/flicker-paperdoll/src/main.rs` + `scripts/hud.lua` (the click-routing reference),
  `Alpha/crates/frontend/flicker-shell/src/shell.rs` (menu / `ShellConfig` / `ModalUi`),
  `Alpha/content/scripts/modal.lua`, `Alpha/content/resources/ui_elements.json`.
- Design-of-record: `docs/scene-system-and-content-pipeline-spec.md`.
- Memory: `ui-vector-primitives`, `client-data-tiers`, `flicker-shell-service`,
  `crate-cluster-taxonomy`, `scene-system-content-pipeline`, `clicktrainer-2d-blend`,
  `paperdoll-inventory-lua-ui`, `promote-examples-to-shell-clients`.

# flicker combat / animation state machine — design handoff

**Purpose.** This is the deep-work brief for the **animation state machine + content-pack
+ souls-like combat metadata** layer, to be picked up in a fresh session. It captures
the architecture worked out with the user (Aaron / Elideus) on 2026-07-05 so the design
is settled *before* building. The visual/mesh/animation *primitives* are done (see
`docs/flicker-animation-handoff.md`); this doc is the **combat spine** that sits on top.

Honour the repo conventions (CLAUDE.md §8): stay out of git, the user verifies the
window, thin slices, don't author roadmaps beyond what's here, `docs/*-handoff.md` +
MCP memory are the durable record.

---

## 1. The foundation that already exists (build ON this, don't redo it)

`examples/flicker-animation` (POC) + the `flicker.rig` format (converter in
`ClayEngine\FbxImport\`) already give us the **sampling half** of the animation system:

- **CPU-authoritative pose** — clips baked to fixed-rate integer-tick keyframes; sampled
  per tick to per-bone local TRS → global transforms (forward kinematics). This IS the
  fixed-tick combat clock substrate. (`pose.rs`.)
- **Name-based retargeting** — clip tracks resolve to the rig skeleton **by bone name**,
  not index. This is exactly what content-pack layering needs (a weapon pack's clips bind
  to the shared skeleton by name). (`format.rs::load_dir`.)
- **CPU skinning + per-material submeshes + textures** (+ PBR maps in progress).
- **Weapon socket attach** — the rig has `Weapon_R`/`Weapon_L` bones; a static prop
  (katana) is drawn rigid at `world × globals[Weapon_R]`. Equip/unequip toggle (`K`).
- **The clip set we have** (13, in-place, 60 Hz): Idle, Walk, Run, Crouch, Crouch_Move_F/L/R,
  Jump_Start/Loop/End, Attack_1, Dame_01 (hit-react), Death_01 — enough for a first state
  machine covering locomotion / crouch / jump / attack / hit / death. The user has MANY more
  to extract (climb, more strafes, more attack/death/damage, root-motion sets).

What is NOT built: the state machine, blending, the TAE event timeline, hitbox/hurtbox
capsules, abilities, stamina/poise, the pack manifest, movement in the client.

---

## 2. Content-pack model (the user's DM-kit framing)

The `flicker.rig` file (skeleton | mesh | clips) is the **primitive/atom**. A **content
pack** is the bundle a player receives:

- **Base character pack** — the 94-bone rig, body mesh + **skin variants** (Color_1/2/3 —
  same mesh + skeleton, swappable albedo/map set = a cosmetic content variant; the maps
  for these are already on disk), the shared **locomotion** clip set, and the base
  movement state machine.
- **Weapon pack** — the weapon mesh(es) + socket + grip offset, a weapon-specific
  **animation set** (draw/sheathe, the attack moveset, blocks, specials), the **ability
  definitions**, and the **combat metadata** hanging off those clips. It **layers onto the
  shared skeleton by bone name** (retargets — the foundation is already name-based).

Design law: **"get a weapon → get its skeletal animations + abilities."** Katana pack,
spear pack, fists pack — each a self-contained kit that extends the base rig's moveset.
This is the shape the format + loader must grow toward.

---

## 3. State machine + the TAE event timeline = the combat authority

Aligns with canon (MCP memory `flicker` 386AC689 / 04BE5862): **TAE-style event timeline
as the authority, CPU-authoritative logical pose for hitboxes, fixed-tick combat clock.**

- **State machine** = a graph of states (each state = a clip, or a blend). Transitions are
  gated by **input + conditions** (grounded, stamina, current event windows, hit flags).
- **Per-clip event timeline (TAE)** = the authority for *what happens when*, all in **ticks**
  (the same clock the clips are baked to). This is where the souls-like combat data lives —
  it is NOT ad-hoc code, it is authored metadata on the clip.
- The tick loop: read input + state → advance the active state's play-head → fire the
  event-timeline events for the ticks crossed → apply transitions → compute the CPU pose +
  hitbox transforms. GPU only does palette skinning (deferred; the CPU pose is truth).

Build the timeline as the spine from day one (the user's canon points hard at TAE-as-
authority) — do NOT build a plain enum state machine and retrofit events later.

---

## 4. Souls-like combat metadata (the requirements → the event/state schema)

The metadata the timeline + state graph must express (all windows in ticks):

- **Attacks:** startup / active / recovery; **hitbox-active** intervals (capsules bound to
  bones with per-window activation); damage / poise-damage / stamina cost; **cancel/combo
  windows** (which states this clip can be cancelled *into*, and when — the whole feel of
  souls combos); **hyper-armor / poise** windows.
- **Movement:** dodge/roll with **i-frame** windows; sprint; backstep; stamina drain;
  in-place-vs-root-motion authority per state (see decision 1).
- **Defense:** block / guard windows; **parry** windows; guard-break.
- **Hit reactions:** `Dame` (hitstun/stagger), poise-break, knockdown, **death**; the
  transitions into them from any state on a hit event.
- **Targeting:** lock-on / directional attacks (later).
- **Non-combat events:** footstep/SFX, weapon-trail on/off, the **equip/pickup swap**
  (weapon appears in hand on a draw event; the ground pickup → to-hand → despawn demo the
  user described lives here), VFX hooks.

**Hitbox/hurtbox layer:** capsules bound to bones (the CPU-authoritative pose already gives
bone globals every tick), activated by the event timeline's hitbox windows. This is the
"logical pose on CPU for hitboxes" from canon.

---

## 5. Format contract evolution

- Keep `flicker.rig` (per-file: skeleton | mesh | clips) as the FBX-derived, purely
  **mechanical** atom — do not stuff combat data into it.
- Add a **pack manifest** that references the rig + meshes + skin/map variants + weapon(s)
  and adds a **combat** section: the state graph, per-clip event timelines, ability defs,
  hitbox bindings. This metadata is **authored by us** (UE won't export TAE tracks) — small,
  diff-able JSON, hand/tool-authored per pack. (Decision 2: keep it a separate file per
  pack, not inside the rig JSON.)

---

## 6. Open decisions to settle at the top of the deep session

These were surfaced with the user; his lean noted, but CONFIRM before building:

1. **Locomotion model** — in-place clips + **capsule-driven** translation as the default,
   root-motion as opt-in per state (climb, some specials)? (In-place set fits the former;
   the user has root-motion sets too.) *Lean: yes, capsule default + root-motion opt-in.*
2. **Metadata home** — combat data (event timelines + state graph) as a **separate authored
   file per pack**, keeping `flicker.rig` mechanical? *Lean: yes, separate.*
3. **Crate extraction** — extract the runtime (`format`/`pose`/`skin` + the new state
   machine) into **`Alpha/flicker-skeletal`** now, so `flicker-csg` consumes a crate, not
   example code? (An `Alpha/` crates dir exists — memory 50EA9C0F.) *Lean: yes, extract.*
4. **Blending** — hard-cut transitions first, add crossfade blending as a later slice, or
   blend from the start? *Lean: hard-cut first, blend soon after (combat feel needs it).*
5. **BRDF/visual** — orthogonal; being handled as the PBR-maps polish this session.

---

## 7. Suggested build slices (after decisions land)

Thin, verify each; the user drives. Roughly:
1. Extract `Alpha/flicker-skeletal` (rig/pose/skin) from the example — clean crate seam.
2. Enum-free **state machine core** + the **TAE event-timeline** data model (authored JSON
   schema) + the tick loop; drive the existing viewer from it (Idle↔Walk↔Run↔Jump by input).
3. **Combat states** — Attack (startup/active/recovery), cancel windows, `Dame` hit-react,
   Death; author the katana pack's first moveset metadata.
4. **Hitbox/hurtbox capsules** bound to bones + activated by hitbox windows; a debug draw.
5. **Weapon-pack loader** — layer a weapon pack (mesh + socket + clips + metadata) onto the
   base rig by name; the **equip/pickup** state transition (ground weapon → collision → in
   hand → despawn; the wine bottle is imported as a second pickup test prop).
6. **Into `flicker-csg`** — capsule + gravity/ground, input → state machine → movement
   (attack LMB, crouch C, jump Space, strafes), CPU pose → hitboxes. This is where it
   graduates from viewer POC to the alpha client.

---

## 8. References

- `docs/flicker-animation-handoff.md` — the primitives (rig format, pose, skin, textures,
  PBR, submeshes, prop attach). Read it first.
- MCP memory (`flicker`): spec `1E75AEBA` (format contract), decision `04BE5862`
  (viewer/pipeline), invariant `F1F14C20` (voxel UV/normal constraint), decision `386AC689`
  (granny-tier / TAE-timeline / CPU-pose canon), decision `50EA9C0F` (Alpha/ dir + this
  direction). A new `spec` entry for THIS design should be stored when the deep session
  confirms the decisions.

---

## 9. Implementation log — Slice 1 LANDED (2026-07-05)

The state-machine core + TAE event timeline + tick loop is built and driving the viewer.
Everything lives **inside `examples/flicker-animation`** (see the branch note below), fully
additive to the existing POC.

**Decisions locked at the top of this session (the §6 open questions):**
1. **Locomotion** — in-place + capsule-driven default; root-motion is a per-state
   `root_motion` flag in the schema (recorded, not yet acted on). ✅ as leaned.
2. **Metadata home** — a **separate authored `flicker.pack` JSON**; `flicker.rig` stays
   mechanical. ✅ as leaned. (`assets/Katanami.pack.json`.)
3. **Crate extraction** — **DEFERRED, not as leaned.** `Alpha/flicker-csg` lives only on
   the (un-pushed) `macbook` branch, which *adds* an `Alpha/` tree; this box is branch
   `surface` with the renderer/animation work and **no `Alpha/`**. Extracting
   `Alpha/flicker-skeletal` here now would collide head-on with the macbook branch at merge
   time. So the state machine is built in the example with **cleanly separable modules**
   (`state.rs` has zero deps on the viewer); extraction to `Alpha/flicker-skeletal` becomes
   a move-after-merge, once the branches are reconciled on GitHub.
4. **Blending** — **hard-cut** transitions first (reset play-head + swap clip). Crossfade
   is the next slice. ✅ as leaned.

**What's built.**
- **`src/state.rs`** — the runtime. Authored wire types (`PackFile` / `StateMachineDef` /
  `StateDef` / `TransitionDef` / `TickWindow` / `Trigger` / `EventDef` / `EventKind`) +
  the resolved `StateMachine`. Clips referenced by **name**, resolved to indices at
  `StateMachine::build` (mirroring the rig loader's bone-name resolution); unresolved
  clip/state names become `warnings()`, not panics (a missing clip holds the rest pose).
  Advances on a **fixed 60 Hz tick** (`tick()` = the atomic combat-clock step; `advance()`
  accumulates a frame's `dt` into whole ticks, capped at 8/frame). Per tick: advance the
  play-head → fire the current state's timeline events for the crossed tick → evaluate
  transitions (**any-state edges first**, then per-state by priority, then `next`
  auto-advance on `clip_done`) → hard-cut on a match. Reports fired one-shots + the windows
  open at the settled tick.
- **TAE event timeline** — `EventKind` covers the souls-like vocabulary (Footstep,
  HitboxActive, Iframe, CancelWindow, Parry, Sfx, Equip, WeaponTrail). Point events fire
  on their tick; window events (`end` set) are reported active while the head is inside.
  **The runtime FIRES/REPORTS events only** — acting on them (hitbox capsules, i-frame
  invulnerability) is the next slice, exactly as designed.
- **`assets/Katanami.pack.json`** — the authored base graph: Idle ⇄ Walk ⇄ Run,
  Crouch ⇄ Crouch_Move, Jump_Start → Jump_Loop → Jump_End → Idle, Attack_1 with a
  **hitbox-active window [15,30]** on `Weapon_R`, a **cancel/combo window [40,55]** (press
  attack inside it to re-enter Attack_1), footstep events on the locomotion clips, and
  **any-state** Hit → Dame_01 / Die → Death_01. Clip names are the real stems
  (`Idle_nonWeapon` / `Walk_nonWeapon` / `Run_nonWeapon` / …); window ticks are within each
  clip's `duration_ticks`.
- **Viewer integration (`main.rs`)** — a `ViewMode { Graph, Manual }`. **Graph** (default
  when the pack loads) lets the state machine own clip + tick; gameplay input drives it:
  `W` move · `Shift` run · `C` crouch · `Space` jump · `F` attack · `H` hit (debug) ·
  `X` die (debug) · `R` reset. **Manual** is the original clip browser (Space play/pause,
  ↑/↓ clip, ←/→ step). `G` toggles modes. The HUD shows the mode, current state, clip/tick,
  the TAE windows open **now**, and a ring of recently-fired events.
- **Tests** — 8 headless `state` tests (initial state, locomotion move/stop, `clip_done`
  auto-advance, single-fire timeline event, hitbox window active, cancel-window gating,
  any-state hit interrupt, missing-clip warning). `cargo build/clippy --all-targets/test`
  all clean for `-p flicker-animation` (11 tests total with the pre-existing 3).

**Branch / merge note (load-bearing for the next session).** This work is on branch
`surface` (the renderer/animation branch). `Alpha/flicker-csg` exists only on the
un-pushed `macbook` branch. The two diverge and must be merged on GitHub by the user; the
state-machine work was deliberately kept **example-local and additive** (no new `Alpha/`
tree, no workspace-member changes) to keep that merge small. **After the merge**, the
`Alpha/flicker-skeletal` extraction (Slice 1 of §7's *original* plan) can proceed, pulling
`format`/`pose`/`skin`/`state` out of the example as a real crate `flicker-csg` consumes.

**Still not built (unchanged from §4/§7):** hitbox/hurtbox capsule binding + debug draw
(acting on HitboxActive windows), crossfade blending, stamina/poise/i-frame *enforcement*,
the weapon-pack loader + equip/pickup transition, and moving the machine into the client
(`flicker-csg`) with capsule + gravity. The `Crouch_Move_L/R`, more attacks, and
root-motion sets are additional clips + graph edges when the user extracts them.

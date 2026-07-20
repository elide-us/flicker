# Scene System + Content Pipeline — Grand-Vision Spec (FDD)

**Design-of-record handoff — 2026-07-19.** Author: Aaron (Elideus), captured with Claude.

This is a **design spec, not landed code** — it defines *what to build and why*, mapped onto
what already exists, so implementation later is assembly (extend proven code) rather than a
rewrite. It unifies two things Aaron's direction made one: a **core-engine Scene System**
(what a "scene" *is*, formalized) and the **content-processing pipeline** that turns
generated art into engine-ready assets. The pipeline is a **factory** — the world needs
*thousands* of animations and creatures — not a one-mesh tool.

**Read first:** `CLAUDE.md` §1–§2, `docs/animation-system-rebuild-spec.md` (the rig/anim
cleanup this dovetails with — esp. D.6/D.7), `Alpha/content/README.md` (the content-tree +
naming standard), memories `animation-system-rebuild`, `multibody-rig-retarget`,
`flicker-lua-ui-system`, `two-server-batch-model`.

**Scope boundary (locked with Aaron):** this doc **specifies** the buildable core — the
Scene System, the Translation Toolkit, the Material Composer, and the Content Pipeline
Editor — and only **frames** the game vision (two play modes, one MMO world) as context the
architecture must serve. It is **not** a roadmap; sequencing/prioritisation are Aaron's.

**Decisions — all core forks resolved by Aaron (2026-07-19):** **D1** dedicated crate ·
**D3** sidecar-delta correction · **D12** the Dungeon-Maker boss-kit framing (§4). Only outstanding
external input: the "4-view + stage-panel" mockup, if it exists in a design tool.

---

## 0. Vision-context — canon-cited; design-intent vs Prism canon separated

> This section **frames**; it does not specify gameplay. Where Aaron's stated direction goes
> beyond recorded Prism canon, it is flagged ⚠ so canon is never silently overwritten.

- **Two populations, one world (CANON — Book II §Design Philosophy).** "Dungeon Makers and
  adventurers" who "never directly interact"; the shared **world is a single real-time
  simulation.** Adventurer = "classic MMO" / "Dark Souls action combat at EverQuest scale"
  (Book I). Dungeon Maker = a **fixed-camera, voxel-slicing RTS** / base-builder /
  tower-defense (Book II).
- **Shared voxel-MMO persistent world (CANON — `CLAUDE.md` §1 + memory
  `two-server-batch-model`).** Offline bake → two batch servers: **Generation** (materialises
  voxel clusters from the hex aggregate near players) and **Simulation** (batch-evolves the
  aggregate, reintegrates player edits conserved, "never a fixed map"), at 1:1 voxel fidelity.
- **⚠ Design intent — not yet Book II canon (flagged for Aaron's ruling):**
  - **One client hosts both play modes.** Canon guarantees one *world* + two *populations*;
    "one client" is Aaron's direction. Adopted here as intent.
  - **Adventurer = 1st/3rd-person.** Aaron's gloss; canon says "MMO/Souls combat" without
    naming a camera. The DM view stays **fixed-camera** (canon).
- **⚠ Dungeon-Maker composition — resolved by Aaron (2026-07-19): an EXTENSION over canon,
  not a conflict (D12).** Book II canon holds: a boss's **creature** is a **recruited Bestiary
  creature placed in a designer-made, tier-scaled boss room** — the free-form creature composer
  "**is not the model**"; the Maker authors *which* creature (biome-recruited,
  Monster-Mastery-gated), *where*, at *what tier*, and *how* the room is built
  (`Alpha/design/Prism/BookII.md:177,179`). **The extension:** that boss room is a **"Design
  Room"** where the Maker also composes the boss's **presentation** from **unlockable kits —
  animation packs, skins, outfit sets, meshes** — surfaced by the Maker's **tech tree** (funded
  by experience earned when adventurers defeat the dungeon, `BookII.md:78,99`), gated by the
  Maker's **talents + the dungeon's biome**. **Unlocking a tier's boss kit is the gate to
  advance the dungeon to the next tier.** A learned **biome boss kit** is reusable across that
  Maker's dungeons in the same biome. This composes *presentation* over the recruited creature —
  **never** the stat block. *(The biome-reuse rule is fuzzy pending dynamic biome detection from
  worldgen, and is stricter than canon's learn-gated/use-free rule at `BookII.md:132` — treat as
  unsettled; a book-side capture is a follow-up. The ≈2000/1000-pt budget + "topology-agnostic
  rig" are engineering guardrails from `animation-system-rebuild-spec.md` §F, not lore.)*
- **Why this frames the build:** the **Scene System serves both modes** (Adventurer world +
  minimap + paperdoll panels; DM fixed-camera voxel-slice view; dev tooling views), and the
  **pipeline feeds two consumers: dev creature-assembly → the Bestiary** (the recruited base
  creatures) **and the Dungeon Maker's unlockable boss kits** (animation packs / skins / sets /
  meshes, surfaced as tech-tree unlocks). "Factory-efficient" is load-bearing precisely because
  the world needs thousands of clips, creatures, and biome×tier boss kits.

---

## 1. Core principle — our internal format is the consistent TARGET

The tool is fundamentally a **hub-and-spoke translation system**. Traditional modeling
pipelines are complex for historical reasons; since we **know our target** (a
well-standardised internal format) and the source reckonings (FBX, BVH, Meshy, various
axis/pose conventions) are established, **translation is mechanical**.

> **{FBX, BVH, Meshy, …} × {mesh, animation, material} → OUR format.**
> One consistent target — `flicker.rig` / `Clip` / `.pack` / `materials.json` + the
> content-tree standard — reached by a toolkit of small mechanical translators, one per
> source-reckoning × kind.

Existing translators are already spokes: `retarget_bvh.py` (BVH→clip), `skin_outfit.py`
(fit→skinned+cloth), `convert_meshy_prop.py` (FBX→prop). The declared seam is
`flicker-skeletal::format` — everything downstream consumes `RigFile` / `Mesh` / `Skeleton`.

---

## 2. The buildable core (FDD feature layers)

Features are FDD-phrased and tagged **[reuse]** (extend existing code, anchor given) /
**[new]** (build), and **[near]** / **[future]**.

### Layer A — Scene System (core ENGINE capability)

Today a `Scene` (`crates/flicker-scene/src/lib.rs:42`) owns **nothing but five methods**
(`enter`/`update`/`render`/`exit`/`is_overlay`) over a shared `&mut Renderer` that exposes
**one** active camera. The two-camera "PiP" in `flicker-packeditor` is a hand-rolled
rect+sprite in a non-Scene `impl App` (`main.rs:943`). Formalize the Scene:

- **[new][near] Give a Scene a *stage* + named *Views*.** A **Stage** = world + lighting; a
  **View** = a named camera (static or interactive) on that stage. Aaron's "static camera
  locations on a predetermined stage" already has a home — `flicker-flight`'s
  `OrbitPose`/`Flight`/`pose_at` (`Alpha/crates/animation/flicker-flight/src/lib.rs:53,101,187`)
  emits exactly named static camera poses over a target.
- **[new][near] Add a *Panel* / multi-surface layout**, and make **View → RenderTarget →
  Panel a first-class, repeatable binding.** RTT exists
  (`create_render_target`/`render_to_texture`/`target_texture`,
  `crates/flicker-render/src/renderer.rs:812,876,859`) but is used once, by hand. Constraint
  to honor: RTT passes reset the per-frame draw queues → all panel passes run **before** the
  main-frame view.
- **[reuse][near] Compose overlays** (HUD, stage panel) on the existing overlay stack +
  `set_layer` (`SceneManager`, `lib.rs:82,161`). Panels/views live *within* one Scene — this
  is **not** "a scene that renders scenes."
- **[new][near] Promote the camera/View abstraction into core.** `OrbitCam` is **copy-pasted
  three times** (solarbirth / flicker-world / poc-chemistry); a core `Camera`/`View`/`Panel`
  ends that duplication (law `less-code-every-calculation-counts`).
- **[mixed][near] Per-View render effects.** Reuse **wireframe** (`mesh.rs:55`), **tint**
  (`mesh.rs:58`), **transparency/alpha** (`pipeline_mesh.rs:245`). **Build outline /
  silhouette / x-ray / mesh-shadow** — all ABSENT today, each a new shader pass. A "view" =
  **(content selection) × (effect)**; the editor's *slot-isolation* and *skin-outline* are
  just view configs, not bespoke systems.
- **[new][near] Add an orthographic projection** to `Camera::projection` (`mesh.rs:97`,
  perspective-only) — **required**, so front/side/top measure limb angle without perspective
  distortion (the precondition for fit/correction).
- **Serves:** Adventurer HUD (world + minimap + paperdoll panels), the editor
  (front/side/top/perspective + stage panel), the DM fixed-camera voxel-slice view, and dev
  tooling. This is why it is engine-core, not editor-local.

### Layer B — Translation Toolkit (factory-efficient)

- **[reuse][near] The mechanical translators** (§1): `retarget_bvh` / `skin_outfit` /
  `convert_meshy_prop`, all → our format.
- **[new][near] Make them factory-efficient (batch/throughput).** The translators are **all
  serial today** (one-item `for` loops) — that IS the throughput gap. The batch engine
  already exists and is the reuse target:
  - `flicker-worker::WorkerPool` — generic task-agnostic closure pool
    (`Alpha/crates/core/flicker-worker/src/lib.rs:91`).
  - The chemistry **`Scheduler`** pattern — seeded-deterministic, chunked `sweep_cells`,
    completion barrier, always-on audit
    (`crates/flicker-poc-chemistry/src/scheduler.rs:78,129`) — the template for a
    deterministic, restartable batch sweep.
  - gzip **`bake`** round-trip with stable ordering (`crates/flicker-voxel/src/bake.rs`).
  Drive the translators through that pattern; do **not** invent new batch infra
  (law `do-not-reinvent-existing-systems`).
- **[decision][future] Import host (D2).** Our-owned translators run in-app (Rust) and/or as
  our offline tools (Python, like `retarget_bvh`); in-app glTF/FBX are later spokes into the
  same hub. Blender stays a front-door *only* where it is already the owned path
  (`muse-character-pipeline-handoff.md`).

### Layer C — Material Composer (cross-cutting, terrain-reusable)

- **[new][near] Author materials into our format.** No material-authoring UI exists anywhere
  (greenfield). Author at the **shared `flicker-materials` vocabulary** (Elements → Compounds
  → Materials; `Tables`/`TableSource`, `crates/flicker-materials/src/`) and **project to
  render maps** — so the composer is **not** hardwired to the character PBR struct.
- **[future] Reusable for terrain.** Worldgen consumes materials at that same
  `flicker-materials` level (`FieldSampler`/`CellSample`, `crates/flicker-worldgen/src/field.rs`),
  so authoring there makes the composer serve both outfits (now) and terrain (later) — the
  generic usefulness Aaron called for.
- **[reuse+fix][near] Close the format gap.** The render `format::Material`
  (`flicker-skeletal/src/format.rs:113`) is missing **`Emit`/`ORM`**, which our own content
  standard mandates (`Alpha/content/README.md:62`). Align the format to the standard as part
  of this (law `canon-values-align-everywhere`).
- Material is thereby the **third kind of piece** in the Translation Toolkit (mesh /
  animation / material).

### Layer D — Content Pipeline Editor (the near-term app that exercises A+B+C)

The concrete deliverable: a **dedicated crate** (D1) — an editor that **consumes the
Scene System**, **runs the Translation Toolkit**, and **hosts the Material Composer**. Ideal flow (Aaron's driving
notes): open a folder → parse/classify → interactive classify + rig/fit/correct → bake →
hot-reload live. Stages:

**Ingest → Classify → Fit/Correct → Skin → Bake → Reload.**

- **[new][near] Ingest** — a folder/file dialog (no dialog dep exists today) → parse/detect
  the contents; reuse the rig-by-most-vertices heuristic (`format.rs:394`) and the
  FBX-glob + manifest match (`tools/blender/convert_meshy_prop.py:80`).
- **[new][near] Classify** — author the `manifest.json` entry (name / slot / socket / match)
  *interactively* — the classifier is an in-app authoring UI for the file that today is
  hand-edited — honoring the naming standard (`PascalCase-Hyphenated`, role-named
  `<AssetName>_<Map>.png`, **skip-if-unmatched**). Surface cloth-region classification
  (`skin_outfit.py:88`) as a reviewable step, not a black box.
- **[reuse+extend][near] Fit / Correct** — the load-bearing stage:
  - Keep the fit gadget's placement loop (`pick_slot`, `Alpha/flicker-paperdoll/src/main.rs:1195`;
    `record_fits` → `fits.json`, `main.rs:1233`).
  - **[reuse] Euler → quaternion authoring is CONFIRMED / in-progress** (rebuild-spec D.6;
    `PieceFit.user_rot` → quat, `main.rs:299`) — coordinate with the parallel session doing
    the animation cleanup; do not fork the fit gadget.
  - **[decision] Correct A-pose drift through skin (D3).** AI-generated art *impersonates* an
    A-pose but drifts (43°, 47°, …). Straight-on ortho view → bend the limb to where the
    generated cloth actually sits → compute the offset that conforms the piece to the
    canonical A-pose bind. **Recommended model:** reuse the retarget's **static rest-rebase** —
    a per-region correction `Sm` aligning the garment's implied bone-direction onto the bind
    bone-direction (the static twin of `Ta_b = Sa_b·Sm_b⁻¹·A_b`, `retarget_bvh.py
    source_base_pose`, rebuild-spec §C.2) — stored as a **sidecar delta** and applied
    **through the skin palette**, not as a rigid socket rotation. **Pivot-align before
    measuring the angle.**
  - **[new] Slot isolation** = a *view config* (Layer A): render only the region + bone
    overlay + skin outline for the picked slot, hiding the rest. Built on the existing
    `editing` selection + per-slot `Slot.on` (`main.rs:731,411`); today everything worn draws
    every frame with no isolation.
  - **[new] Reference cages** — hip / chest / feet fitting units authored once (from the
    skeleton + bind-mesh region extents), matched to the bone; everything is fit against them.
  - **[new] Octahedral bone widget** (Unreal-style diamond, ⅔–⅓ weighted toward the joint,
    sphere at each origin) on the existing lines / billboard / mesh primitives + the existing
    bone-world-transform plumbing (`bone_segments`/`bone_axis_segments` over
    `pose::global_transforms`, `main.rs:1334`). The line+axis overlay already exists; the
    diamond is the new draw.
- **[reuse][near] Skin / Bind** — nearest-body-vertex weight-transfer + region cloth
  (`skin_outfit.py --build-cloth`), CPU-skin preview (`skin::palette`/`skin`,
  `draw_skinned_instanced`). Surface the existing Python algorithm as an in-tool stage.
- **[reuse][near] Bake** — write `.skinned.json` / `flicker.rig` / `fits.json` /
  `manifest.json` + the correction sidecar into the by-type content tree. Outputs are also
  **bundled as biome+tier-tagged content kits** — the unit the Dungeon-Maker tech tree unlocks
  (§0, D12); the tech-tree *gating* itself is gameplay (framed, out of scope here).
- **[new][near] Reload** — greenfield asset file-watch → hot-reload (only Lua-layout re-eval
  + a shell `Action::Reload` exist today; no asset file-watching).
- **[future] Bake to compact binary** — the recorded-but-deferred objective
  (`Alpha/content/README.md:106`; memory *content → compressed binary*). Not now.

**UI law:** every panel here is a Lua widget set (`hud.lua` + `ui_elements.json` +
`flicker-widgets/widgets.lua`), engine as single source of truth — **never HTML, never a
parallel UI** (memory `flicker-lua-ui-system`).

---

## 3. Domain model (objects → homes)

| Object | Home / reuse |
|---|---|
| Source bundle / Manifest entry | `Alpha/content/source/<Set>/manifest.json`; `convert_meshy_prop.py:80` |
| Asset (classified, named) | `SlotDef`/`SLOTS` (`main.rs:151,175`); naming std `README.md:50` |
| Canonical 66-bone skeleton | `PrismHumanBaseA.json`; `format::Skeleton`/`Bone` (`format.rs:63,244`) |
| Bind pose (actual, pitched-foot A-pose) | `pose::global_transforms` (`pose.rs:62`) |
| Prop / Garment | `Fit::Prop` / `Fit::Garment`; `load_outfit`/`remap_outfit_joints` (`format.rs:542,511`) |
| Fit / Correction | `PieceFit` (`main.rs:199`), `fits.json` + **correction sidecar** (new) |
| Reference cage | **new** (skeleton + bind-mesh region) |
| Cloth region | `skin_outfit.py:88`; `format::Cloth*` (`format.rs:142`); `cloth.rs` |
| Clip / Pack | `retarget_bvh.py`; `format::Clip` (`format.rs:208`); `state::*` |
| Material | `flicker-materials::Tables` + render `format::Material` (`format.rs:113`) |
| Stage / View / Panel | **new core** (Layer A) |
| Content kit / boss kit | biome+tier-tagged bundle of packs/skins/sets/meshes (**new packaging**; the DM tech-tree unlock) |
| Bake target | `.skinned.json` (`skin_outfit.py:261`), `flicker.rig`, `fits.json`, `manifest.json` |

---

## 4. Decisions (open ones flagged with a recommendation)

- **D1 — Editor home** *(resolved by Aaron 2026-07-19: dedicated crate).* The content-pipeline /
  rigging editor is a **dedicated crate** (not evolving `flicker-paperdoll` in place); it hosts the
  pipeline stages as Scene-System views/modes, and the paperdoll fit view is the **seed** that folds
  into it. **The designer-vs-player line is NOT a build/ship split:** who may use the pipeline import
  tools is just an **enablement flag from the auth system** (a runtime entitlement) — a separate
  integration we are **nowhere near**, so the spec must **not** architect a shipped designer-vs-player
  crate split around it now. The dedicated crate is code organization, not an audience boundary. (Crate
  name provisional; placement follows the crate-cluster taxonomy — memory `crate-cluster-taxonomy`.)
- **D2 — Ingest / translator host** *(reshaped).* Hub-and-spoke: our-owned translators;
  near-term ingest what is already converted + our tools; in-app glTF/FBX are later spokes.
- **D3 — Correction model** *(resolved by Aaron 2026-07-19: sidecar delta).* The A-pose-drift
  correction is a **static rest-rebase applied through the skin palette, stored as a sidecar delta**
  (reuses the retarget's `Sm` math; pairs with the D.6 Euler→quat fix). Not a rigid socket rotation,
  not a destructive vertex bake.
- **D4 — Orthographic projection** is **required** (Layer A).
- **D5 — Scene** = stage + named Views + Panels + first-class RTT-View→Panel; camera promoted
  to core.
- **D6 — Factory-efficiency** = drive translators via `WorkerPool` + the chemistry
  `Scheduler` sweep pattern.
- **D7 — Material Composer** authors at the **`flicker-materials` vocabulary**, projects to
  render maps.
- **D8 — Slot isolation & skin-outline** are **view configs**, not a new selection system.
- **D9 — Reference cages** authored from skeleton + bind-mesh regions.
- **D10 — Bake** = existing JSON now; **binary bake stays future**.
- **D11 — Euler → quaternion** for outfit — **confirmed / in-progress** (rebuild-spec D.6).
- **D12 — Dungeon-Maker framing** *(resolved by Aaron 2026-07-19; biome-reuse fuzzy).* Boss
  composition is an **extension over** the recruited-creature base, not a conflict: the boss
  room is a **Design Room** where the DM composes the boss's **presentation** from
  **tech-tree-unlocked kits (animation packs / skins / sets / meshes)** gated by talents + biome
  + tier; unlocking a tier's boss kit **gates tier advancement**; learned biome kits are reusable
  in-biome (dynamic-biome detail pending worldgen). Pipeline outputs feed both dev
  creature-assembly → Bestiary **and** these DM boss kits — never the creature's stat block.

---

## 5. Alignment & guardrails

- **Rebuild spec** (`docs/animation-system-rebuild-spec.md`): the correction work **is** D.6
  (Euler→quat) and touches D.7 (world-vs-local frame) — coordinate with the parallel session.
- **Content-tree / manifest standard** (`Alpha/content/README.md`): classifier writes the
  standard names + manifest; skip-if-unmatched; never bake a guessed scale.
- **Lua-UI law** (`flicker-lua-ui-system`): every panel is a widget set; engine single source
  of truth; no HTML.
- **less-code / do-not-reinvent**: every layer names its reuse anchor; this is assembly of
  existing systems, not a parallel build.
- **DNA-forward / topology-agnostic format** (rebuild-spec §F): the format special-cases
  nothing (variable bone count, name-keyed) — the pipeline stays generic so it serves the
  arbitrary-creature Bestiary.
- **canon-values-align** (28 elements; Book II boss semantics; `Emit`/`ORM` maps).
- **user-verifies-app-themselves**: the editor is a GUI viewer (fine per `dev-box-profile`);
  Claude verifies via build/clippy/test, Aaron drives the window.
- **Access boundary (D1)**: who can use the pipeline import tools (designer vs player) is a **future
  auth-system enablement flag** (runtime entitlement), **not** a build/ship crate split — a separate
  integration we are nowhere near; do not design around it now. The dedicated crate is code
  organization, not an audience/ship boundary.

---

## 6. Explicitly NOT in this doc

Not a roadmap or priority order (the layers are a dependency/feature model; sequencing is
Aaron's) · not the binary content-bake (deferred) · not RigLogic / DNA evaluation (parked) ·
not Adventurer / Dungeon-Maker *gameplay* specs (framed only) · no engine code this pass.

---

## 7. Files referenced (index)

- Paperdoll / fit: `Alpha/flicker-paperdoll/src/main.rs` (`PieceFit` 199, `matrix` 295,
  `pick_slot` 1195, `record_fits` 1233, bone overlay 1334/1355).
- Skeletal runtime: `Alpha/crates/animation/flicker-skeletal/src/{format,pose,skin,cloth,state}.rs`.
- Render / RTT / camera: `crates/flicker-render/src/{renderer,mesh,pipeline_*}.rs`.
- Scene: `crates/flicker-scene/src/lib.rs`. Shell: `Alpha/crates/frontend/flicker-shell/src/shell.rs`.
- Camera cinematics: `Alpha/crates/animation/flicker-flight/src/lib.rs`.
- Batch substrate: `Alpha/crates/core/flicker-worker/src/lib.rs`,
  `crates/flicker-poc-chemistry/src/scheduler.rs`, `crates/flicker-voxel/src/bake.rs`.
- Materials: `crates/flicker-materials/src/`, `Alpha/content/data/materials.json`,
  `crates/flicker-worldgen/src/field.rs`.
- Tools: `tools/{skin_outfit,retarget_bvh}.py`,
  `tools/blender/{convert_meshy_prop,io_scene_flicker_rig}.py`.
- Standards / specs: `Alpha/content/README.md`, `docs/animation-system-rebuild-spec.md`,
  `docs/muse-character-pipeline-handoff.md`, Prism `Alpha/design/Prism/Book{I,II}.md`.

---

## 8. Status & next

**Status:** design-of-record, no code. Grounded against the live tree 2026-07-19 (every
`[reuse]` anchor above resolves to real code at the cited path).

**Immediate next:** all core decisions are set (**D1** dedicated crate · **D3** sidecar-delta
correction · **D12** boss-kit framing). The one outstanding external input is the "4-view + stage-panel"
UI mockup referenced in the driving notes but not in the repo (`Alpha/design/` holds A-pose concept
art + decorative UI chrome only) — share it if it lives in a design tool. The natural first thin slice
is **Layer A** (Scene stage/View/Panel + ortho), since Layers C and D both stand on it — but slice
order is Aaron's to set.

**When implementation begins,** each layer carries its own build/clippy/test + in-window
verification by Aaron; this doc is updated in place as slices land (and `CLAUDE.md` §4/§10 +
the memory index when a subsystem's state changes materially).

# Muse character pipeline — session handoff (2026-07-15)

Handoff from a Cowork session (Blender MCP + repo access, **no Rust toolchain** — nothing
here was compiled). Continue in **Claude Code**, which can `cargo build/test`. Read this
first, then the cross-machine memory entries (MCP `memory_*`, project `flicker`) cited at
the end.

---

## ⟳ Update — Claude Code session (2026-07-15): Muse is an OUTFIT; reduced-bone pipeline landed

**Reframe (Aaron):** Muse is a **clothing OUTFIT, not a character.** The Meshy mesh is a whole
body, but the base body is *not* meant to be part of the model — we want only the textured
outfit, **drawn OVER a separate base-skin model** that owns the full 94-bone skeleton + attach
points. The outfit should carry only the bones it connects to (no head/fingers). **This
supersedes the "body swap" + "2 bugs" framing below** (kept for history).

**What that means for the old bugs:** "Bug A / body horror" is *not* a skinning bug — rest-pose
skinning is bit-exact (0.0 cm). The current `Muse.json` is a full body (legs 58% / torso 26% /
hands 8% / arms 5% / head 3% of weight mass, 65/94 bones), and its **head verts are bound to the
HAIR bones** (Hair_F_01/03), which the clips swing up to ~179° → the "swivel." It dissolves once
the head/body geometry is removed from the outfit. "Bug B / gray" — the code half landed (loader
now reads each body's maps from its own dir).

**Landed this session (engine + exporter — reduced-bone pipeline, Aaron's chosen path):**
- **Exporter** `io_scene_flicker_rig.py` → `outfit=True` (operator checkbox *"Outfit (reduced bone
  set)"*): keep only bones the mesh weights + ancestor chain to root, re-index parents, remap
  mesh joints. Per-bone matrices unchanged, so shared bones still match the base exactly.
  `outfit=False` stays byte-identical to the validated Katanami export.
- **Engine** `flicker-skeletal::format::load_outfit(path, base_bones)`: remaps an outfit's reduced
  joints into base-skeleton index space **by bone NAME**. Backward-compatible with the current
  full-94 file (identity remap). 2 new unit tests.
- **paperdoll**: Muse is now an `OutfitLayer` **drawn OVER the base body** (both skinned by the
  same per-frame palette), not a body-swap. **Key 4 toggles the overlay**; 1/2/3 select base skin.
  Removed `active_char`/`set_muse`/`set_katanami`/`rebuild_sub`/`katanami_mesh`.
- **Verified without Blender**: parity harness on real `Muse.json` — exporter-reduce → engine-remap
  round-trips to the original joints with **0 / 282 394 mismatches** (bit-identical skinning);
  reduced tree valid (94→78 on the current full-body mesh). Workspace builds; skeletal 17 + paperdoll
  5 tests green. Only the bpy glue (vertex-group iteration, matrix extraction) is Blender-untested —
  it mirrors the already-validated full-export path.

**Remaining — Blender/Cowork (content), then it's viewer-verifiable:**
1. Delete the base body/head/hands from the Meshy Muse — keep only the outfit geometry.
2. **Data-Transfer weights from the Katanami base body** onto the outfit (don't ship Meshy's
   smeared auto-weights).
3. Re-export via the addon with **Outfit (reduced bone set)** ON → overwrites `muse/Muse.json`.
4. Drop `Baked_BaseColor.png` + `Baked_MetallicRoughness.png` beside `Muse.json` (loader already
   loads them; MetallicRoughness is a *packed* map — channel split may need a later fix).

Then `cargo run -p flicker-paperdoll`, key 4 = outfit over base. New memory: decision *"Muse = an
OUTFIT LAYER (reduced-bone …) — reduced-bone pipeline LANDED in Claude Code"*.

---

## What this thread is

Standing up the **Blender → `flicker.rig` exporter** and using it to bring a **Meshy-generated
"Muse" character** into the engine as a swappable body in `flicker-paperdoll`. Three things
landed; two bugs remain (below).

## Landed

### 1. Blender exporter addon — `tools/blender/io_scene_flicker_rig.py`
Emits `flicker.rig` v1 (skeleton + mesh + materials + morphs; experimental clip bake) straight
from a `.blend`. Install via Preferences → Add-ons → Install from Disk → **File > Export >
flicker.rig (.json)**; also importable as a module (how it was driven over MCP).
`export_rig(arm_obj, mesh_obj, unit_scale=1.0, uv_name="", export_clips=False)` is the core.

**Validated byte-exact vs the Katanami oracle** (`Alpha/content/characters/katanami/Katana_Morph_Color1.json`):
skeleton worst-local `7.7e-4`, worst-inverse_bind `2.2e-3`, 0 parent/order mismatches; mesh
positions `5e-7`, UV/weights at float-epsilon, 0 skin-binding mismatches (validated earlier on
the original Katanami mesh). Conventions: column-major matrix flatten (row-vector layout),
`inverse_bind = colmajor(matrix_local⁻¹)`, cm units, UV `v→1-v`, 4-weight cap renormalized,
per-material submeshes.

Two fixes forced by the real Meshy conform target (both in the addon, both important):
- **Units via object transform, not a scalar.** Conformed mesh was in METERS while the skeleton
  was CM (armature scaled 0.01). Mesh verts are mapped through `M = arm.matrix_world⁻¹ @ mesh.matrix_world`
  into armature-local (cm) space — reconciles units regardless of per-object scale, and reduces
  to identity (byte-exact) for the coincident-cm Katanami.
- **Root identity-framing.** Blender reorients the imported root bone (90° X), but the engine +
  baked clips expect `root == identity`; the drift cascades into the root's children. The
  exporter re-frames a top bone named `root` (at origin, no weights) to identity. **bpy gotcha:
  compare bones by `.name`, not `is` (fresh wrappers → identity checks silently fail).**

### 2. Muse export — `Alpha/content/characters/muse/Muse.json`
The Meshy "Lonely Muse" mesh conformed to the Katanami 94-bone skeleton, exported via the addon.
94 bones, 74,841 verts, cm space, weights normalized, single material. In Blender the scene has
`Katanami_Rig`/`Mesh1.0` (the conformed Muse on the flicker skeleton) and the raw Meshy
`char1`/`…Armature` (24-bone Mixamo-style rig — NOT exported).

### 3. paperdoll body swap — `Alpha/flicker-paperdoll/src/main.rs` (+ `flicker-skeletal/format.rs`)
Muse rides the SAME skeleton + clips + state machine, so a swap only replaces `model.mesh` +
`sub`. Keys: **1/2/3 → Katanami body + skin Color_1/2/3**, **4 → Muse body** (`set_katanami` /
`set_muse`; `active_char` 0/1). HUD shows `body Katanami Color_N` / `body Muse`. Added `Clone`
to `Mesh`/`Vertex`. Muse mesh loaded in `build_viewer` from `…/muse/Muse.json` (optional).
**Not compiled — build it first in Claude Code.**

## Open bugs (for Claude Code)

### A. Skinning artifacts — head swivels, body deforms wrong ("body horror")
The Muse animates but deforms badly (head flips). This is a **weighting / bind-pose problem on
the conformed mesh**, not the swap code. Hypotheses to check, cheapest first:
1. **Bind-pose mismatch** — the Meshy mesh may have been bound at a rest pose that doesn't match
   the skeleton's rest the clips assume. Verify `skinning_rest_matches_bind` holds for `Muse.json`
   (skinned rest == bind). If not, the mesh wasn't in the skeleton's rest at export.
2. **Weight quality** — Meshy's auto-weights, transferred to a 94-bone rig, may be smeared across
   wrong bones (head/neck especially — the visible symptom). Inspect the head/neck vertex weights
   in `Muse.json`; compare to Katanami's. May need a cleaner weight transfer in Blender
   (Data Transfer from the Katanami body, or re-project) before re-export.
3. **Joint index remap** — confirm the Muse vertex `joints` indices line up with the 94-bone
   order the clips resolve against (they should, since skeleton order matched the oracle, but
   verify a few head verts point at `head`/`neck_01`).

### B. Muse renders untextured (gray)
- **UVs are fine** — exported per-vertex. Not the problem.
- The material references `Baked_BaseColor.png` / `Baked_MetallicRoughness.png` (Meshy bakes)
  which are **not in the repo**, and the paperdoll texture loader (`Viewer::enter`) scans only
  the Katanami assets dir with a partly-hardcoded name list + `Katanami_` prefix logic.
- Fix: (1) drop the Meshy baked PNGs into `Alpha/content/characters/muse/`; (2) extend the init
  texture loader to load the active/muse mesh's material maps from the muse dir (generalize it to
  walk `model.mesh.materials` from a per-body asset dir instead of the hardcoded Katanami list).
  Split BaseColor (sRGB) vs Metallic/Roughness (linear) as the existing loader already does.

## Build / run
- `cargo build -p flicker-paperdoll` (first build here). `cargo run -p flicker-paperdoll`.
- Controls: drag/wheel cam · Space play/pause · ←/→ step · ↑/↓ clip · G graph/manual ·
  **1/2/3 Katanami skin · 4 Muse** · M/T/B/K view · R reset · Esc.
- Convention reminders (CLAUDE.md §8): stay out of git; the user runs the window and reports;
  thin slices; wrap big work back into this handoff.

## Files touched
- `tools/blender/io_scene_flicker_rig.py` (new — the exporter addon)
- `Alpha/content/characters/muse/Muse.json` (new — the exported Muse rig; needs baked PNGs beside it)
- `Alpha/flicker-paperdoll/src/main.rs` (body-swap: fields, `set_katanami`/`set_muse`, key 4, HUD, `build_viewer`)
- `Alpha/crates/animation/flicker-skeletal/src/format.rs` (`Clone` on `Mesh`, `Vertex`)

## MCP memory (project `flicker`) — cross-machine record
- decision — Blender→flicker.rig pipeline (drop FBX for owned content)
- invariant — flicker.rig matrix convention CONFIRMED (column-major flatten)
- invariant — exporter SHIPPED + the two conform fixes (units, root identity)
- spec — flicker character skeleton contract (UE mannequin + sockets + secondary bones)
- decision — Muse = elegant gothic aristocrat, derived from Katanami base
- decision — Meshy for mesh/skin gen; deferred behind the Unity-platform migration
- snippet — Meshy MCP setup + tools
- note — paperdoll key-4 Muse body swap (this feature)

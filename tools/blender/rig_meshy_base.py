"""
rig_meshy_base.py — headless: rig an UNRIGGED mesh (e.g. a Meshy base body) onto the
Katanami skeleton, borrowing its default weights and dropping the cloth/hair/breast
jiggle handles, then export a flicker.rig JSON via the io_scene_flicker_rig addon.

    Blender --background <unrigged>.blend --python rig_meshy_base.py -- \
        --fbx    <Katanami rig>.FBX \
        --out    <output>.json \
        --katanami-json <Katanami rig>.json \
        [--decimate 0.2]

The mesh to rig is whatever MESH lives in the opened .blend (the first one found).

Reproduce content/characters/base_human_female/BaseHumanFemale.json:
    Blender --background \
      Alpha/content/characters/base_human_female/Meshy_AI_Female_Human_Base_Mod_*.blend \
      --python tools/blender/rig_meshy_base.py -- \
      --fbx  ~/Repos/VoxelmancyProject/KatanamiExtraction/Meshes/Katana_Morph_Color1.FBX \
      --out  Alpha/content/characters/base_human_female/BaseHumanFemale.json \
      --katanami-json Alpha/content/characters/katanami/Katana_Morph_Color1.json \
      --decimate 0.2

Pipeline: decimate -> import the Katanami FBX -> isolate the Body+Head SKIN as the weight
source (by tri-count fingerprint; the material NAMES are misleading) -> delete the 31
jiggle bones -> bbox-align the mesh to the skin -> Data-Transfer weights (nearest) ->
DISSOLVE each jiggle bone's weight into its surviving parent (a plain delete zeroes the
hip band, which is ~100% Pevis_Cloth on the Katanami skin) -> Limit-4 + Normalize ->
save the mesh's textures as PNG beside the output -> export via the addon (full mode; the
addon synthesizes the `root` bone).

*** PREREQUISITE — POSE ALIGNMENT ***
The input mesh MUST be modelled in (approximately) the Katanami skeleton's REST pose (a
~45deg A-pose, feet ~shoulder width, arms out-and-down). The clips are baked against that
rest, so the skeleton's rest CANNOT be changed without breaking every animation — the mesh
has to come to the skeleton, not the other way round. If the mesh is in a different pose
(arms lower/higher, different proportions), the arm/leg BONES land OUTSIDE the mesh limbs
and no weighting can save it (hands collapse onto the forearm, knees push through, etc.).
This script prints a POSE-ALIGNMENT report (per-bone bone->nearest-vertex distance); a
limb bone should sit a few cm INSIDE its limb. Distances >~10cm at the hands/feet mean the
mesh needs fitting to the A-pose first (interactive: fit the armature to the mesh, then
re-pose the mesh back to the A-pose and apply, so bind-pose == clip rest).
"""
import bpy, sys, os, math, json, argparse
from mathutils import Vector

# ---- args (after the `--`) ----
argv = sys.argv[sys.argv.index("--") + 1:] if "--" in sys.argv else []
ap = argparse.ArgumentParser()
ap.add_argument("--fbx", required=True, help="Katanami rig FBX (armature + weighted mesh)")
ap.add_argument("--out", required=True, help="output flicker.rig .json")
ap.add_argument("--katanami-json", required=True, help="Katanami flicker.rig JSON (for the jiggle->parent map)")
ap.add_argument("--decimate", type=float, default=0.2, help="collapse-decimate ratio for the input mesh (1.0 = none)")
ap.add_argument("--tools", default=os.path.dirname(os.path.abspath(__file__)), help="dir holding io_scene_flicker_rig.py")
args = ap.parse_args(argv)
OUT_DIR = os.path.dirname(os.path.abspath(args.out))

# The 31 secondary-motion handles dropped from the base rig (cloth 16 + sleeve 6 + hair 7
# + breast 2). All leaves in the Katanami skeleton — safe to delete.
JIGGLE = {"Breast_left", "Breast_right"}
for lr in ("B", "F", "L", "R"):
    JIGGLE |= {f"Pevis_Cloth_{lr}_01", f"Pevis_Cloth_{lr}_02"}
for lr in ("L", "R"):
    JIGGLE |= {f"Cloth_B_{lr}_0{i}" for i in range(1, 5)}
    JIGGLE |= {f"sleeve_{lr}_0{i}" for i in range(1, 4)}
    JIGGLE |= {f"Hair_{lr}_01", f"Hair_{lr}_02"}
JIGGLE |= {"Hair_F_01", "Hair_F_02", "Hair_F_03"}
# Body+Head skin = the material slots whose tri counts fingerprint Body(2136) + Head(873).
SKIN_TRI_COUNTS = {2136, 873}

import importlib.util
_spec = importlib.util.spec_from_file_location("fr_local", os.path.join(args.tools, "io_scene_flicker_rig.py"))
fr = importlib.util.module_from_spec(_spec)
_spec.loader.exec_module(fr)  # load the repo copy, not any installed addon

def log(*a): print("[rig]", *a)

# ---- 1) input mesh + decimate ----
base = next(o for o in bpy.data.objects if o.type == "MESH")
log(f"input mesh '{base.name}' verts={len(base.data.vertices)}")
if args.decimate < 0.999:
    bpy.ops.object.select_all(action="DESELECT"); base.select_set(True); bpy.context.view_layer.objects.active = base
    dm = base.modifiers.new("dec", "DECIMATE"); dm.decimate_type = "COLLAPSE"; dm.ratio = args.decimate
    bpy.ops.object.modifier_apply(modifier=dm.name)
    log(f"decimated -> {len(base.data.vertices)} verts (ratio {args.decimate})")

# ---- 2) import Katanami FBX ----
before = set(bpy.data.objects)
bpy.ops.import_scene.fbx(filepath=args.fbx)
new = [o for o in bpy.data.objects if o not in before]
arm = next(o for o in new if o.type == "ARMATURE")
kmesh = next(o for o in new if o.type == "MESH")
log(f"imported armature '{arm.name}' bones={len(arm.data.bones)}")

# ---- 3) isolate the Body+Head skin as the weight source ----
from collections import Counter
tc = Counter()
for p in kmesh.data.polygons:
    tc[p.material_index] += (len(p.vertices) - 2)
skin_slots = {mi for mi, n in tc.items() if n in SKIN_TRI_COUNTS}
bpy.ops.object.select_all(action="DESELECT"); kmesh.select_set(True); bpy.context.view_layer.objects.active = kmesh
bpy.ops.object.duplicate()
skin = bpy.context.view_layer.objects.active; skin.name = "Katanami_Skin"
import bmesh
bm = bmesh.new(); bm.from_mesh(skin.data); bm.faces.ensure_lookup_table()
bmesh.ops.delete(bm, geom=[f for f in bm.faces if f.material_index not in skin_slots], context="FACES")
bm.to_mesh(skin.data); bm.free()
log(f"skin source: slots {skin_slots}, {len(skin.data.vertices)} verts")

# ---- 4) delete the 31 jiggle bones ----
bpy.ops.object.select_all(action="DESELECT"); arm.select_set(True); bpy.context.view_layer.objects.active = arm
bpy.ops.object.mode_set(mode="EDIT")
for nm in list(JIGGLE):
    eb = arm.data.edit_bones.get(nm)
    if eb:
        arm.data.edit_bones.remove(eb)
bpy.ops.object.mode_set(mode="OBJECT")
surviving = {b.name for b in arm.data.bones}
log(f"armature now {len(arm.data.bones)} bones (dropped jiggle)")

# ---- 5) bbox-align the mesh to the skin ----
def wbb(o):
    cs = [o.matrix_world @ v.co for v in o.data.vertices]
    return (Vector((min(c.x for c in cs), min(c.y for c in cs), min(c.z for c in cs))),
            Vector((max(c.x for c in cs), max(c.y for c in cs), max(c.z for c in cs))))
smn, smx = wbb(skin); bmn, bmx = wbb(base)
base.location += Vector(((smn.x + smx.x - bmn.x - bmx.x) * 0.5,
                         (smn.y + smx.y - bmn.y - bmx.y) * 0.5, smn.z - bmn.z))
bpy.context.view_layer.update()

# ---- POSE-ALIGNMENT CHECK (see the prerequisite note in the docstring) ----
def bone_world(nm):
    b = arm.data.bones.get(nm)
    return None if b is None else (arm.matrix_world @ b.head_local)
bverts = [base.matrix_world @ v.co for v in base.data.vertices]
def nearest_vert(p):
    return min(math.dist(p, v) for v in bverts)
log("POSE-ALIGNMENT (bone -> nearest mesh vert; a limb bone should be a few cm INSIDE):")
worst = 0.0
for nm in ("hand_l", "hand_r", "foot_l", "foot_r", "calf_l", "pelvis", "spine_03", "head"):
    bp = bone_world(nm)
    if bp is not None:
        dm = nearest_vert(bp) * 100.0  # m -> cm
        worst = max(worst, dm if nm.startswith(("hand", "foot")) else 0.0)
        log(f"    {nm:10s} {dm:5.1f} cm")
if worst > 10.0:
    log(f"*** WARNING: extremity bones sit {worst:.0f}cm from the mesh — the input is NOT "
        f"pose-aligned to the Katanami A-pose. Weights (esp. hands/feet) will be wrong; "
        f"fit the mesh to the A-pose before rigging (see docstring). Note: rotation alone "
        f"can't close it — the mesh's proportions (arm length) also differ from the rig. ***")

# ---- 6) Data-Transfer weights skin -> base ----
bpy.ops.object.select_all(action="DESELECT"); base.select_set(True); bpy.context.view_layer.objects.active = base
mod = base.modifiers.new("wt", "DATA_TRANSFER"); mod.object = skin
mod.use_vert_data = True; mod.data_types_verts = {"VGROUP_WEIGHTS"}
mod.vert_mapping = "POLYINTERP_NEAREST"
mod.layers_vgroup_select_src = "ALL"; mod.layers_vgroup_select_dst = "NAME"
bpy.ops.object.datalayout_transfer(modifier=mod.name)
bpy.ops.object.modifier_apply(modifier=mod.name)

# ---- 7) DISSOLVE jiggle weights into the nearest surviving parent, then Limit-4 + Normalize ----
kd = json.load(open(args.katanami_json))
kbn = kd["skeleton"]["bones"]
pname = {b["name"]: (kbn[b["parent"]]["name"] if b["parent"] >= 0 else None) for b in kbn}
def nearest_surviving(nm):
    cur = pname.get(nm)
    while cur is not None and cur not in surviving:
        cur = pname.get(cur)
    return cur
for jb in JIGGLE:
    anc = nearest_surviving(jb)
    jvg = base.vertex_groups.get(jb)
    if not anc or not jvg:
        if jvg:
            base.vertex_groups.remove(jvg)
        continue
    avg = base.vertex_groups.get(anc); ji = jvg.index
    adds = []
    for v in base.data.vertices:
        for g in v.groups:
            if g.group == ji and g.weight > 0.0:
                adds.append((v.index, g.weight)); break
    for vi, w in adds:
        avg.add([vi], w, "ADD")
    base.vertex_groups.remove(jvg)
for vg in list(base.vertex_groups):
    if vg.name not in surviving:
        base.vertex_groups.remove(vg)
bpy.ops.object.select_all(action="DESELECT"); base.select_set(True); bpy.context.view_layer.objects.active = base
try:
    bpy.ops.object.vertex_group_limit_total(limit=4)
    bpy.ops.object.vertex_group_normalize_all()
except Exception as e:
    log("weight cleanup warning:", e)
zero = sum(1 for v in base.data.vertices if sum(g.weight for g in v.groups) < 1e-6)
log(f"weights cleaned; zero-weight verts: {zero}/{len(base.data.vertices)}")

# ---- 8) save the mesh's textures as PNG beside the output ----
saved = []
for m in base.data.materials:
    if not m or not m.use_nodes:
        continue
    for n in m.node_tree.nodes:
        if n.type == "TEX_IMAGE" and n.image:
            out = os.path.join(OUT_DIR, (os.path.splitext(os.path.basename(n.image.filepath))[0] or n.image.name) + ".png")
            try:
                n.image.filepath_raw = out; n.image.file_format = "PNG"; n.image.save(); saved.append(os.path.basename(out))
            except Exception as e:
                log(f"texture save warning ({n.image.name}): {e}")
log(f"saved textures: {saved}")

# ---- 9) export via the addon ----
data = fr.export_rig(arm, base, unit_scale=1.0, uv_name="", export_clips=False,
                     source_file=os.path.splitext(os.path.basename(args.out))[0], outfit=False)
with open(args.out, "w") as f:
    json.dump(data, f)
log(f"WROTE {args.out}: {len(data['skeleton']['bones'])} bones, "
    f"{len(data['mesh']['vertices'])} verts, {len(data['mesh']['materials'])} materials")

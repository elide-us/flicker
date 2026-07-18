"""
flicker.rig v1 exporter — Blender addon + importable module.

Emits the engine's native `flicker.rig` v1 JSON straight from a Blender scene,
replacing the FBX -> C++ FbxImport path for content authored in Blender.

Conventions (confirmed byte-exact against the Katanami oracle, 2026-07-14):
  * Each 4x4 matrix is stored as the COLUMN-MAJOR flatten of the Blender
    mathutils.Matrix  ->  [M[r][c] for c in 0..3 for r in 0..3].
    That yields the FBX row-vector layout (translation in the LAST row).
    The glam consumer reads it with Mat4::from_cols_array and NO transpose.
    Applied to BOTH `local` and `inverse_bind`.
      - local(bone)        = colmajor( parent ? Wp^-1 @ Wb : Wb )
      - inverse_bind(bone) = colmajor( Wb^-1 )
    where W* is the bone's armature-space rest matrix (matrix_local), with its
    translation scaled by `unit_scale` so the whole rig lands in centimeters.
  * Root bone: bone 0 must be a root-at-feet named "root", parent -1. If the
    armature already has a single top bone named "root" it is used as-is;
    otherwise an identity "root" is synthesized and the real top bones parent to it.
  * Units: engine expects CENTIMETERS. FBX-imported data is already cm -> unit_scale=1.0.
    Meter-authored data -> unit_scale=100.0.
  * Mesh: NON-deduplicated verts (one per triangle corner), triangles grouped into
    contiguous per-material submeshes {material,start,count}; sequential indices.
    Vertex: p (cm), n (split/corner normal), uv (v flipped -> 1-v, top-origin),
    joints[4] (bone indices, 0-padded), weights[4] (top-4, renormalized to 1.0).
  * Materials carry the Unreal-style `slot` (from a `flicker_slot` custom prop, else
    the material name) plus base_color/normal/roughness/metalness/ao map basenames,
    and a flat `color` when there is no base_color texture.
  * Outfit / reduced-bone mode (outfit=True): export ONLY the bones the mesh actually
    weights + their ancestor chain to root, re-indexing parents and remapping the mesh
    joints into the reduced set. For a clothing layer skinned over a base body that owns
    the full skeleton; the engine remaps the outfit's joints back by bone NAME at load
    (flicker-skeletal `load_outfit`). Per-bone local/inverse_bind matrices are unchanged
    (only which bones are present + their indices), so shared bones still match the base
    skeleton exactly. outfit=False is byte-identical to the full validated export.
  * Clips (experimental / unvalidated): bake the active action to fixed 60 Hz
    integer-tick tracks keyed by bone name; keys are {t, T, R:[x,y,z,w], S}.
"""

bl_info = {
    "name": "flicker.rig exporter",
    "author": "flicker / ClayEngine",
    "version": (1, 0, 0),
    "blender": (4, 1, 0),
    "location": "File > Export > flicker.rig (.json)",
    "description": "Export the engine's native flicker.rig v1 JSON (skeleton + mesh [+ clip]).",
    "category": "Import-Export",
}

import os
import json
import bpy
from mathutils import Matrix, Vector

FORMAT = "flicker.rig"
VERSION = 1
TICK_RATE_HZ = 60
IDENTITY16 = [1.0, 0.0, 0.0, 0.0,
              0.0, 1.0, 0.0, 0.0,
              0.0, 0.0, 1.0, 0.0,
              0.0, 0.0, 0.0, 1.0]


# ----------------------------------------------------------------------------- math

def colmajor(M):
    """16 floats, column-major flatten of a mathutils.Matrix (row-vector layout)."""
    return [M[r][c] for c in range(4) for r in range(4)]


def scaled_world(bone_matrix, unit_scale):
    """Armature-space rest matrix with its translation scaled to centimeters."""
    m = bone_matrix.copy()
    if unit_scale != 1.0:
        t = m.translation * unit_scale
        m.translation = t
    return m


# ------------------------------------------------------------------------- skeleton

def used_bone_names(arm_obj, mesh_obj):
    """The keep-set for a reduced-bone (outfit) export: every bone name carrying ANY
    skin weight on `mesh_obj`, PLUS each one's ancestor chain up to the root. Ancestors
    are included so every kept bone's parent is also kept — the reduced skeleton stays a
    valid, self-contained tree (no dangling parent index)."""
    arm = arm_obj.data
    vg_name = {vg.index: vg.name for vg in mesh_obj.vertex_groups}
    used = set()
    for v in mesh_obj.data.vertices:
        for g in v.groups:
            if g.weight > 0.0:
                nm = vg_name.get(g.group)
                if nm:
                    used.add(nm)
    keep = set()
    for nm in used:
        b = arm.bones.get(nm)
        while b is not None:
            keep.add(b.name)
            b = b.parent
    return keep


def build_skeleton(arm_obj, unit_scale, root_identity=True, keep=None):
    arm = arm_obj.data
    roots = [b for b in arm.bones if b.parent is None]

    order = []

    def dfs(b):
        order.append(b)
        for c in b.children:
            dfs(c)

    for r in roots:
        dfs(r)

    has_named_root = (len(roots) == 1 and roots[0].name.lower() == "root")
    root_name = roots[0].name if has_named_root else None
    # Force the root anchor to an identity frame. Blender reorients the root bone
    # (points it up +Z) on import, giving matrix_local a 90deg rotation, but the
    # engine + baked clips expect root == identity. Root sits at the origin with no
    # skin weights, so identity-framing it is lossless and restores clip alignment.
    treat_root_id = has_named_root and root_identity

    bones = []
    name_to_idx = {}

    if not has_named_root:
        bones.append({"name": "root", "parent": -1,
                      "local": list(IDENTITY16), "inverse_bind": list(IDENTITY16)})
        name_to_idx["root"] = 0

    for b in order:
        # Outfit mode: skip bones the mesh doesn't weight (and that aren't an ancestor of
        # one). `keep` is ancestor-closed, so any kept bone's parent is also kept → the
        # parent-index lookup below always resolves.
        if keep is not None and b.name not in keep:
            continue
        idx = len(bones)
        name_to_idx[b.name] = idx
        if treat_root_id and b.name == root_name:
            bones.append({"name": b.name, "parent": -1,
                          "local": list(IDENTITY16), "inverse_bind": list(IDENTITY16)})
            continue
        Wb = scaled_world(b.matrix_local, unit_scale)
        if b.parent is not None:
            parent_idx = name_to_idx[b.parent.name]
            if treat_root_id and b.parent.name == root_name:
                Wp = Matrix.Identity(4)
            else:
                Wp = scaled_world(b.parent.matrix_local, unit_scale)
            local = Wp.inverted() @ Wb
        else:
            parent_idx = -1 if has_named_root else 0
            local = Wb
        bones.append({
            "name": b.name,
            "parent": parent_idx,
            "local": colmajor(local),
            "inverse_bind": colmajor(Wb.inverted()),
        })
    return bones, name_to_idx, (not has_named_root)


# ------------------------------------------------------------------------- materials

def _principled(mat):
    if not mat or not getattr(mat, "use_nodes", False):
        return None
    for n in mat.node_tree.nodes:
        if n.type == "BSDF_PRINCIPLED":
            return n
    return None


def _tex_basename(node, input_name):
    """Basename of the first image feeding `input_name` (traverses through e.g. Normal Map)."""
    if node is None:
        return ""
    inp = node.inputs.get(input_name)
    if not inp or not inp.is_linked:
        return ""
    stack = [inp.links[0].from_node]
    seen = set()
    while stack:
        nd = stack.pop()
        if nd in seen:
            continue
        seen.add(nd)
        if nd.type == "TEX_IMAGE" and nd.image:
            fp = nd.image.filepath
            return os.path.basename(fp) if fp else (nd.image.name + ".png")
        for i in nd.inputs:
            if i.is_linked:
                stack.append(i.links[0].from_node)
    return ""


# Principled input → internal map role (the `Alpha/content/README.md` naming vocabulary).
# The ROLE comes from which shader input the texture feeds — NOT its filename — so an FBX
# with embedded/unnamed images (`Image_0`, `Image_3`, …) names as correctly as one with
# descriptive files. Matches the set `build_materials` records.
_TEX_ROLES = [
    ("Base Color", "BaseColor"),
    ("Normal", "Normal"),
    ("Roughness", "Roughness"),
    ("Metallic", "Metallic"),
]


def _image_for_input(node, input_name):
    """The image node feeding `input_name` (traverses intermediates like a Normal Map node)."""
    if node is None:
        return None
    inp = node.inputs.get(input_name)
    if not inp or not inp.is_linked:
        return None
    stack = [inp.links[0].from_node]
    seen = set()
    while stack:
        nd = stack.pop()
        if nd in seen:
            continue
        seen.add(nd)
        if nd.type == "TEX_IMAGE" and nd.image:
            return nd.image
        for i in nd.inputs:
            if i.is_linked:
                stack.append(i.links[0].from_node)
    return None


def save_material_textures(mesh_obj, out_dir, asset_name):
    """Save every texture the materials reference as `<asset>_<Role>.png`, named by the
    material INPUT it feeds (see `_TEX_ROLES`), and reassign each image's `filepath` so the
    export records that clean name. Role-driven, so embedded textures (`Image_0`…) and
    generically-named vendor files both land on the internal standard, uniquely and without
    collision. A single image feeding several inputs (a packed map) is written once and
    reused. Returns the saved basenames. This is THE texture-naming path for every FBX
    converter — do not re-derive names from vendor filenames.
    """
    import os as _os

    saved = []
    n_mats = len(mesh_obj.material_slots)
    for mi, slot in enumerate(mesh_obj.material_slots):
        p = _principled(slot.material)
        if p is None:
            continue
        # Multi-material meshes namespace by slot so two materials' BaseColor can't collide.
        prefix = asset_name if n_mats <= 1 else "%s_m%d" % (asset_name, mi)
        by_image = {}  # image.name -> already-saved fname (same image, multiple inputs)
        for input_name, role in _TEX_ROLES:
            img = _image_for_input(p, input_name)
            if img is None:
                continue
            if img.name in by_image:
                img.filepath = _os.path.join(out_dir, by_image[img.name])
                continue
            fname = "%s_%s.png" % (prefix, role)
            out = _os.path.join(out_dir, fname)
            try:
                img.filepath_raw = out
                img.file_format = "PNG"
                img.save()
                img.filepath = out  # so `_tex_basename` records the clean name
                by_image[img.name] = fname
                if fname not in saved:
                    saved.append(fname)
            except Exception as e:
                print("[fr] texture save warning (%s, %s): %s" % (role, img.name, e))
    return saved


def build_materials(mesh_obj):
    mats = []
    for slot in mesh_obj.material_slots:
        m = slot.material
        p = _principled(m)
        slot_name = (m.get("flicker_slot") if m else None) or (m.name if m else "Material")
        base = _tex_basename(p, "Base Color")
        color = []
        if not base and p is not None:
            c = p.inputs.get("Base Color")
            if c is not None:
                color = [round(c.default_value[0], 4),
                         round(c.default_value[1], 4),
                         round(c.default_value[2], 4)]
        mats.append({
            "name": m.name if m else "Material",
            "slot": slot_name,
            "base_color": base,
            "normal": _tex_basename(p, "Normal"),
            "roughness": _tex_basename(p, "Roughness"),
            "metalness": _tex_basename(p, "Metallic"),
            "ao": "",
            "color": color,
        })
    return mats


# ------------------------------------------------------------------------------ mesh

def _loop_normal_fn(me):
    try:
        me.calc_normals_split()
        return lambda li: tuple(me.loops[li].normal)
    except Exception:
        pass
    try:
        cn = me.corner_normals
        _ = cn[0].vector
        return lambda li: tuple(cn[li].vector)
    except Exception:
        pass
    return lambda li: tuple(me.vertices[me.loops[li].vertex_index].normal)


def build_mesh(arm_obj, mesh_obj, name_to_idx, uv_name):
    me = mesh_obj.data
    lnorm = _loop_normal_fn(me)

    # Map mesh-local verts into ARMATURE-local space, so positions land in the
    # same (cm) space as the bind matrices regardless of per-object scale/units.
    # For the original cm Katanami (mesh & armature coincident) this M == identity,
    # preserving the byte-exact validated output.
    # `arm_obj is None` = a bone-less PROP export (see `export_prop`): there is no
    # armature space to land in, so the mesh's own world transform is used as-is.
    M = mesh_obj.matrix_world if arm_obj is None else (
        arm_obj.matrix_world.inverted() @ mesh_obj.matrix_world)
    N = M.to_3x3()

    uvl = me.uv_layers.get(uv_name) if uv_name else None
    if uvl is None:
        uvl = me.uv_layers.active or (me.uv_layers[0] if len(me.uv_layers) else None)
    uvd = uvl.data if uvl else None

    vg_name = {vg.index: vg.name for vg in mesh_obj.vertex_groups}

    def vw(vi):
        gs = sorted([(g.group, g.weight) for g in me.vertices[vi].groups if g.weight > 0.0],
                    key=lambda x: -x[1])[:4]
        j = [name_to_idx.get(vg_name.get(g, ""), 0) for g, _ in gs]
        w = [x for _, x in gs]
        while len(j) < 4:
            j.append(0)
            w.append(0.0)
        s = sum(w) or 1.0
        return j, [x / s for x in w]

    polys_by_mat = {}
    for p in me.polygons:
        polys_by_mat.setdefault(p.material_index, []).append(p)

    verts = []
    subs = []
    n_slots = max(1, len(mesh_obj.material_slots))
    for mi in range(n_slots):
        start = len(verts)
        for p in polys_by_mat.get(mi, []):
            lps = list(p.loop_indices)
            # fan-triangulate (identity for real triangles -> preserves oracle order)
            for t in range(1, len(lps) - 1):
                for li in (lps[0], lps[t], lps[t + 1]):
                    vi = me.loops[li].vertex_index
                    p = M @ me.vertices[vi].co
                    nrm = (N @ Vector(lnorm(li)))
                    nrm.normalize()
                    if uvd is not None:
                        u, v = uvd[li].uv
                        uv = [u, 1.0 - v]
                    else:
                        uv = [0.0, 0.0]
                    j, w = vw(vi)
                    verts.append({
                        "p": [p.x, p.y, p.z],
                        "n": [nrm.x, nrm.y, nrm.z],
                        "uv": uv,
                        "joints": j,
                        "weights": w,
                    })
        cnt = len(verts) - start
        if cnt > 0:
            subs.append({"material": mi, "start": start, "count": cnt})

    indices = list(range(len(verts)))
    return verts, indices, subs


# ------------------------------------------------------------------------------ clip

def build_clip(arm_obj, action, unit_scale, synthesized_root):
    """Experimental: bake `action` to 60 Hz integer-tick tracks keyed by bone name."""
    scene = bpy.context.scene
    fps = scene.render.fps / max(1e-9, scene.render.fps_base)
    f0, f1 = action.frame_range
    dur_ticks = int(round((f1 - f0) * TICK_RATE_HZ / fps)) + 1

    if arm_obj.animation_data is None:
        arm_obj.animation_data_create()
    prev_action = arm_obj.animation_data.action
    prev_frame = scene.frame_current
    arm_obj.animation_data.action = action

    tracks = {pb.name: [] for pb in arm_obj.pose.bones}
    for tick in range(dur_ticks):
        frame = f0 + tick * fps / TICK_RATE_HZ
        fi = int(frame)
        scene.frame_set(fi, subframe=frame - fi)
        for pb in arm_obj.pose.bones:
            Wb = scaled_world(pb.matrix, unit_scale)
            if pb.parent is not None:
                Wp = scaled_world(pb.parent.matrix, unit_scale)
                local = Wp.inverted() @ Wb
            else:
                local = Wb
            t, r, s = local.decompose()
            tracks[pb.name].append({
                "t": tick,
                "T": [t.x, t.y, t.z],
                "R": [r.x, r.y, r.z, r.w],
                "S": [s.x, s.y, s.z],
            })

    scene.frame_set(prev_frame)
    arm_obj.animation_data.action = prev_action

    track_list = []
    if synthesized_root:
        track_list.append({
            "bone": "root",
            "keys": [{"t": i, "T": [0.0, 0.0, 0.0], "R": [0.0, 0.0, 0.0, 1.0],
                      "S": [1.0, 1.0, 1.0]} for i in range(dur_ticks)],
        })
    for n, k in tracks.items():
        track_list.append({"bone": n, "keys": k})

    return {"name": action.name, "tick_rate_hz": TICK_RATE_HZ,
            "duration_ticks": dur_ticks, "tracks": track_list}


# --------------------------------------------------------------------------- assemble

def export_rig(arm_obj, mesh_obj, unit_scale=1.0, uv_name="", export_clips=False,
               source_file="", outfit=False, retarget=False):
    """Core entry point: build the full flicker.rig dict from an armature + skinned mesh.

    outfit=True emits a reduced bone set (only the bones this mesh weights + ancestors),
    remapping the mesh joints to it — for a clothing layer over a base body. The mesh
    `joints` come out in the reduced index space; the engine remaps them back by bone
    name against the base skeleton (`load_outfit`).

    retarget=True marks this rig for ROTATION-ONLY clip playback (the engine keeps each
    bone's own rest translation and applies only the clip's rotation) — set for a rig
    RETARGETED from a differently-proportioned authoring skeleton, e.g. a Meshy body
    driven by the Katanami clip library. Leave false for the authoring character."""
    keep = used_bone_names(arm_obj, mesh_obj) if outfit else None
    bones, name_to_idx, synthesized_root = build_skeleton(arm_obj, unit_scale, keep=keep)
    verts, indices, subs = build_mesh(arm_obj, mesh_obj, name_to_idx, uv_name)
    materials = build_materials(mesh_obj)

    clips = []
    if export_clips and arm_obj.animation_data and arm_obj.animation_data.action:
        clips.append(build_clip(arm_obj, arm_obj.animation_data.action,
                                unit_scale, synthesized_root))

    return {
        "format": FORMAT,
        "version": VERSION,
        "retarget": retarget,
        "source": {
            "file": source_file or (os.path.basename(bpy.data.filepath) if bpy.data.filepath else ""),
            "fbx_version": "",
            "source_axis": "Z_up",
            "source_unit": "cm",
            "applied_transform": "none",
            "textures": [],
        },
        "skeleton": {"bones": bones},
        "mesh": {
            "vertices": verts,
            "indices": indices,
            "submeshes": subs,
            "materials": materials,
        },
        "morphs": [],
        "clips": clips,
    }


def export_prop(mesh_obj, uv_name="", source_file=""):
    """A bone-less PROP export: geometry + materials only, `skeleton.bones = []` (the shape
    `Mesh_Katana.json` already has). For a rigid item — weapon, sheath, pendant — that the
    engine draws at a socket bone's animated transform rather than skinning.

    Reuses `build_mesh` / `build_materials` unchanged; only the armature-space mapping is
    skipped (there is no armature). With no vertex groups `vw()` already yields joints 0 /
    weights 0, which is exactly right for geometry that is never skinned.

    NOTE the geometry is emitted RAW, in whatever space the source FBX used. Meshy
    normalises every asset's longest axis to ~1.899 about the origin, so a prop's real
    scale/orientation/position is NOT recoverable from the file and is carried separately
    as authored fit data — never baked in here."""
    verts, indices, subs = build_mesh(None, mesh_obj, {}, uv_name)
    return {
        "format": FORMAT,
        "version": VERSION,
        "retarget": False,
        "source": {
            "file": source_file,
            "fbx_version": "",
            "source_axis": "Z_up",
            "source_unit": "cm",
            "applied_transform": "none",
            "textures": [],
        },
        "skeleton": {"bones": []},
        "mesh": {
            "vertices": verts,
            "indices": indices,
            "submeshes": subs,
            "materials": build_materials(mesh_obj),
        },
        "morphs": [],
        "clips": [],
    }


def find_rig(context, use_selection):
    """Locate (armature, skinned mesh) from the current selection / active object."""
    objs = context.selected_objects if use_selection else list(context.scene.objects)
    arm = None
    for o in ([context.active_object] + objs):
        if o and o.type == "ARMATURE":
            arm = o
            break
    if arm is None:
        # maybe a mesh is active/selected -> derive its armature
        for o in ([context.active_object] + objs):
            if o and o.type == "MESH":
                for m in o.modifiers:
                    if m.type == "ARMATURE" and m.object:
                        arm = m.object
                        break
            if arm:
                break
    if arm is None:
        return None, None

    bound = [o for o in context.scene.objects
             if o.type == "MESH" and any(m.type == "ARMATURE" and m.object == arm
                                         for m in o.modifiers)]
    mesh = None
    if context.active_object in bound:
        mesh = context.active_object
    else:
        sel_bound = [o for o in bound if o in objs]
        mesh = (sel_bound or bound or [None])[0]
    return arm, mesh


# ---------------------------------------------------------------------------- operator

try:
    from bpy_extras.io_utils import ExportHelper
    from bpy.props import StringProperty, FloatProperty, BoolProperty

    class EXPORT_OT_flicker_rig(bpy.types.Operator, ExportHelper):
        bl_idname = "export_scene.flicker_rig"
        bl_label = "Export flicker.rig"
        bl_description = "Export the engine's native flicker.rig v1 JSON"
        filename_ext = ".json"
        filter_glob: StringProperty(default="*.json", options={"HIDDEN"})

        unit_scale: FloatProperty(
            name="Unit Scale (to cm)", default=1.0, min=0.0001, max=100000.0,
            description="Multiply positions to reach centimeters. 1.0 if data is "
                        "already cm (FBX-imported); 100.0 if authored in meters.")
        uv_name: StringProperty(
            name="UV Layer", default="",
            description="UV layer to export; blank = active layer")
        use_selection: BoolProperty(
            name="Selected only", default=True,
            description="Resolve the armature/mesh from the current selection")
        export_clips: BoolProperty(
            name="Bake active action (experimental)", default=False,
            description="Also bake the armature's active action to a 60 Hz clip "
                        "(unvalidated)")
        outfit: BoolProperty(
            name="Outfit (reduced bone set)", default=False,
            description="Export only the bones this mesh weights (+ their ancestor "
                        "chain), remapping joints — for a clothing layer skinned over a "
                        "separate base body that owns the full skeleton")

        def execute(self, context):
            arm, mesh = find_rig(context, self.use_selection)
            if arm is None or mesh is None:
                self.report({"ERROR"}, "Could not resolve an armature + skinned mesh. "
                                       "Select the character's armature (and its mesh).")
                return {"CANCELLED"}
            data = export_rig(arm, mesh, self.unit_scale, self.uv_name,
                              self.export_clips, outfit=self.outfit)
            with open(self.filepath, "w") as f:
                json.dump(data, f)
            self.report({"INFO"}, "flicker.rig: %d bones, %d verts, %d submeshes"
                        % (len(data["skeleton"]["bones"]),
                           len(data["mesh"]["vertices"]),
                           len(data["mesh"]["submeshes"])))
            return {"FINISHED"}

    def _menu(self, context):
        self.layout.operator(EXPORT_OT_flicker_rig.bl_idname, text="flicker.rig (.json)")

    def register():
        bpy.utils.register_class(EXPORT_OT_flicker_rig)
        bpy.types.TOPBAR_MT_file_export.append(_menu)

    def unregister():
        bpy.types.TOPBAR_MT_file_export.remove(_menu)
        bpy.utils.unregister_class(EXPORT_OT_flicker_rig)

except Exception:
    # Importable as a plain module (e.g. from the MCP) even without operator registration.
    def register():
        pass

    def unregister():
        pass


if __name__ == "__main__":
    register()

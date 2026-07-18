"""
convert_meshy_prop.py — headless: convert a vendor's unrigged item FBXs (outfit pieces,
weapons, sheaths, jewellery) into bone-less `flicker.rig` prop JSONs + textures, applying
the project's INTERNAL ASSET NAMING STANDARD (see `Alpha/content/README.md`).

    Blender --background --factory-startup --python convert_meshy_prop.py -- \
        --manifest <source set>/manifest.json [--decimate 1.0]

One JSON per FBX, `skeleton.bones = []` — the shape `Mesh_Katana.json` already has. The
engine draws these at a socket bone's animated transform (`Fit::Prop`), never skinned.

*** NAMING — the whole point of the manifest ***
Vendor stems are unstable, non-unique and unreadable (`Meshy_AI_Lonely_Muse_Top_Duste_
0716234729_texture` — truncated mid-word, timestamped, vendor-prefixed). Nothing
downstream should ever see them. The manifest is the RECORDED source→internal mapping:
it lives beside the source bundle, is versioned, and replaces ad-hoc CLI renames that
nothing remembers. Vendor map names are normalised too — Meshy names every item's maps
identically (`Baked_BaseColor`, `texture_0_roughness`), so unnamespaced they COLLIDE in
one output dir and the first item silently wins (caught 2026-07-16: all five Muse002
pieces resolved to the older Muse001 albedo — textures loaded, no error, wrong result).

*** THE GEOMETRY IS EMITTED RAW — READ THIS BEFORE "FIXING" THE SCALE ***
Meshy normalises EVERY asset so its longest axis is ~1.899 units, centred on the origin.
Measured 2026-07-16 across all nine Muse002 + MuseEpicSet assets: boots 1.8970, forearm
wraps 1.8960, pants 1.8887, duster 1.8976, pendant 1.8980, katana 1.8983, katana saya
1.8991, wakizashi 1.8989, wakizashi saya 1.8982. A katana and a pendant come out the same
size — the export carries ZERO real scale, and Meshy's "resize to height" only re-defaults
(user, 2026-07-16), so nothing upstream can supply it either. A piece's real scale /
orientation / position is therefore AUTHORED fit data recorded separately — never baked in
here. Do NOT add a normalise/rescale step: it would bake one guess into the asset and make
the recorded fit unreproducible.
"""
import bpy, sys, os, glob, json, argparse, importlib.util

argv = sys.argv[sys.argv.index("--") + 1:] if "--" in sys.argv else []
ap = argparse.ArgumentParser()
ap.add_argument("--manifest", required=True, help="source-set manifest.json (see Alpha/content/README.md)")
ap.add_argument("--decimate", type=float, default=1.0, help="collapse ratio (1.0 = none; these are already game-res)")
ap.add_argument("--tools", default=os.path.dirname(os.path.abspath(__file__)))
args = ap.parse_args(argv)

_spec = importlib.util.spec_from_file_location("fr_local", os.path.join(args.tools, "io_scene_flicker_rig.py"))
fr = importlib.util.module_from_spec(_spec)
_spec.loader.exec_module(fr)

def log(*a):
    print("[prop]", *a)



MF = json.load(open(args.manifest))
SRC = os.path.dirname(os.path.abspath(args.manifest))
REPO = os.path.abspath(os.path.join(args.tools, "..", ".."))
OUT = os.path.join(REPO, "Alpha", "content", MF["out"])
os.makedirs(OUT, exist_ok=True)
ITEMS = MF["items"]
log(f"manifest '{MF.get('set', '?')}' → {OUT}  ({len(ITEMS)} items)")


def item_of(fbx):
    """The manifest entry matching this FBX (or an empty dict)."""
    stem = os.path.basename(fbx)[:-4]
    for it in ITEMS:
        if it["match"] in stem:
            return it
    return {}


def internal_name(fbx):
    """Vendor stem → the manifest's internal name. Unmatched files are SKIPPED, never
    guessed: an un-named asset in the tree is worse than an absent one."""
    stem = os.path.basename(fbx)[:-4]
    for it in ITEMS:
        if it["match"] in stem:
            return it["name"]
    return None


written, skipped = [], []
for fbx in sorted(glob.glob(os.path.join(SRC, "*.fbx"))):
    name = internal_name(fbx)
    if name is None:
        skipped.append(os.path.basename(fbx))
        continue
    bpy.ops.wm.read_factory_settings(use_empty=True)
    try:
        bpy.ops.import_scene.fbx(filepath=fbx)
    except Exception as e:
        log(f"FAILED to import {os.path.basename(fbx)}: {e}")
        continue
    meshes = [o for o in bpy.data.objects if o.type == "MESH"]
    if not meshes:
        log(f"SKIP {name}: no mesh")
        continue
    # Vendors ship one object per item; join defensively so a multi-object export still
    # lands as a single prop rather than silently exporting only the first piece.
    bpy.ops.object.select_all(action="DESELECT")
    for o in meshes:
        o.select_set(True)
    bpy.context.view_layer.objects.active = meshes[0]
    if len(meshes) > 1:
        bpy.ops.object.join()
        log(f"{name}: joined {len(meshes)} objects")
    mesh = bpy.context.view_layer.objects.active

    if args.decimate < 0.999:
        dm = mesh.modifiers.new("dec", "DECIMATE")
        dm.decimate_type = "COLLAPSE"
        dm.ratio = args.decimate
        bpy.ops.object.modifier_apply(modifier=dm.name)

    # Textures → `<AssetName>_<Role>.png`, named by the material INPUT each feeds (shared,
    # role-driven; see io_scene_flicker_rig.save_material_textures). Robust for embedded /
    # generically-named FBX textures. `filepath` is reassigned BEFORE the export so
    # `_tex_basename` records the internal name. (For an L/R split both halves keep the base
    # `name`, so they share one texture set instead of duplicating ~20MB per side.)
    saved = fr.save_material_textures(mesh, OUT, name)
    if saved:
        log(f"{name}: textures {saved}")

    def emit(obj, out_name):
        data = fr.export_prop(obj, uv_name="", source_file=os.path.basename(fbx))
        with open(os.path.join(OUT, out_name + ".json"), "w") as f:
            json.dump(data, f)
        co = [obj.matrix_world @ v.co for v in obj.data.vertices]
        size = [max(c[i] for c in co) - min(c[i] for c in co) for i in range(3)]
        log(f"WROTE {out_name}.json: {len(data['mesh']['vertices'])} verts, raw size "
            f"{size[0]:.3f} x {size[1]:.3f} x {size[2]:.3f} (vendor-normalised; fit is authored)")
        written.append(out_name)

    if item_of(fbx).get("split") == "lr":
        # A symmetric PAIR in one mesh (gloves, boots) cannot be worn: the halves follow
        # different limbs, so no single bone or transform can place both. Split them by
        # LOOSE PARTS and bin by centroid x — verified on this set: gloves and boots are each
        # exactly 2 disjoint shells at x ≈ ±0.46 / ±0.29. Binning (rather than assuming two
        # parts) keeps it correct if a piece ever separates into more shells per side.
        bpy.ops.object.select_all(action="DESELECT")
        mesh.select_set(True)
        bpy.context.view_layer.objects.active = mesh
        bpy.ops.object.mode_set(mode="EDIT")
        bpy.ops.mesh.select_all(action="SELECT")
        bpy.ops.mesh.separate(type="LOOSE")
        bpy.ops.object.mode_set(mode="OBJECT")
        parts = [o for o in bpy.data.objects if o.type == "MESH"]
        sides = {"L": [], "R": []}
        for o in parts:
            co = [o.matrix_world @ v.co for v in o.data.vertices]
            cx = sum(c.x for c in co) / max(1, len(co))
            sides["L" if cx > 0 else "R"].append(o)
        log(f"{name}: {len(parts)} loose part(s) -> L={len(sides['L'])} R={len(sides['R'])}")
        if not sides["L"] or not sides["R"]:
            log(f"ERROR {name}: split found no {'left' if not sides['L'] else 'right'} half — "
                f"emitting UNSPLIT so the bad geometry is visible rather than silently halved")
            emit(mesh, name)
        else:
            for side, objs in sides.items():
                bpy.ops.object.select_all(action="DESELECT")
                for o in objs:
                    o.select_set(True)
                bpy.context.view_layer.objects.active = objs[0]
                if len(objs) > 1:
                    bpy.ops.object.join()
                emit(bpy.context.view_layer.objects.active, f"{name}-{side}")
    else:
        emit(mesh, name)

if skipped:
    log(f"SKIPPED (no manifest entry — add one rather than let it land unnamed): {skipped}")
log(f"DONE — {len(written)} props: {written}")

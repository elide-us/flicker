#!/usr/bin/env python3
"""
skin_outfit.py — bind a fitted outfit piece to the body rig by WEIGHT TRANSFER, so it
deforms with the body instead of riding as a rigid prop.

    python3 tools/skin_outfit.py [--pieces Corset-Duster,Hem-Pants,...]

The pieces are placed by hand with the paperdoll fit gadget (recorded in
`musefit/fits.json`). This tool bakes that fit into the mesh geometry, then gives each
outfit vertex the skin weights of the NEAREST body vertex — so the cloth follows exactly
the body surface it covers. That reuses the body's already-working deformation (the
`rig_meshy_base` precedent) rather than auto-weighting each piece from scratch, and needs
no Blender armature: the body mesh already carries the weights.

Output: `<Piece>.skinned.json` beside the raw prop. The paperdoll draws it via the
`Fit::Garment` path (skinned with the shared body palette); the raw `<Piece>.json` stays as
the re-fit input. Weapons and the pendant are NOT skinned — they are rigid props at a
socket (and the pendant gets a procedural jiggle chain later).

*** THE FIT MATH MUST MATCH THE ENGINE. *** A piece's baked world transform is
`rest_global[socket] @ fit_matrix`, where `fit_matrix` replicates `PieceFit::matrix()` in
flicker-paperdoll/src/main.rs EXACTLY:
    rot(socket rest-rotation cancel) @ T(offset) @ R(euler XYZ, user_rot) @ S(scale*uniform)
If this drifts from the engine, the baked mesh sits in the wrong place and the transfer
grabs the wrong weights — so the anchor check below (piece lands in its body region) is the
guard.

NOTE the loose regions (skirt hem, sleeves, ribbon — the cloth-physics parts) skin only
ROUGHLY here: a long hem grabs leg weights and splits on the legs. That is expected and is
exactly what the jiggle/cloth layer is for; this pass gets everything attached to the body.
"""
import json, os, sys, argparse, math
import numpy as np

REPO = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
BODY = os.path.join(REPO, "Alpha/content/characters/PrismHumanBaseA/PrismHumanBaseA.json")
MUSEFIT = os.path.join(REPO, "Alpha/content/characters/musefit")
FITS = os.path.join(MUSEFIT, "fits.json")

# Piece -> socket bone. Mirrors `SLOTS` in flicker-paperdoll/src/main.rs (the garments only;
# weapons/pendant are rigid props and are not skinned).
PIECE_SOCKET = {
    "Corset-Duster": "spine_02",
    "Hem-Pants": "pelvis",
    "Foot-Boots-L": "calf_l",
    "Foot-Boots-R": "calf_r",
    "Hand-Gloves-L": "lowerarm_l",
    "Hand-Gloves-R": "lowerarm_r",
}


# ── Region classification (the cloth-physics parts) ────────────────────────────
# The dangly regions of a welded garment (bell sleeves, skirt hem, ribbon) can't be
# loose-part-split, but they fall out of the body bone each vert weighted to. The WHOLE
# dangly garment is cloth (not just the flared tips) so it flows instead of reading as
# baked; the rigid remainder is the body-hugging core (corset, coat torso, collar).
# `--debug-regions` recolours the regions into submeshes so the viewer shows the split.
#   sleeve = ALL forearm/hand-weighted verts (the whole sleeve hangs from the elbow ring).
#   hem    = leg-weighted coat tails + waist-weighted coat panels hanging below the hip line.
LEG_KEYS = ("thigh", "calf", "foot", "shin", "knee")
# region id -> (name, debug rgb 0..1, or None to keep the piece's textured material)
REGIONS = [
    ("rigid", None),
    ("sleeve_l", [1.0, 0.15, 0.15]),   # red
    ("sleeve_r", [0.20, 0.45, 1.0]),   # blue
    ("hem", [0.20, 0.90, 0.30]),       # green
]

# ── Dynamic-cloth chain layout (--build-cloth) ─────────────────────────────────
# Per cloth region: a fan of K straight jiggle chains hung from an anchor bone, each vert
# bound along the nearest chain. Chain direction points from the anchor to that angular
# sector's far extent — so a wrap-around region (hem) gets panels that drape their own way,
# while a straight hang (sleeve) stays near-parallel. Params/K are tunable per region.
CLOTH_SEGMENTS = 5
# stiffness is the DRAPE knob: low = limp (gravity wins, the region hangs down), high =
# stiff cord that holds its modelled shape. Cloth wants LIMP so a flared bell sleeve drapes
# instead of standing out. gravity is the secondary lever (more negative = heavier hang).
CLOTH_REGIONS = {
    "sleeve_l": {"bone": "lowerarm_l", "k": 2,
                 "params": {"gravity": [0.0, 0.0, -600.0], "stiffness": 0.015, "damping": 0.9, "iterations": 8, "max_dt": 1.0 / 30.0}},
    "sleeve_r": {"bone": "lowerarm_r", "k": 2,
                 "params": {"gravity": [0.0, 0.0, -600.0], "stiffness": 0.015, "damping": 0.9, "iterations": 8, "max_dt": 1.0 / 30.0}},
    "hem": {"bone": "pelvis", "k": 4,
            "params": {"gravity": [0.0, 0.0, -800.0], "stiffness": 0.01, "damping": 0.9, "iterations": 8, "max_dt": 1.0 / 30.0}},
}


def classify_regions(dom_names, z, hip_z):
    """Per-vertex region id (0 rigid · 1 sleeve_l · 2 sleeve_r · 3 hem). The WHOLE dangly
    garment is cloth: sleeves = all forearm/hand-weighted verts; hem = leg-weighted tails +
    waist-weighted (pelvis/spine_01) coat panels hanging below the hip line (z < hip_z). The
    rigid remainder is the body-hugging core. A piece with none of these stays all-rigid, so
    it is a no-op for gloves/boots."""
    dn = np.asarray(dom_names)
    z = np.asarray(z)
    reg = np.zeros(len(dn), dtype=np.int64)
    leg = np.array([any(k in n for k in LEG_KEYS) for n in dn])
    low_coat = np.isin(dn, ["pelvis", "spine_01"]) & (z < hip_z)
    reg[leg | low_coat] = 3
    reg[np.isin(dn, ["lowerarm_l", "hand_l"])] = 1
    reg[np.isin(dn, ["lowerarm_r", "hand_r"])] = 2
    return reg


def split_by_region(out_verts, reg, base_materials):
    """Reorder the (non-deduped, sequential-triple) vertex list so each region is a
    contiguous submesh, and give the cloth regions a flat debug colour. Triangles are the
    unit of reorder (verts 3t..3t+2) so geometry is preserved; a boundary triangle goes to
    its cloth region (cloth wins) so the coloured regions read solid. Returns
    (vertices, indices, submeshes, materials, tri_vert_counts)."""
    ntri = len(out_verts) // 3
    r = reg.reshape(ntri, 3)
    tri_reg = np.zeros(ntri, dtype=np.int64)
    for t in range(ntri):
        nz = r[t][r[t] != 0]
        tri_reg[t] = 0 if nz.size == 0 else int(np.bincount(nz).argmax())
    materials = [base_materials[0] if base_materials else {"name": "mat0", "base_color": "", "color": []}]
    new_verts, submeshes, counts = [], [], {}
    for rid, (rname, rgb) in enumerate(REGIONS):
        tris = np.where(tri_reg == rid)[0]
        counts[rname] = int(tris.size) * 3
        if tris.size == 0:
            continue
        if rid == 0:
            mat = 0
        else:
            mat = len(materials)
            materials.append({"name": f"dbg_{rname}", "slot": "dbg", "base_color": "",
                              "normal": "", "roughness": "", "metalness": "", "ao": "", "color": rgb})
        start = len(new_verts)
        for t in tris:
            new_verts.extend(out_verts[3 * t:3 * t + 3])
        submeshes.append({"material": mat, "start": start, "count": len(new_verts) - start})
    return new_verts, list(range(len(new_verts))), submeshes, materials, counts


def log(*a):
    print("[skin]", *a)


def m4(a):
    """16-float COLUMN-major (c*4+r, as the engine stores glam Mat4) -> 4x4 numpy [row][col]."""
    return np.array([[a[c * 4 + r] for c in range(4)] for r in range(4)], dtype=np.float64)


def rest_globals(bones):
    """FK the skeleton at rest -> a world 4x4 per bone."""
    G = [None] * len(bones)
    for i, b in enumerate(bones):
        L = m4(b["local"])
        p = b["parent"]
        G[i] = L if p < 0 else G[p] @ L
    return G


def euler_xyz(x_deg, y_deg, z_deg):
    """glam Quat::from_euler(EulerRot::XYZ, ...) as a 3x3: intrinsic X then Y then Z = Rx@Ry@Rz."""
    x, y, z = math.radians(x_deg), math.radians(y_deg), math.radians(z_deg)
    cx, sx = math.cos(x), math.sin(x)
    cy, sy = math.cos(y), math.sin(y)
    cz, sz = math.cos(z), math.sin(z)
    Rx = np.array([[1, 0, 0], [0, cx, -sx], [0, sx, cx]])
    Ry = np.array([[cy, 0, sy], [0, 1, 0], [-sy, 0, cy]])
    Rz = np.array([[cz, -sz, 0], [sz, cz, 0], [0, 0, 1]])
    return Rx @ Ry @ Rz


def fit_matrix(fit, socket_bone):
    """Replicate PieceFit::matrix(): rot(cancel socket rest) @ T @ R @ S."""
    # `rot` = rotation part of the socket's inverse_bind (its own rest-rotation cancel).
    ib = m4(socket_bone["inverse_bind"])
    rot = np.eye(4)
    rot[:3, :3] = ib[:3, :3]
    s = np.array(fit["scale"], dtype=np.float64) * fit["uniform"]
    S = np.diag([s[0], s[1], s[2], 1.0])
    R = np.eye(4)
    R[:3, :3] = euler_xyz(*fit.get("rotate", [0, 0, 0]))
    T = np.eye(4)
    T[:3, 3] = fit["offset"]
    return rot @ (T @ R @ S)


def nearest_indices(query, body_pts, chunk=512):
    """The nearest body vertex for every query point — EXACT, vectorized, and always valid.
    Chunked numpy brute force (no scipy). Correctness over speed: baggy cloth hangs far from
    the body, so a capped spatial search silently misses it (a hakama leg landed 13.9k verts
    on the body's LAST vertex — head-weighted — via a `[-1]` fallback). A full argmin can't."""
    q = query.astype(np.float32)
    b = body_pts.astype(np.float32)
    out = np.empty(len(q), dtype=np.int64)
    for s in range(0, len(q), chunk):
        e = min(s + chunk, len(q))
        d = ((q[s:e, None, :] - b[None, :, :]) ** 2).sum(2)  # (chunk, n_body)
        out[s:e] = d.argmin(1)
        # farthest-match telemetry: a large min distance = genuinely loose cloth (→ jiggle).
    return out


def skin_piece(name, body, body_pts, fits, bones, idx, body_dom):
    fit = fits["fits"].get(name)
    if fit is None:
        log(f"SKIP {name}: no recorded fit")
        return None
    socket = PIECE_SOCKET[name]
    path = os.path.join(MUSEFIT, name + ".json")
    piece = json.load(open(path))
    V = piece["mesh"]["vertices"]

    RG = rest_globals(bones)
    world = RG[idx[socket]] @ fit_matrix(fit, bones[idx[socket]])
    N3 = np.linalg.inv(world[:3, :3]).T  # normals: inverse-transpose

    p = np.array([v["p"] for v in V], dtype=np.float64)
    n = np.array([v["n"] for v in V], dtype=np.float64)
    baked = (world @ np.c_[p, np.ones(len(p))].T).T[:, :3]
    bn = (N3 @ n.T).T
    bn /= np.clip(np.linalg.norm(bn, axis=1, keepdims=True), 1e-9, None)

    # anchor check: the baked piece must land in its socket's body region.
    sp = RG[idx[socket]][:3, 3]
    ctr = baked.mean(0)
    log(f"{name}: socket {socket} @ z={sp[2]:.0f} · baked center z={ctr[2]:.0f} · "
        f"bbox z {baked[:,2].min():.0f}..{baked[:,2].max():.0f}")

    # weight transfer: each outfit vert takes the nearest body vert's joints+weights.
    nn = nearest_indices(baked, body_pts)
    far = np.sqrt(((baked - body_pts[nn]) ** 2).sum(1))
    log(f"  transfer: nearest-body distance mean={far.mean():.1f} max={far.max():.1f} cm "
        f"({int((far > 8).sum())} verts >8cm = loose cloth, need jiggle)")
    bv = body["mesh"]["vertices"]
    out_verts = [{
        "p": [float(baked[i, 0]), float(baked[i, 1]), float(baked[i, 2])],
        "n": [float(bn[i, 0]), float(bn[i, 1]), float(bn[i, 2])],
        "uv": V[i]["uv"],
        "joints": list(bv[nn[i]]["joints"]),
        "weights": list(bv[nn[i]]["weights"]),
    } for i in range(len(V))]

    # Report the cloth-region split from the transfer we just computed (dominant body bone
    # + nearest-body distance) — informative during a normal skin. The actual recolour for
    # the viewer is done by --debug-regions on the finished <Piece>.skinned.json (no raw-prop
    # fit needed), so it stays out of this bake path.
    bname = {i: b["name"] for i, b in enumerate(bones)}
    dom_names = [bname.get(int(body_dom[nn[i]]), "") for i in range(len(V))]
    hip_z = RG[idx["pelvis"]][2, 3] if "pelvis" in idx else -1e9
    reg = classify_regions(dom_names, baked[:, 2], hip_z)
    rc = {REGIONS[k][0]: int((reg == k).sum()) for k in range(len(REGIONS))}
    log(f"  regions: sleeve_l={rc['sleeve_l']} sleeve_r={rc['sleeve_r']} "
        f"hem={rc['hem']} rigid={rc['rigid']}")
    mesh_out = {"vertices": out_verts, "indices": piece["mesh"]["indices"],
                "submeshes": piece["mesh"]["submeshes"], "materials": piece["mesh"]["materials"]}

    out = {
        "format": "flicker.rig", "version": 1, "retarget": False,
        "source": {"file": name + " (skinned to body)", "fbx_version": "",
                   "source_axis": "Z_up", "source_unit": "cm", "applied_transform": "baked-fit", "textures": []},
        "skeleton": {"bones": body["skeleton"]["bones"]},  # full body skeleton; joints index it, load_outfit remap = identity
        "mesh": mesh_out,
        "morphs": [], "clips": [],
    }
    out_path = os.path.join(MUSEFIT, name + ".skinned.json")
    json.dump(out, open(out_path, "w"))
    log(f"WROTE {name}.skinned.json: {len(out_verts)} verts skinned to the body rig")
    return out_path


def debug_split_existing(name, bones):
    """Recolour an already-skinned garment's cloth regions for the viewer, straight from
    <Piece>.skinned.json — the raw-prop fit only mattered to BAKE the skin, which already
    happened, so this needs no fit. Backs the textured file up once (<Piece>.skinned.bak.json)
    so --restore-regions can put it back even when the fit is gone."""
    path = os.path.join(MUSEFIT, name + ".skinned.json")
    if not os.path.exists(path):
        log(f"SKIP {name}: no {name}.skinned.json to recolour (skin it first)")
        return None
    skn = json.load(open(path))
    sv = skn["mesh"]["vertices"]
    if len(sv) % 3 != 0:
        log(f"SKIP {name}: {len(sv)} verts is not a triangle list")
        return None
    baked = np.array([v["p"] for v in sv], dtype=np.float64)
    bname = {i: b["name"] for i, b in enumerate(bones)}
    dom_names = [bname.get(int(v["joints"][int(np.argmax(v["weights"]))]), "") for v in sv]
    idx = {b["name"]: i for i, b in enumerate(bones)}
    RG = rest_globals(bones)
    hip_z = RG[idx["pelvis"]][2, 3] if "pelvis" in idx else -1e9
    reg = classify_regions(dom_names, baked[:, 2], hip_z)
    rc = {REGIONS[k][0]: int((reg == k).sum()) for k in range(len(REGIONS))}
    log(f"{name}: regions sleeve_l={rc['sleeve_l']} sleeve_r={rc['sleeve_r']} "
        f"hem={rc['hem']} rigid={rc['rigid']}")
    nv, ind, subs, mats, counts = split_by_region(sv, reg, skn["mesh"]["materials"])
    bak = os.path.join(MUSEFIT, name + ".skinned.bak.json")
    if not os.path.exists(bak):
        json.dump(skn, open(bak, "w"))
        log(f"  backed up textured -> {name}.skinned.bak.json")
    skn["mesh"] = {"vertices": nv, "indices": ind, "submeshes": subs, "materials": mats}
    json.dump(skn, open(path, "w"))
    log(f"  DEBUG {len(subs)} submeshes {counts} -> {name}.skinned.json (restore: --restore-regions)")
    return path


def restore_regions(name):
    """Put the textured <Piece>.skinned.json back from the --debug-regions backup."""
    path = os.path.join(MUSEFIT, name + ".skinned.json")
    bak = os.path.join(MUSEFIT, name + ".skinned.bak.json")
    if not os.path.exists(bak):
        log(f"SKIP restore {name}: no backup {name}.skinned.bak.json")
        return
    json.dump(json.load(open(bak)), open(path, "w"))
    os.remove(bak)
    log(f"RESTORED {name}.skinned.json (removed backup)")


def _frame(d):
    """An orthonormal (u, w) perpendicular to unit `d`, for measuring sector angle."""
    up = np.array([0.0, 0.0, 1.0]) if abs(d[2]) < 0.9 else np.array([0.0, 1.0, 0.0])
    u = np.cross(up, d); u /= np.linalg.norm(u) + 1e-12
    w = np.cross(d, u)
    return u, w


def _region_chains(a, P, k):
    """Lay out up to k straight chains fanning from anchor `a` through region verts P.
    Each chain aims from `a` at its angular sector's far centroid. Returns dicts with
    dir/seg_len/extent (anchor is `a` for all)."""
    c = P - a
    d = c.mean(0); n = np.linalg.norm(d)
    if n < 1e-6:
        return []
    d = d / n
    u, w = _frame(d)
    ang = np.arctan2(c @ w, c @ u)
    sec = np.floor((ang + np.pi) / (2 * np.pi) * k).astype(int) % k
    chains = []
    for j in range(k):
        m = sec == j
        if m.sum() < 3:
            continue
        cj = c[m]
        proj_d = cj @ d
        far = cj[proj_d >= np.quantile(proj_d, 0.6)].mean(0)
        dj = far / (np.linalg.norm(far) + 1e-12)
        extent = float((cj @ dj).max())
        if extent < 1e-3:
            continue
        chains.append({"dir": dj, "seg_len": extent / CLOTH_SEGMENTS, "extent": extent})
    return chains


def build_cloth(name, bones, idx):
    """Add dynamic-cloth metadata (mesh.cloth) to <Piece>.skinned.json: classify the cloth
    regions, lay out a jiggle-chain fan per region, and bind each region vert along the
    nearest chain. No raw-prop fit needed (works on the finished skin); geometry untouched."""
    path = os.path.join(MUSEFIT, name + ".skinned.json")
    if not os.path.exists(path):
        log(f"SKIP {name}: no {name}.skinned.json (skin it first)")
        return None
    skn = json.load(open(path))
    sv = skn["mesh"]["vertices"]
    sp = np.array([v["p"] for v in sv], dtype=np.float64)
    bname = {i: b["name"] for i, b in enumerate(bones)}
    dom_names = [bname.get(int(v["joints"][int(np.argmax(v["weights"]))]), "") for v in sv]
    RG = rest_globals(bones)
    hip_z = RG[idx["pelvis"]][2, 3] if "pelvis" in idx else -1e9
    reg = classify_regions(dom_names, sp[:, 2], hip_z)

    regions_out = []
    for rid, (rname, _rgb) in enumerate(REGIONS):
        cfg = CLOTH_REGIONS.get(rname)
        if cfg is None:
            continue
        vidx = np.where(reg == rid)[0]
        if vidx.size < 6 or cfg["bone"] not in idx:
            continue
        a = RG[idx[cfg["bone"]]][:3, 3]
        P = sp[reg == rid]
        chains = _region_chains(a, P, cfg["k"])
        if not chains:
            continue
        dirs = np.array([c["dir"] for c in chains])                     # (C,3)
        seglen = np.array([c["seg_len"] for c in chains])               # (C,)
        ext = np.array([c["extent"] for c in chains])                   # (C,)
        rel = P - a                                                     # (V,3)
        t = np.clip(rel @ dirs.T, 0.0, ext[None, :])                    # (V,C) along-ray
        perp = rel[:, None, :] - t[:, :, None] * dirs[None, :, :]       # (V,C,3)
        pick = np.linalg.norm(perp, axis=2).argmin(1)                   # (V,) nearest chain
        s = t[np.arange(len(P)), pick] / seglen[pick]                   # segment coord [0,S]
        kk = np.clip(np.floor(s).astype(int), 0, CLOTH_SEGMENTS - 1)
        ff = np.clip(s - kk, 0.0, 1.0)
        binds = [{"v": int(vidx[i]), "c": int(pick[i]), "k": int(kk[i]), "f": float(ff[i])}
                 for i in range(len(P))]
        regions_out.append({
            "name": rname, "anchor_bone": cfg["bone"], "params": cfg["params"],
            "chains": [{"anchor": [float(x) for x in a],
                        "dir": [float(x) for x in c["dir"]],
                        "seg_len": float(c["seg_len"]), "segments": CLOTH_SEGMENTS} for c in chains],
            "binds": binds,
        })
        log(f"  {rname}: {len(chains)} chains, {len(binds)} verts bound (anchor {cfg['bone']})")

    skn["mesh"]["cloth"] = {"regions": regions_out}
    json.dump(skn, open(path, "w"))
    log(f"WROTE cloth -> {name}.skinned.json ({len(regions_out)} regions)")
    return path


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--pieces", default=",".join(PIECE_SOCKET))
    ap.add_argument("--debug-regions", action="store_true",
                    help="recolour the cloth regions (sleeves/hem) of the finished <Piece>.skinned.json "
                         "into flat-coloured submeshes so the paperdoll viewer shows the classification. "
                         "Backs the textured file up to <Piece>.skinned.bak.json; undo with "
                         "--restore-regions. Needs no fit (works on the already-skinned mesh).")
    ap.add_argument("--restore-regions", action="store_true",
                    help="restore <Piece>.skinned.json from the --debug-regions backup.")
    ap.add_argument("--build-cloth", action="store_true",
                    help="add dynamic-cloth metadata (mesh.cloth: a jiggle-chain fan per cloth "
                         "region + per-vert binds) to the finished <Piece>.skinned.json so the "
                         "paperdoll runtime swings the sleeves/hem. Needs no fit; geometry untouched.")
    args = ap.parse_args()

    body = json.load(open(BODY))
    bones = body["skeleton"]["bones"]
    idx = {b["name"]: i for i, b in enumerate(bones)}
    body_pts = np.array([v["p"] for v in body["mesh"]["vertices"]], dtype=np.float64)
    body_dom = np.array([v["joints"][int(np.argmax(v["weights"]))] for v in body["mesh"]["vertices"]], dtype=np.int64)
    fits = json.load(open(FITS))
    log(f"body: {len(body_pts)} weighted verts, {len(bones)} bones; grid built")

    for name in args.pieces.split(","):
        name = name.strip()
        if name not in PIECE_SOCKET:
            log(f"SKIP {name}: not a skinnable garment (weapons/pendant stay rigid props)")
            continue
        if args.restore_regions:
            restore_regions(name)
        elif args.debug_regions:
            debug_split_existing(name, bones)
        elif args.build_cloth:
            build_cloth(name, bones, idx)
        else:
            skin_piece(name, body, body_pts, fits, bones, idx, body_dom)


if __name__ == "__main__":
    main()

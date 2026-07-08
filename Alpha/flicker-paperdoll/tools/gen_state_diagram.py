#!/usr/bin/env python3
"""Generate the interactive state-machine diagram straight from the source of truth.

Reads examples/flicker-animation/assets/Katanami.pack.json (the authored state graph)
and the clip durations under assets/clips/, computes a clustered auto-layout, and writes
a self-contained HTML page. The pack is the single source of truth — re-run this whenever
the pack changes and the diagram follows. (Eventually this data lives in the DB and the
editor is TheOracle's graph tool; this script is the stop-gap renderer.)

Usage:  python3 gen_state_diagram.py [out.html]
        (default out: ./state-machine.html)
"""
import json, glob, math, os, sys

HERE = os.path.dirname(os.path.abspath(__file__))
ASSETS = os.path.join(HERE, "..", "assets")
PACK = os.path.join(ASSETS, "Katanami.pack.json")
CLIPS = os.path.join(ASSETS, "clips")
OUT = sys.argv[1] if len(sys.argv) > 1 else os.path.join(os.getcwd(), "state-machine.html")

# ---- read the clip library (name -> duration, + category) ----
def clip_rows():
    rows = []
    for f in sorted(glob.glob(os.path.join(CLIPS, "**", "*.json"), recursive=True)):
        try:
            d = json.load(open(f))
        except Exception:
            continue
        rm = "RootMotion" in f
        cat = os.path.relpath(os.path.dirname(f), CLIPS).replace(os.sep, " / ")
        for c in d.get("clips", []):
            name = ("RM/" if rm else "") + c.get("name", "?")
            rows.append((name, c.get("duration_ticks", 0), cat))
    return rows

LIB = clip_rows()
DUR = {n: d for n, d, _ in LIB}

pack = json.load(open(PACK))
sm = pack["state_machine"]
default_blend = sm.get("default_blend_ticks", 0)

# ---- cluster inference + layout grid ----
def group_of(name):
    if name == "Idle": return "core"
    if name.startswith("Walk"): return "walk"
    if name.startswith("Run"): return "run"
    if name.startswith("Crouch"): return "crouch"
    if name.startswith("Jump"): return "jump"
    if name.startswith("Attack") or name.startswith("Short_Attack"): return "combat"
    if name.startswith("Dame") or name.startswith("Death"): return "react"
    return "misc"

# cluster -> (col,row) grid cell + display label
CELL = {"jump": (0, 0), "core": (1, 0), "walk": (2, 0), "run": (3, 0),
        "crouch": (1, 1), "combat": (2, 1), "react": (3, 1), "misc": (0, 1)}
CLABEL = {"core": "Idle", "walk": "Walk", "run": "Run", "crouch": "Crouch",
          "jump": "Jump", "combat": "Combat", "react": "Reactions", "misc": "Misc"}
GROUPCOL = {"core": "--g-loco", "walk": "--g-loco", "run": "--g-loco", "crouch": "--g-crouch",
            "jump": "--g-jump", "combat": "--g-combat", "react": "--g-react", "misc": "--mute"}

CELL_W, CELL_H = 300, 280
MX, MY = 46, 96
SUB_X, SUB_Y = 168, 88

states = sm["states"]
by_cluster = {}
for s in states:
    by_cluster.setdefault(group_of(s["name"]), []).append(s["name"])

pos = {}
clabels = []
for cl, names in by_cluster.items():
    col, row = CELL.get(cl, (0, 2))
    cx, cy = MX + col * CELL_W, MY + row * CELL_H
    clabels.append({"label": CLABEL.get(cl, cl), "x": cx + 8, "y": cy - 10})
    names = sorted(names)  # base state (bare stem) sorts before its directional suffixes
    n = len(names)
    cols = 1 if n <= 1 else 2
    rows = math.ceil(n / cols)
    gw, gh = cols * SUB_X, rows * SUB_Y
    ox = cx + (CELL_W - gw) / 2 + SUB_X / 2
    oy = cy + (CELL_H - gh) / 2 + SUB_Y / 2
    for i, nm in enumerate(names):
        pos[nm] = (round(ox + (i % cols) * SUB_X), round(oy + (i // cols) * SUB_Y))

VB_W = MX * 2 + 4 * CELL_W
VB_H = MY + 2 * CELL_H + 40
ANY = {"x": round(VB_W / 2), "y": 34, "hw": 32, "hh": 14}

# ---- nodes ----
KINDMAP = {"hitbox_active": "hitbox_active", "cancel_window": "cancel_window",
           "iframe": "iframe", "weapon_trail": "weapon_trail", "sfx": "sfx",
           "footstep": "footstep", "equip": "equip"}
N = {}
for s in states:
    nm = s["name"]
    ev = [{"t": e["tick"], "end": e.get("end"), "k": e["kind"], "l": e.get("label", "")}
          for e in s.get("events", [])]
    N[nm] = {"x": pos[nm][0], "y": pos[nm][1], "clip": s["clip"], "dur": DUR.get(s["clip"], 0),
             "loop": s.get("looping", True), "group": group_of(nm), "ev": ev}
    if s.get("next"):
        N[nm]["next"] = s["next"]

# ---- edges ----
EDGES = []
for s in states:
    nm = s["name"]
    for t in s.get("transitions", []):
        opt = {}
        on = t.get("on", "")
        if t.get("window"):
            opt["win"] = [t["window"]["start"], t["window"]["end"]]
            opt["combo"] = 1
        if on == "clip_done":
            opt["auto"] = 1
            opt["lbl"] = "clip done"
        if "priority" in t:
            opt["pri"] = t["priority"]
        if "blend_ticks" in t:
            opt["blend"] = t["blend_ticks"]
        EDGES.append([nm, t["to"], on, opt])
    if s.get("next"):
        EDGES.append([nm, s["next"], "", {"auto": 1, "lbl": "auto"}])
for a in sm.get("any", []):
    opt = {"any": 1}
    if "blend_ticks" in a:
        opt["blend"] = a["blend_ticks"]
    EDGES.append(["ANY", a["to"], a.get("on", ""), opt])

used = sorted({s["clip"] for s in states})
META = {"states": len(states), "clips": len(LIB), "blend": default_blend,
        "any": len(sm.get("any", []))}

# ---- library grouped by category ----
CATORDER = ["In-Place / MoveBasic", "In-Place / Animation_Dame", "In-Place / Strafe",
            "In-Place / Run_new", "In-Place / Death_new",
            "RootMotion / Locomotions", "RootMotion / MoveBasic"]
libcat = {}
for name, d, cat in LIB:
    libcat.setdefault(cat, []).append([name, d])
LIBOUT = []
for cat in CATORDER + [c for c in libcat if c not in CATORDER]:
    if cat in libcat:
        LIBOUT.append([cat, sorted(libcat[cat])])

TEMPLATE = open(os.path.join(HERE, "state_diagram_template.html")).read()
def inject(tok, val):
    global TEMPLATE
    TEMPLATE = TEMPLATE.replace(tok, json.dumps(val))
inject("__NODES__", N)
inject("__ANY__", ANY)
inject("__EDGES__", EDGES)
inject("__CLABELS__", clabels)
inject("__LIB__", LIBOUT)
inject("__USED__", used)
inject("__META__", META)
inject("__VBW__", VB_W)
inject("__VBH__", VB_H)

with open(OUT, "w") as f:
    f.write(TEMPLATE)
print(f"wrote {OUT}  ({META['states']} states, {len(EDGES)} edges, {len(LIB)} clips, viewBox {VB_W}x{VB_H})")

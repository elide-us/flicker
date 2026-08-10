#!/usr/bin/env python3
"""Bake the controller-glyph TEXTURE ATLAS the Prism UI draws its input hints from.

Source of truth is the SVG set already in the tree at
`Alpha/content/source/prism_ui_icons` (Fluent UI icons plus the hand-authored
`ic_prism_*` ones). This tool is the repeatable form of "make those drawable": each
icon is a single `<path>`, so the glyphs are flattened and scanline-filled into one
RGBA sheet.

ONE sheet, not one texture per glyph: the sprite batch groups its quads by texture
handle, so an atlas draws a whole footer legend in a single bind. The engine picks a
cell with `HudCommand::Sprite`'s `uv` sub-rect; the name -> cell map lives beside the
palette in `ui_elements.json` (`pad_glyphs`), which is what keeps the grid geometry in
exactly one place.

Glyphs are baked WHITE with the coverage in alpha, so a style tints them (bronze for a
resting hint, ink for a lit one) instead of the sheet baking a colour in.

Run:  python3 tools/gen_prism_pad_glyphs.py
"""
import math
import os
import re
from PIL import Image

ROOT = os.path.join(os.path.dirname(os.path.abspath(__file__)), "..")
SRC = os.path.join(ROOT, "Alpha", "content", "source", "prism_ui_icons")
OUT = os.path.join(
  ROOT, "Alpha", "content", "package", "sensorium", "assets", "prism_pad_glyphs.png"
)

# The atlas, in ROW-MAJOR cell order. This list IS the layout: index i sits at
# (i % COLS, i // COLS), which is the same arithmetic `ui_elements.json` documents and
# the engine's `cell_uv` (component.rs) reproduces. Append only — inserting renumbers
# every cell.
GLYPHS = [
  ("lt", "ic_fluent_game_controller_trigger_left_20_filled.svg"),
  ("rt", "ic_fluent_game_controller_trigger_right_20_filled.svg"),
  ("lb", "ic_fluent_game_controller_bumper_left_20_filled.svg"),
  ("rb", "ic_fluent_game_controller_bumper_right_20_filled.svg"),
  ("a", "ic_fluent_game_controller_button_a_20_filled.svg"),
  ("b", "ic_fluent_game_controller_button_b_20_filled.svg"),
  ("x", "ic_fluent_game_controller_button_x_20_filled.svg"),
  ("y", "ic_fluent_game_controller_button_y_20_filled.svg"),
  ("menu", "ic_prism_menu_button_20_filled.svg"),
  ("view", "ic_prism_view_button_20_filled.svg"),
  ("dpad", "ic_prism_dpad_24_filled.svg"),
  ("dpad_up", "ic_prism_dpad_press_up_24_filled.svg"),
  ("dpad_down", "ic_prism_dpad_press_down_24_filled.svg"),
  ("dpad_left", "ic_prism_dpad_press_left_24_filled.svg"),
  ("dpad_right", "ic_prism_dpad_press_right_24_filled.svg"),
  ("stick", "ic_prism_thumbstick_24_filled.svg"),
]
COLS = 4
CELL = 64
# Supersample factor for the fill; the cell is rendered at CELL*SS and box-filtered
# down, which is what gives these thin letterforms a clean edge at HUD sizes.
SS = 4
# Curve flattening: segments per cubic, and per quarter-turn of an arc. Generous —
# this runs once, offline, and the cost is a few hundred points per glyph.
CURVE_STEPS = 24
ARC_STEPS_PER_QUARTER = 12

_NUM = re.compile(r"[-+]?(?:\d*\.\d+|\d+\.?)(?:[eE][-+]?\d+)?")
_CMD = re.compile(r"([MmLlHhVvCcSsQqTtAaZz])")


def parse_path(d):
  """Flatten an SVG path `d` into subpaths: a list of point lists, user units.

  Handles the command set these icons actually use (M/L/H/V/C/S/Q/T/A/Z, absolute
  and relative). Anything outside that raises rather than silently dropping a
  contour — a glyph that loses a subpath is a hole that quietly fills in.
  """
  subpaths, pts = [], []
  cx = cy = 0.0          # current point
  sx = sy = 0.0          # subpath start, for Z
  prev_c2 = prev_q = None  # reflection points for S / T
  tokens = [t for t in _CMD.split(d) if t.strip()]

  def flush():
    if len(pts) > 1:
      subpaths.append(list(pts))
    pts.clear()

  i = 0
  while i < len(tokens):
    cmd = tokens[i]
    args = [float(n) for n in _NUM.findall(tokens[i + 1])] if i + 1 < len(tokens) else []
    i += 2 if (i + 1 < len(tokens) and not _CMD.fullmatch(tokens[i + 1])) else 1
    rel = cmd.islower()
    up = cmd.upper()

    if up == "Z":
      if pts:
        pts.append((sx, sy))
        flush()
      cx, cy = sx, sy
      prev_c2 = prev_q = None
      continue

    # Each command consumes a fixed arity, repeating while arguments remain (an
    # "M 1 2 3 4" is a moveto followed by an implicit lineto, per the SVG spec).
    arity = {"M": 2, "L": 2, "H": 1, "V": 1, "C": 6, "S": 4, "Q": 4, "T": 2, "A": 7}[up]
    if not args:
      continue
    for k in range(0, len(args) - arity + 1, arity):
      a = args[k:k + arity]
      if up == "M":
        x, y = (cx + a[0], cy + a[1]) if rel else (a[0], a[1])
        flush()
        pts.append((x, y))
        cx, cy, sx, sy = x, y, x, y
        up = "L"  # subsequent pairs of an M run are linetos
        prev_c2 = prev_q = None
      elif up in ("L", "H", "V", "T"):
        if up == "H":
          x, y = (cx + a[0], cy) if rel else (a[0], cy)
        elif up == "V":
          x, y = (cx, cy + a[0]) if rel else (cx, a[0])
        else:
          x, y = (cx + a[0], cy + a[1]) if rel else (a[0], a[1])
        if up == "T":
          q = prev_q if prev_q else (cx, cy)
          qx, qy = 2 * cx - q[0], 2 * cy - q[1]
          _quad(pts, cx, cy, qx, qy, x, y)
          prev_q = (qx, qy)
        else:
          pts.append((x, y))
          prev_q = None
        cx, cy = x, y
        prev_c2 = None
      elif up in ("C", "S"):
        if up == "C":
          x1, y1, x2, y2, x, y = a
          if rel:
            x1, y1, x2, y2, x, y = cx + x1, cy + y1, cx + x2, cy + y2, cx + x, cy + y
        else:
          x2, y2, x, y = a
          if rel:
            x2, y2, x, y = cx + x2, cy + y2, cx + x, cy + y
          c = prev_c2 if prev_c2 else (cx, cy)
          x1, y1 = 2 * cx - c[0], 2 * cy - c[1]
        _cubic(pts, cx, cy, x1, y1, x2, y2, x, y)
        cx, cy, prev_c2, prev_q = x, y, (x2, y2), None
      elif up == "Q":
        x1, y1, x, y = a
        if rel:
          x1, y1, x, y = cx + x1, cy + y1, cx + x, cy + y
        _quad(pts, cx, cy, x1, y1, x, y)
        cx, cy, prev_q, prev_c2 = x, y, (x1, y1), None
      elif up == "A":
        rx, ry, rot, large, sweep, x, y = a
        if rel:
          x, y = cx + x, cy + y
        _arc(pts, cx, cy, rx, ry, rot, large, sweep, x, y)
        cx, cy, prev_c2, prev_q = x, y, None, None
  flush()
  return subpaths


def _cubic(pts, x0, y0, x1, y1, x2, y2, x3, y3):
  for s in range(1, CURVE_STEPS + 1):
    t = s / CURVE_STEPS
    u = 1 - t
    pts.append((
      u * u * u * x0 + 3 * u * u * t * x1 + 3 * u * t * t * x2 + t * t * t * x3,
      u * u * u * y0 + 3 * u * u * t * y1 + 3 * u * t * t * y2 + t * t * t * y3,
    ))


def _quad(pts, x0, y0, x1, y1, x2, y2):
  for s in range(1, CURVE_STEPS + 1):
    t = s / CURVE_STEPS
    u = 1 - t
    pts.append((u * u * x0 + 2 * u * t * x1 + t * t * x2,
                u * u * y0 + 2 * u * t * y1 + t * t * y2))


def _arc(pts, x0, y0, rx, ry, rot_deg, large, sweep, x, y):
  """Endpoint -> centre parameterization (SVG implementation notes F.6.5), flattened."""
  if rx == 0 or ry == 0 or (x0 == x and y0 == y):
    pts.append((x, y))
    return
  rx, ry = abs(rx), abs(ry)
  phi = math.radians(rot_deg)
  cos_p, sin_p = math.cos(phi), math.sin(phi)
  dx2, dy2 = (x0 - x) / 2.0, (y0 - y) / 2.0
  x1p = cos_p * dx2 + sin_p * dy2
  y1p = -sin_p * dx2 + cos_p * dy2
  # Scale the radii up if they are too small to span the endpoints (F.6.6).
  lam = (x1p * x1p) / (rx * rx) + (y1p * y1p) / (ry * ry)
  if lam > 1:
    s = math.sqrt(lam)
    rx, ry = rx * s, ry * s
  num = rx * rx * ry * ry - rx * rx * y1p * y1p - ry * ry * x1p * x1p
  den = rx * rx * y1p * y1p + ry * ry * x1p * x1p
  co = math.sqrt(max(num / den, 0.0))
  if large == sweep:
    co = -co
  cxp, cyp = co * rx * y1p / ry, -co * ry * x1p / rx
  cx = cos_p * cxp - sin_p * cyp + (x0 + x) / 2.0
  cy = sin_p * cxp + cos_p * cyp + (y0 + y) / 2.0

  def angle(ux, uy, vx, vy):
    d = math.hypot(ux, uy) * math.hypot(vx, vy)
    if d == 0:
      return 0.0
    c = max(-1.0, min(1.0, (ux * vx + uy * vy) / d))
    a = math.acos(c)
    return -a if ux * vy - uy * vx < 0 else a

  t0 = angle(1, 0, (x1p - cxp) / rx, (y1p - cyp) / ry)
  dt = angle((x1p - cxp) / rx, (y1p - cyp) / ry, (-x1p - cxp) / rx, (-y1p - cyp) / ry)
  if not sweep and dt > 0:
    dt -= 2 * math.pi
  elif sweep and dt < 0:
    dt += 2 * math.pi
  steps = max(2, int(abs(dt) / (math.pi / 2) * ARC_STEPS_PER_QUARTER) + 1)
  for s in range(1, steps + 1):
    t = t0 + dt * (s / steps)
    ex, ey = rx * math.cos(t), ry * math.sin(t)
    pts.append((cos_p * ex - sin_p * ey + cx, sin_p * ex + cos_p * ey + cy))


def rasterize(subpaths, size, scale):
  """Even-odd scanline fill into an `size`x`size` coverage buffer (0..255).

  Even-odd (not nonzero) because these icons rely on it for their counters: the
  A's triangle, the B's bowls, the menu button's three bars are all subpaths cut
  OUT of an outer disc, and a nonzero fill would flood them solid.
  """
  cov = bytearray(size * size)
  edges = []
  for sp in subpaths:
    scaled = [(px * scale, py * scale) for px, py in sp]
    for j in range(len(scaled)):
      x0, y0 = scaled[j]
      x1, y1 = scaled[(j + 1) % len(scaled)]
      if y0 != y1:
        edges.append((x0, y0, x1, y1))
  for py in range(size):
    yc = py + 0.5
    xs = []
    for x0, y0, x1, y1 in edges:
      if (y0 <= yc < y1) or (y1 <= yc < y0):
        xs.append(x0 + (yc - y0) * (x1 - x0) / (y1 - y0))
    if not xs:
      continue
    xs.sort()
    row = py * size
    for k in range(0, len(xs) - 1, 2):  # even-odd: fill between alternating crossings
      a, b = xs[k], xs[k + 1]
      for px in range(max(0, int(math.ceil(a - 0.5))), min(size, int(b + 0.5))):
        cov[row + px] = 255
  return cov


def main():
  rows = (len(GLYPHS) + COLS - 1) // COLS
  sheet = Image.new("RGBA", (COLS * CELL, rows * CELL), (255, 255, 255, 0))
  hi = CELL * SS
  for i, (name, filename) in enumerate(GLYPHS):
    path = os.path.join(SRC, filename)
    with open(path, "r", encoding="utf-8") as fh:
      svg = fh.read()
    box = re.search(r'viewBox="([-\d.\s]+)"', svg)
    span = float(box.group(1).split()[2]) if box else 20.0
    ds = re.findall(r'\sd="([^"]+)"', svg)
    if not ds:
      raise SystemExit(f"{filename}: no <path d=...>")
    subpaths = []
    for d in ds:
      subpaths.extend(parse_path(d))
    cov = rasterize(subpaths, hi, hi / span)
    # White with the coverage in alpha — the sheet carries SHAPE, the style carries
    # colour.
    glyph = Image.frombytes("L", (hi, hi), bytes(cov))
    cell = Image.new("RGBA", (hi, hi), (255, 255, 255, 0))
    cell.putalpha(glyph)
    cell = Image.merge("RGBA", (
      Image.new("L", (hi, hi), 255), Image.new("L", (hi, hi), 255),
      Image.new("L", (hi, hi), 255), glyph,
    ))
    sheet.paste(cell.resize((CELL, CELL), Image.LANCZOS),
                ((i % COLS) * CELL, (i // COLS) * CELL))
    print(f"  {i:2d} {name:<11} {filename}")
  os.makedirs(os.path.dirname(OUT), exist_ok=True)
  sheet.save(OUT)
  print(f"{len(GLYPHS)} glyphs -> {COLS}x{rows} cells of {CELL}px -> {OUT}")


if __name__ == "__main__":
  main()

#!/usr/bin/env python3
"""Regenerate the Prism UI faces the engine embeds.

The engine loads single-weight instances renamed to a project-scoped **Prism role**
family (see Alpha/content/fonts/README.md). This tool is the repeatable form of that
recipe: instance a `wght` from the upstream variable source (no name-table update),
then rewrite the `name`/`OS/2`/`head` tables to the Prism role + RIBBI style so the
renderer's `FontRole` + cosmic-text `Attrs.weight()/.style()` select the right face.

Faces (family + RIBBI style; cosmic-text keys on usWeightClass + italic bit):

    Prism Display  Regular  Cormorant Garamond  600   (headings, default)
    Prism Display  Bold     Cormorant Garamond  700   (bold headings; `bold:true`)
    Prism Label    Regular  Cinzel              600   (tracked caps)
    Prism Body     Regular  EB Garamond         400   (prose)
    Prism Body     Italic   EB Garamond Italic  400   (flavor; `italic:true`)

Run:  python3 tools/gen_prism_fonts.py
"""
import os
from fontTools.ttLib import TTFont
from fontTools.varLib.instancer import instantiateVariableFont

# The generated faces live in the SHIP tree. The upstream variable fonts they are
# instanced from are NOT project content — they are archived in `Prism/Licenses`
# alongside the OSS licences, pending hand-off to the content project. Nothing in
# the engine reads them; only this script does, and only when a face is
# regenerated. (Both paths moved with the package restructure — this script
# pointed at the retired `Alpha/content/fonts` and could not have run.)
FONTS = os.path.join(
    os.path.dirname(__file__), "..", "Alpha", "content", "package", "sensorium", "fonts"
)
SRC = os.path.join(os.path.dirname(__file__), "..", "Prism", "Licenses")

# (source file, wght, family, RIBBI style, output file)
FACES = [
    ("CormorantGaramond[wght].ttf",        600, "Prism Display", "Regular", "CormorantGaramond-SemiBold.ttf"),
    ("CormorantGaramond[wght].ttf",        700, "Prism Display", "Bold",    "CormorantGaramond-Bold.ttf"),
    ("Cinzel[wght].ttf",                   600, "Prism Label",   "Regular", "Cinzel-SemiBold.ttf"),
    ("EBGaramond[wght].ttf",               400, "Prism Body",    "Regular", "EBGaramond-Regular.ttf"),
    ("EBGaramond-Italic[wght].ttf",        400, "Prism Body",    "Italic",  "EBGaramond-Italic.ttf"),
]

WIN, MAC = (3, 1, 0x409), (1, 0, 0)


def set_name(font, value, name_id):
    for pid, eid, lid in (WIN, MAC):
        font["name"].setName(value, name_id, pid, eid, lid)


def rebrand(font, family, style):
    bold = style in ("Bold", "Bold Italic")
    italic = style in ("Italic", "Bold Italic")
    ps_family = family.replace(" ", "")
    full = family if style == "Regular" else f"{family} {style}"
    set_name(font, family, 1)                        # Family
    set_name(font, style, 2)                          # Subfamily (RIBBI)
    set_name(font, f"{family} {style}", 3)            # Unique ID
    set_name(font, full, 4)                           # Full name
    set_name(font, f"{ps_family}-{style.replace(' ', '')}", 6)  # PostScript
    set_name(font, family, 16)                        # Typographic family
    set_name(font, style, 17)                         # Typographic subfamily

    os2 = font["OS/2"]
    fs = os2.fsSelection & ~((1 << 0) | (1 << 5) | (1 << 6))  # clear ITALIC/BOLD/REGULAR
    if italic:
        fs |= 1 << 0
    if bold:
        fs |= 1 << 5
    if not italic and not bold:
        fs |= 1 << 6
    os2.fsSelection = fs

    mac = font["head"].macStyle & ~0b11
    if bold:
        mac |= 0b01
    if italic:
        mac |= 0b10
    font["head"].macStyle = mac


def main():
    for src, wght, family, style, out in FACES:
        src_path = os.path.join(SRC, src)
        out_path = os.path.join(FONTS, out)
        font = TTFont(src_path)
        instantiateVariableFont(font, {"wght": wght}, inplace=True, updateFontNames=False)
        font["OS/2"].usWeightClass = wght
        rebrand(font, family, style)
        font.save(out_path)
        # verify what we just wrote
        v = TTFont(out_path)
        print(f"  {out:34s} family={v['name'].getDebugName(1):14s} "
              f"style={v['name'].getDebugName(2):8s} wght={v['OS/2'].usWeightClass} "
              f"italic={bool(v['OS/2'].fsSelection & 1)} glyphs={len(v.getGlyphOrder())}")

    # Prism Rune — a static runic face (Noto Sans Runic), family-renamed to the
    # Prism role (no instancing). Elder Futhark + extended runic; used for the
    # decorative glowing corner glyphs in the vector HUD panels. OFL.
    rune = TTFont(os.path.join(SRC, "NotoSansRunic-Regular.ttf"))
    rune["OS/2"].usWeightClass = 400
    rebrand(rune, "Prism Rune", "Regular")
    rune_out = os.path.join(FONTS, "NotoSansRunic-Prism.ttf")
    rune.save(rune_out)
    v = TTFont(rune_out)
    print(f"  {'NotoSansRunic-Prism.ttf':34s} family={v['name'].getDebugName(1):14s} "
          f"style={v['name'].getDebugName(2):8s} wght={v['OS/2'].usWeightClass} "
          f"glyphs={len(v.getGlyphOrder())}")


if __name__ == "__main__":
    print("Regenerating Prism faces ->", os.path.normpath(FONTS))
    main()
    print("done.")

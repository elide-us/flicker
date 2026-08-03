# Prism UI fonts

The Prism UI faces used by the engine's text renderer. Each text face is instanced to a
single weight; the rune face is a static rename. All get a **project-scoped family name**
(a Prism *role*, not the original typeface name) so the renderer matches by role, and are
OFL-clean modified versions.

| Generated file (engine loads this) | Family name     | Role · style      | Typeface · weight        |
|------------------------------------|-----------------|-------------------|--------------------------|
| `CormorantGaramond-SemiBold.ttf`   | `Prism Display` | Display · Regular | Cormorant Garamond · 600 |
| `CormorantGaramond-Bold.ttf`       | `Prism Display` | Display · Bold    | Cormorant Garamond · 700 |
| `Cinzel-SemiBold.ttf`              | `Prism Label`   | Label · caps      | Cinzel · 600             |
| `EBGaramond-Regular.ttf`           | `Prism Body`    | Body · Regular    | EB Garamond · 400        |
| `EBGaramond-Italic.ttf`            | `Prism Body`    | Body · Italic     | EB Garamond Italic · 400 |
| `NotoSansRunic-Prism.ttf`          | `Prism Rune`    | Rune · corners    | Noto Sans Runic · 400    |

Weight (600/700) is selected by cosmic-text `Attrs.weight()` off the RIBBI style + `OS/2`
`usWeightClass`; italic by `Attrs.style()`. The renderer maps these from `FontRole` + the
`italic`/`bold` flags on a text command.

## Source vs generated

This folder holds ONLY the **generated** faces the engine embeds. Regenerate them with
`python3 tools/gen_prism_fonts.py` — it pins `wght` via `fontTools.varLib.instancer` (no
name-table update) for the text faces, renames Noto Sans Runic for the rune face, then
rewrites the `name`/`OS/2`/`head` tables to the Prism role + RIBBI style. Do not hand-edit
the generated `.ttf`.

The upstream **variable fonts** they are instanced from (`CormorantGaramond[wght].ttf`,
`Cinzel[wght].ttf`, `EBGaramond[wght].ttf`, `EBGaramond-Italic[wght].ttf`,
`NotoSansRunic-Regular.ttf`, downloaded verbatim from Google Fonts,
`github.com/google/fonts/ofl/<family>`) are **not project content**: nothing in the engine
reads them, and they are archived in **`Prism/Licenses/`** beside the OSS licences, pending
hand-off to the content project. `gen_prism_fonts.py` reads them from there.

## License

SIL Open Font License 1.1. The licence texts (`OFL-CormorantGaramond.txt`, `OFL-Cinzel.txt`,
`OFL-EBGaramond.txt`, `OFL-NotoSansRunic.txt`) live in **`Prism/Licenses/`**, the project's
home for OSS licences, together with the upstream fonts they cover.

The generated faces here are modified versions renamed off the original family names (Prism
roles), as OFL requires for modifications.

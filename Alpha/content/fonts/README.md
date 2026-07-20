# Prism UI fonts

The three serif faces of the **Prism UI Design Language**, used by the engine's text
renderer. Each is instanced to a single weight and given a **project-scoped family name**
(a Prism *role*, not the original typeface name) so the renderer matches by role and the
files are OFL-clean modified versions.

| Generated file (engine loads this) | Family name  | Role (FontRole) | Typeface · weight            |
|------------------------------------|--------------|-----------------|------------------------------|
| `CormorantGaramond-SemiBold.ttf`   | `Prism Display` | Display      | Cormorant Garamond · 600     |
| `Cinzel-Medium.ttf`                | `Prism Label`   | Label / caps | Cinzel · 500                 |
| `EBGaramond-Regular.ttf`           | `Prism Body`    | Body         | EB Garamond · 400            |

## Source vs generated

- **`source/`** — the upstream **variable fonts** (the source) + their OFL licenses,
  downloaded verbatim from Google Fonts (`github.com/google/fonts/ofl/<family>`):
  `CormorantGaramond[wght].ttf`, `Cinzel[wght].ttf`, `EBGaramond[wght].ttf`,
  `OFL-*.txt`.
- **this folder** — the **generated** single-weight instances the engine embeds. Regenerate
  with `fontTools.varLib.instancer` (pin `wght`, no `--update-name-table`) then rewrite the
  `name` table family to the Prism role. Do not hand-edit the generated `.ttf`.

## License

SIL Open Font License 1.1 — see `source/OFL-CormorantGaramond.txt`, `source/OFL-Cinzel.txt`,
`source/OFL-EBGaramond.txt`. The generated instances are modified versions renamed off the
original family names (Prism roles), as OFL requires for modifications.

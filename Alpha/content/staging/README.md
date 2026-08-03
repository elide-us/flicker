# `staging/` — processed content awaiting promotion

**The ingest benches write here. Nothing here ships, and the running game never
reads it.**

```
external sources ──► [Clayworks · Loomforge · retarget · paperdoll] ──► staging/
                                                                          │
                                                        ┌─────────────────┴─────────────────┐
                                                        │   THE CONTENT MANAGER BENCH       │
                                                        │   review · then PROMOTE           │
                                                        └─────────────────┬─────────────────┘
                                                                          ▼
                                                                      package/
```

Before this tier existed, the benches wrote **straight into `package/`** — so
"I imported an asset" and "the asset ships" were the same event, with no review
step and no way to stage work in progress. `staging/` splits them: a bench's
output lands here, and content reaches `package/` only by an explicit
**promotion**, which is the Content Manager's whole job (it also records the
promotion in the package manifest).

(Clayworks' final wizard step is called *Commit* — that is the bench writing its
baked output here. It is unrelated to git.)

## Rules

- **Same at-rest form as `package/`.** Processed text content is gz
  (`<name>.<ext>.gz`), written through `flicker_core::compression` via
  `flicker_content::package`. A promotion is therefore a plain byte move — no
  transcoding, and never a hand-rolled second gz path.
- **Not authoring input.** Raw vendor exports live in `source/` (gitignored).
  This tree holds *processed* output — the pipeline's product, not its input.
- **Layout mirrors `package/`** (`characters/`, `retarget/`, `flights/`, …), so
  a promotion is usually the same relative path under a different root.
- **`.trash/<batch-id>/`** holds files displaced by a Replace-resolved conflict.
  It exists so a batch that overwrote something is still undoable; the Content
  Manager owns it, and nothing else should write there.

The root itself is declared per-executable in the app's `content.json`
(`content_root`), and `staging/` is derived from it — see
`flicker_content::roots`.

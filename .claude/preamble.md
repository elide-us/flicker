**The MCP memory server is the single source of truth for this project.** There are no
local design docs and no local memory files — and you must not create any.
**Read MCP first — every session, every task.** Use `memory_search`, `memory_coderules`,
and `memory_get` (project `flicker`, plus the universal `general` rules). Architecture,
specs, decisions, invariants, working conventions, and history all live there as linked
entries. "Where's the spec / doc / rule for X?" resolves to an MCP entry, never a file.
**Before creating any file, module, or crate:** grep the repo *and* query
`memory_coderules` + `memory_search`; if either surfaces a match, extend it if possible,
if not, surface a question to the operator.
**Store durable knowledge only in MCP** (`memory_store` + `memory_link_add` — linked
entries, not prose). Never write `docs/*.md` or any local `.md` design/memory file.
**Memory titles are NAMES, not claims — hard cap 256 chars, aim under 100.** A title is a
short noun phrase a search list can scan; the sentence, date, attribution, qualifiers and
detail go in `body`/`tags`. Write the body first, then title it. Over the cap the DB rejects
the row and the whole call is wasted. Same for `source` (128) and `tags` (512).

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
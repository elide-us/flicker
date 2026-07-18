# II. Dungeon Maker's Design

This book defines the Dungeon Maker's half of Prism: the modular template building system, the modular boss builder, and the item creation systems.

> **Provenance.** This book was drafted primarily from an early exploratory ChatGPT session on the Dungeon Maker system (source material, not canon), since **fully absorbed into this book and removed** — its content is preserved here and in the `prism-bible-recompile` ledger. Where that source conflicted with seed canon or Book V's Canon Rulings, the rulings won. Content below is Canon where backed by a ruling or ledger decision; everything else is marked **[PROPOSED]**, **[OPEN QUESTION]**, or **[STUB]**.

> **Terminology — RESOLVED (2026-07-04, ledger 500F8438).** **Dungeon Maker** (abbreviated **DM**) wins platform-wide. "Dungeon Master" is dropped — the term sits too close to TSR/Wizards of the Coast's Dungeons & Dragons trademark. This book uses Dungeon Maker throughout, as it always did.

---

## Design Philosophy

Canon (Seed canon 7, ledger C91261FB):

- Prism has two player populations that never directly interact: **Dungeon Makers** and **adventurers**.
- Dungeon Maker gameplay is an RTS/base-builder/tower-defense game. Adventurer gameplay is a classic MMO.
- The coupling between the two is **asynchronous**: dungeons are built, published, discovered, and run — never a live head-to-head.
- The point of the design is player-generated content at a scale no development team can match.

The Dungeon Maker path is not a housing system or an instanced dungeon generator. It is a full progression game with its own economy, advancement trees, resource requirements, and long-term goals: Dungeon Makers create the challenges, rewards, and destinations that drive adventurer activity, and a Dungeon Maker's success is measured by attracting repeated visitation, not by preventing completion. Players generate content for one another rather than relying exclusively on developer-authored experiences. *(Source framing, consistent with canon.)*

**The interaction boundary — resolved (2026-07-04, ledger A2596715); Seed canon 7 stands.** The two populations never *directly* interact, and the coupling remains asynchronous and indirect — but the shared **world** is a single real-time simulation, and that is where the two loops meet. A Dungeon Maker operates through a fixed-camera, voxel-slicing RTS view of their own domain (see The Template System and Imps and Resource Logistics), issuing map commands and seeing only what is visible in the domain they control. Their minions are only *secondarily* guided — the Maker clicks a location, and the minion goes and does the literal work. An adventurer moving through the world meets those minions, traps, and monsters as **world objects**, and can kill a minion to disrupt a build in real time — but is interacting with the *world*, usually without ever realizing a Maker stands behind it, never with another player head-to-head. The boundary is live but world-mediated: the two populations' activities overlap in real time; the players themselves do not. The tower-defense frame defends the dungeon's world-objects (minions, traps, monsters), never a live head-to-head between players.

The one acknowledged crack — not a sanctioned path — is the founding **wisp**: a Maker who has no dungeon yet wanders the world as a minion spirit (see Imps and Resource Logistics), and players will recognize a lone wisp for what it is and may camp or harass it. This is a possible seam in the asynchronous wall, not a mode of play and not an invalidation of the rule; the wisp can flee and rematerialize elsewhere. (Ledger A2596715; Seed canon 7 preserved.)

---

## The Template System

Canon (Seed canon 8, ledger 49A3A9D6): dungeons are assembled from modular template pieces with **constrained connection points** — traps, rooms, and fills attach only at defined sockets, and fills are bounded. Fairness is enforced at the construction grammar, never policed after the fact.

Template pieces represent dungeon components. The source lists as examples: hallways, chambers, treasure rooms, boss arenas, trap complexes, secret passages, puzzle rooms, monster nests, ritual chambers, and environmental hazards. These pieces are the building blocks from which larger dungeons are assembled; as a Dungeon Maker advances, more sophisticated templates unlock, allowing more complex layouts, stronger encounters, and more specialized themes. The template system exists so that Dungeon Makers work at the level of design and progression rather than piece-by-piece sculpting. *(Source.)*

*[Fill and material pieces draw on Book III's material vocabulary; how materials are stored (voxel containers vs the engine's material ledger) is an open fork that touches Book II's fill pieces. Ledger 9C677816.]*

**The design team authors the shapes; the player fills them (2026-07-04, refined by Elideus 2026-07-13; ledgers 66083A94, A9DB81C8).** The design team provides **space layouts with constraints** — the *shapes* of rooms and the menu of **what may connect to what and how** (branching halls, loops, dead-ends, rooms opening into one another) — in the spirit of the **solid pass (blockout) of a modern level-development pipeline**: the designer draws the volumes, the player fills them. Within those shapes a Dungeon Maker has **real creative freedom** — filling with their own materials and detailing the space however they see fit, *the painter working over the scene the inker has drawn* — including large, open, and **multi-level spaces** (combat arenas and open halls spanning several floors, in the register of Landmark builds). Filling is done through **voxelmancy** — the Maker's CSG (constructive solid geometry) editor — where each provided shape exposes what is *fillable* and what is *locked*. The shape geometry is what the engine reads for **collision**, which is what makes the construction-grammar fairness of Seed canon 8 enforceable: a build is legal because its shapes and connections are legal, not because it is policed after the fact. *(The design team authors the raw shape vocabulary; players compose and fill it, they do not sculpt raw templates from nothing.)*

**The constraints are few and mechanical (Elideus, 2026-07-13, ledger A9DB81C8).** Beyond the shape-and-connection vocabulary, a Maker is bound mainly by two things: **build-time validation logic** and a **budget**. Validation is structural — a space that must be traversable **cannot be saved unless the design validates as passable** (you cannot wall off a required route); the editor refuses an invalid save the same way the socket grammar refuses an illegal trap, and passability is the named example of a wider class of save-time checks. The budget is a **cell and room-type allowance unlocked with experience** (The Ten Tiers, Technology Trees) — how much you can build, and which room and connection types you have. Aside from those, the design intent is deliberately **few restrictions**: build whatever detail you like within the shapes you are given.

**Dungeon scale.** A Maker chooses a scale when founding a dungeon — for example a **16-voxel grid**, meaning each template block spans sixteen voxels. Larger blocks unlock with tiers, up to a **64-voxel** scale reserved for the largest works; a top-tier boss room must reach that scale to hold something the size of a dragon.

**[PROPOSED] — how the fill behaves (recovered draft, 2026-07-13, `sources/legacy-dungeon-maker-book2-recovered.md`).** Two textures from the recovered Book II sharpen the fillable/locked model above. First, the editable regions are **bounded to specific zones** of a template — a template might expose only its top and bottom few layers, or its corner columns, as fillable, and lock the rest — which is exactly how the grammar prevents invalid layouts (blocked halls, unreachable exits) without after-the-fact policing. Second, voxelmancy sculpts **deformable, vector-defined voxels**, not simple cubes: a voxel carries shape data (a centre and a vector toward one corner) so a voxel and its neighbours resolve into curves, slants, and rounded geometry — the "clay" quality (the recovered draft's "ClayEngine") that lets block-based templates read as organic stonework. The voxel-shape geometry itself is a render/engine concern (Seed canon 1); the design truth is that fills are bounded and surfaces can be organic. *(Superseded: the recovered draft's 40³–160³ template-size ladder is not carried — the canon dungeon scale is the 16→64 voxels-per-block above; ledger 66083A94.)*

**What voxelmancy and the tech tree afford.** Because the template drives collision, surface treatments become gameplay. A **secret door** is made by covering a passable hole with a special "**cloth**" enablement (a tech-tree unlock) so the surface reads as solid while players can still navigate through it. **Surface physics** — slow, slippery, and their kin — are likewise template and tech options a Maker can apply.

**Co-op editing obeys the owner's rights grants (Book I, Account, Friends, and Co-op Building).** Voxelmancy on shared ground is not open to all comers: a claim or build's owner grants other accounts graduated rights — full **edit**, NPC-**roster operation**, or minimal **interact** — and the construction grammar's fairness still binds every editor regardless of who is holding the tools.

---

## The Trap System

Traps are the construction grammar's sharpest expression of Seed canon 8 (ledger 49A3A9D6): they are **never built freeform.** Each room template exposes designated **trap sockets** (the recovered draft's "trap nodes") — predefined attachment points into which a Maker slots a modular trap, and nowhere else. Socket count and complexity scale with template tier (Tier 1's Hall offers one socket; Tier 2 reaches five — The Ten Tiers), so the most dangerous traps are reachable only by advanced Makers, and a layout is fair because its sockets are fair, not because it is policed after the fact. *(Recovered draft archived at `sources/legacy-dungeon-maker-book2-recovered.md`; the detail below is [PROPOSED] elaboration of the canon socket concept.)*

**[PROPOSED] — the socket declaration.** Each socket declares what it will accept, so a template constrains not just *where* traps go but *which*:

- **Attachment type** — floor, ceiling, wall, or hidden.
- **Allowed trap class** — a socket may take only certain kinds (projectiles only; no liquids).
- **Trigger compatibility** — which triggers the socket supports: step-plate, line-of-sight, or proximity.

**[PROPOSED] — triggering.** The default is one-to-one: one trigger fires one trap. Tech unlocks widen it to **one-to-many** (one trigger, several traps), **delay chains**, and **randomized activation**. As everywhere in the kit, triggers are **template-governed and pre-authored** — a Maker selects from authored behaviors and **cannot script new logic** (the same no-scripting principle as Story Content Packs), which is what keeps triggering inside the fairness grammar.

**[PROPOSED] — trap archetypes.** Five broad, scalable classes; their effects should resolve to Book I / Book IV's status-effect taxonomy (stun, snare, poison, fear, and kin) rather than bespoke effects:

1. **Projectile** — arrows, darts, spears, flame jets; single / burst / cone / fan; upgradable to homing, elemental, or volley.
2. **Impact** — spikes, hammers, crushers; drop-from-above or rise-from-below; pairs with **bait triggers** (false doors, loot chests).
3. **Environmental hazard** — liquids (acid, lava, slime, oil), solids (sand floods, boulder drops), gases (poison, hallucinogen, sleep mist).
4. **Movement** — pusher plates, moving walls, wind gusts; platform removal, collapsing pits; the puzzle-integration workhorse.
5. **Psychological** — misdirection through layout, lighting, and sound; false triggers and aesthetic cues that instill caution or fear (the mundane end of the register Book VII's Inverted phenomena occupy).

**[PROPOSED] — trap progression.** Trap classes unlock along the **Trap Engineering** tech tree (Technology Trees), gated by completions, roughly: Tier 1 basic spikes and projectiles; Tier 2 hidden traps, delayed triggers, pitfall combinations; Tier 3 liquids, corrosives, chain reactions; Tier 4+ elemental traps, puzzle-linked mechanisms, multi-room effects; Tier 8+ rolling boulders, dungeon-reset mechanisms, boss-arena modifiers. Every trap module carries a **visual preview** in the voxelmancy editor and, crucially, **collision and timing validation** — the construction-grammar guarantee that a trap interacts fairly, the build-side face of the adventurer fairness law (Book I: "if the player dies, they missed something"). Balance constraints bound each module so no socketed trap is unfair by construction.

---

## Incentive Structure

Canon (Seed canon 8, ledger 49A3A9D6): uncompletable dungeons yield **no XP and no traffic**. Trolling is expected at the margins and starved economically, not moderated.

Dungeon Makers gain experience when their dungeons are completed; that experience funds advancement through tiers and technology trees. *(Source, consistent with canon.)* The source frames the resulting economy consistently: success is repeated visitation; reputation is a resource; better dungeons create better rewards, better rewards attract more adventurers, and more adventurers create more opportunities for Dungeon Maker progression.

---

## The Ten Tiers

Dungeon Makers advance through **ten tiers** of dungeon complexity, each a significant increase in capability, complexity, and influence. Early tiers focus on basic dungeon functionality and survival; by the highest tiers, a Dungeon Maker is operating an entire ecosystem rather than a single dungeon. Advancement unlocks, in aggregate: new construction templates, additional room types, larger dungeon sizes, stronger monsters, advanced traps, specialized themes, improved resource gathering, enhanced loot creation, and new technology branches. *(Source.)*

Canon constraint (Seed canon 6, ledger 8FB95281): celestial events are global modifiers across **all** tiers and regions — never tier-exclusive or endgame-only bonuses. Scarcity and abundance are deterministic and foretold in the sky.

**Tiers are a linear *capability* ladder; the layout within is flexible (2026-07-04, refined 2026-07-13; ledgers 66083A94, A9DB81C8).** Advancement through the ten tiers is linear as a *progression* — each tier a larger cell/room-type budget plus new unlocks — but **within that budget the layout is genuinely flexible**, per the solid-pass model in The Template System: the design team supplies the room shapes and connection options, and the Maker composes and fills them freely, bounded only by the budget and by save-time validation (passability and its kin). *(This corrects an earlier "layout freedom is deliberately small / not free-form" framing: the freedom lives in filling and connecting the provided shapes — including multi-level combat and open spaces — not in a tightly-scripted layout.)* Every tier is still conceived the same way — a budget of rooms, trap allocations, and loot points, with a **Dungeon Heart** in the boss room. On advancing a tier, the previous tier's boss room becomes a **loot camp**, and the Heart migrates outward to the new boss room.

- **Tier 1** unlocks three parts: **Door, Hall, Heart.** The Door is crafted in voxelmancy and carries a player-destructible door piece. The Hall attaches to the Door at its one connection point and offers one trap socket. Traps are gated by experience and tech — in a desert fire zone, the first is a simple, easily-disarmed, low-damage exploding fire box. Beyond the trap lies the boss room, whose **Tier 1 boss is a Normal-scale creature** (not a Weak one) dropping minor but *consistent* loot — a **Fire +1 dagger** off a T1 Goblin boss, say. The intended clear is a party of **3–4 around level 10** (monsters scale harder than players).
- **Tier 2** unlocks two more halls, a room, two more doors, and a third trap — **five trap sockets in all** — and the pattern keeps compounding through the higher tiers. The top tiers call for **64-scale** templates and a boss room large enough to hold a T10 terror.

*(Party-scale note: the 3–4-at-level-10 T1 target sits a little under the general dungeon tuning of 5–7 players, ledger CA50B875; T1 likely just runs smaller. Flagged, not yet reconciled.)*

---

## Technology Trees

Beyond tier advancement, Dungeon Makers gain access to technology trees — specialized areas of expertise through which a Dungeon Maker develops a distinct identity. Different Dungeon Makers pursue different technological paths, producing varied dungeon experiences across the world. *(Source.)*

**[PROPOSED]** Example branches — source-work only; the source itself says "examples may include":

- **Monster Mastery** — monster quality, diversity, behavior, and specialization. Ranking this tree is what gates a DM's access to a bestiary creature's Strong power tier (Weak/Normal are default-available; Boss is gated separately, by the same top-tier/deep-tech-tree wall as raid-zone and Advent-gate construction) — see Book VII's Power Tiers section, ledger 24DD9710.
- **Trap Engineering** — advanced traps, environmental hazards, and defensive systems.
- **Arcane Research** — magical enhancements, enchantments, and unusual dungeon mechanics.
- **Resource Management** — efficiency of minions, logistics, harvesting, and material conversion.
- **Relic Crafting** — creation of rare equipment, magical artifacts, and unique rewards.
- **Theme Expansions** *(recovered draft, 2026-07-13)* — thematic content trees that skin a dungeon's identity (the recovered Book II names Undead, Eldritch, Clockwork, and Fae as examples). To avoid a second, competing creature taxonomy, these should **map onto Book VII's colour sets and Inverted** rather than stand alone — e.g. Undead → Black/Death, Eldritch → Inverted, Clockwork → the Golems / Ancient-Civilisation archetype — and dovetail with Story Content Packs for their narrative dressing.

---

## Imps and Resource Logistics

Dungeon Makers maintain their dungeons through resource collection. Specialized servants — imps and other minions — gather materials from the surrounding world and haul them back to the dungeon. These resources are consumed by construction, expansion, maintenance, monster support, trap creation, and item production. *(Source.)*

The gathering process is not hidden from the world: observant adventurers may discover resource routes, track minion activity, or follow signs of dungeon expansion to uncover previously unknown dungeon locations. Visible logistics are the discovery mechanism that ties the Dungeon Maker ecosystem into the adventurer experience. *(Source.)* Adventurers **can** interfere with minions live, killing them to halt a task — but as world-mediated interaction, never a live head-to-head with the Maker (interaction boundary resolved, ledger A2596715; Design Philosophy).

**Minion labor is literal, at 1:1 voxel fidelity (2026-07-04, ledger 66083A94).** Nothing a Maker orders happens instantly. Order a cluster of dirt dug out and the minions travel to it and dig it voxel by voxel, hauling the spoil to containers or processing. Order something built and they first dig the space, then fetch or craft the material the applied template calls for, then place that voxel. The Maker directs by clicking locations on the sliced RTS view (The Template System); the minions do the physical work. Killed minions respawn and fall back to other jobs — every step is a real action in the world, which is precisely why an adventurer's presence can perturb a build in real time.

**The founding wisp.** Before a Maker has a dungeon at all, they are a wandering minion — a **minion spirit, a wisp**. Founding a dungeon begins with the wisp finding an allowable wall and materializing to start digging. A lone wisp is recognizable for what it is and can be camped or harassed — the one acknowledged crack in the asynchronous wall (Design Philosophy) — but a harried wisp can flee and rematerialize elsewhere.

---

## Placement and Monster Recruitment

Canon (Elideus, 2026-07-02; ledger B4F9A3AC): specific biome types are **seeded with specific kinds of creatures**, and on the DM-vs-adventurer scale **certain monsters can only be recruited in certain areas**. **Picking where to place your dungeon matters** — siting is a strategic decision, because the biome decides the recruitable bench.

**[PROPOSED] The bench is not fixed, because the biome is not fixed.** A biome is the emergent output of the living ecological simulation (ledger 10B27578: water + nutrients → plant layer → insect layer → animal layer, spawns from detected conditions), running over a world the erosion and crust systems keep reshaping (Book III: Rivulets, the crust clock; ledgers F156C63C, 41EB4B47). As the biome around a dungeon evolves — succession shifting the trophic layers, erosion exposing, burying, or depleting the resource base, creatures migrating as conditions change — the recruitable bench and the strategic value of a site shift with it. Siting is a bet on a moving target, which ties this section to §Decay and Maintenance: the same biome evolution that moves the bench is the source of a live dungeon's decay pressure.

The bestiary itself lives in **Book VII: Bestiary**. Book VII's Monsters and Terrors are the game's dungeon-oriented, DM-recruited population; Animals are chiefly open-world ecological wildlife (Appendix E of that book) and are not the primary recruitment target, though nothing rules out a build using them.

**Recruitment gating — resolved (2026-07-04).** Learning is biome-gated; using is not. A Dungeon Maker learns a creature by first recruiting it from its native biome — the recruitment act itself is what unlocks that creature, and its Monster Mastery tech-tree branch, permanently. Concretely: building and operating a dungeon within a given biome is what unlocks the tech tree for that biome's creature sets and features in the first place — a Maker who has never sited a dungeon in Volcanic terrain has no path to the Volcanic bench's tech branch. Once a creature is learned, it joins the Maker's permanent roster; deploying it elsewhere afterward is gated only by the ordinary Monster Mastery/power-tier rules (Book VII, Appendix D — Weak/Normal by default, Strong/Boss by tech-tree rank), not by the biome of the build it's placed in. Siting is still a strategic decision (§ above) — it decides what's *learnable* here and now — but it does not lock a matured roster to one biome forever.

**Superseded — the "monster attraction" model (recovered draft, 2026-07-13, `sources/legacy-dungeon-maker-book2-recovered.md`).** An older Book II had Makers **attract monster factions by environmental theming** — fire-themed rooms drawing fire-lovers, moist decay drawing fungal beasts. That is **replaced by the recruitment model above:** the biome the dungeon *entrance* sits in decides the recruitable bench (Book VII, the bestiary — "the monster manual"), and a creature is learned by recruiting it there (ledger B1472A6A), not lured by décor. The only kernel that survives is that **the surrounding environment governs which creatures are available** — through the emergent ecological simulation (ledger 10B27578), not a theming-attraction table. (Elideus flagged this supersession on intake.)

**[STUB]** remaining: how placement interacts with discovery and imp logistics is still open; whether material/reagent gathering follows the same learn-by-biome pattern as creature recruitment is a live question (Book IV's Reagent Grades) — flagged, not answered here.

---

## Discovery, Advertising, and Reputation

New dungeons begin hidden from the wider player population. As adventurers discover, explore, and report on dungeon locations, information spreads: locations become known destinations and may appear on regional information boards, guild records, and maps. Dungeon Makers advertise their creations on in-game boards and compete for attention — a poorly designed dungeon is ignored; a well-designed dungeon becomes a destination with a steady stream of visitors. Reputation is a resource. *(Source, consistent with the incentive canon above.)*

**[PROPOSED] — in-game mystery, out-of-game community tools (recovered draft, 2026-07-13, `sources/legacy-dungeon-maker-book2-recovered.md`).** The recovered Book II splits discovery cleanly along the philosophy of ambiguity (ledger BFBE6085): **in-game keeps the numbers hidden** — no glowing markers, no map exclamations, no dungeon stat readouts; discovery is earned, and a returning adventurer receives only narrative feedback (the home-written Journal, Book I: *"your journey sharpened your senses"*). **Out of game**, a companion layer surfaces what the game hides — a shared **online map and message board** carrying discovered dungeons, completion rates, average clear times, ratings (fun / challenge), screenshots, reviews, and strategy, where Makers earn prestige and rankings. Internal stats (completion rate, time-to-clear, adventurer ratings) exist and drive the economy; players read them only through these external tools, never an in-game UI. This is the Dungeon-Maker face of the same companion-app social layer the adventurer side already carries (Book I, Account, Friends, and Co-op Building; ledger 045347FE).

---

## Decay and Maintenance

The world is designed around continual change: resources shift, materials become depleted, monsters migrate, structures decay, environmental conditions change. Dungeon Makers must continually adapt to remain relevant and competitive; the pressure prevents long-term stagnation. *(Source.)*

Dungeons function as living systems rather than static structures: monsters occupy habitats, resources are consumed and replenished, bosses require support structures, and expansion creates new maintenance requirements. Over time a dungeon develops an identity from its creator's choices — a militarized fortress, a site of magical experimentation, a specialist in rare crafting materials or legendary equipment production. *(Source.)*

**[PROPOSED] The driver of that change is the living biome.** The continual change above is not ambient flavor — it is the world simulation the dungeon sits inside reaching the dungeon: the ecological sim (ledger 10B27578) shifting which creatures and resources a biome supports, over terrain the erosion and crust systems keep reworking (Book III: Rivulets, the crust clock; ledgers F156C63C, 41EB4B47). Resource depletion and replenishment, monster migration, and changing conditions are that simulation's output. This binds maintenance to placement: as a dungeon's biome evolves, its recruitable bench (§Placement and Monster Recruitment) and material base move, and adapting to that drift *is* the Dungeon Maker's ongoing decay pressure — distinct from the abandonment-decay below (ledger 95EAF28D), which removes what no one maintains at all.

The same law governs the adventurer side: claims and settlements require ongoing upkeep even while actively played — maintenance costs, settlement NPCs doing repair work — and abandoned structures decay back into the world through the erosion system. Nothing persists that no one maintains. (Ruled; ledger 95EAF28D.)

---

## Story Content Packs

Canon (ruled 2026-07-01/02; ledger 95EAF28D; source archived at `sources/celestial-cosmology-session-2026-07-01.md`):

- Builders can unlock **specialized content trees** by invested points — explicitly **not by purchase** — granting curated sets of story content to deploy: webs of NPCs, quest parts, and items authored from the mythology (the scale of a self-contained storyline cluster with recurring NPCs across missions).
- Deployment is governed by **validation-as-authorship**: the content pack carries grammar constraints the builder must satisfy — in Elideus's words, rules like "if you [take] this particular content pack, you are not allowed to finish the ninth tier of a dungeon without having this kind of a room deployed that has this NPC sitting in it." The build validates only when the required interaction set is complete. This extends the fairness principle of Seed canon 8 — enforcement at the construction grammar — from spatial fairness to narrative completeness.
- The primitive set is **event-oriented discovery content, not fetch quests** ("I don't want to give people the ability to say, go collect ten seashells"): builders deploy opportunities players stumble upon, in the discovery-and-exploration register.
- Builders do not need to understand the story they deploy: they select storylines and enhancements that fit their theme, and **the lore rides the items and NPCs** — canonical mythology propagates through player-built dungeons by construction.

**[STUB]** The concrete toolkit — the quest-part building blocks, the constraint vocabulary, which storylines become packs — is undesigned. Open: how content-tree investment prices an authoring license; how packs interact with tiers (the tier-nine example implies gating); and where pack validation runs relative to the template grammar's socket validation.

---

## The Boss Builder

Book 0 names a **modular boss builder** as one of this book's three systems. The source supports: boss arenas and boss rooms as template pieces; stronger bosses unlocked by tier; "unique boss experiences" at the high tiers; and bosses requiring support structures within the dungeon's ecology.

**The builder mechanism — resolved (2026-07-04, ledger 66083A94/EBF514AA).** A boss is not a free-form, player-composed creature. It is a **recruited creature from Book VII placed in a designer-made, tier-scaled boss-room template**, at an elevated power tier: the Tier 1 boss is a Normal-scale creature standing in for the boss slot (a T1 Goblin boss dropping a Fire +1 dagger), and the ceiling rises with tier and dungeon scale up to a Boss-tier apex — a dragon in a 64-scale T10 boss room. The Maker's authorship is in *which* creature (gated by biome recruitment and the Monster Mastery tree, Book VII), *where* and at *what tier* it sits, and *how* the room and its support structures are built in voxelmancy — not in sculpting the creature itself. The Dracolich word-grammar (Book IV; the Vetala-on-Dracolich pattern, Book VII) remains a **naming and identity** convention for composed legendaries, not the boss-construction interface. (Closes ledger EBF514AA for the builder mechanism; the deterministic-naming pattern stays [PROPOSED] under Loot Creation.)

**Superseded — the free-form boss composer (recovered draft, 2026-07-13, `sources/legacy-dungeon-maker-book2-recovered.md`).** An older Book II offered a **fully customizable boss** assembled from a base archetype (Brute, Mage, Trickster, Summoner), traits (Teleport, Enrage, Phases, Summons), an elemental affinity, and a bespoke loot table. That composer is **not the model** — a boss is a recruited creature, not a built one (above). What survives is **vocabulary only**: the trait list (teleport, enrage, phases, summons) is a **[PROPOSED] candidate for the Boss-tier signature mechanics** every Boss-tier creature carries (Book VII, Power Tiers, ledger 24DD9710), and per-encounter **loot-table design** is already the Maker's job (Loot Creation). The Maker authors *which* creature, *where*, at *what tier*, and *how* the room is built — never the creature's stat block.

---

## Loot Creation

Reward generation is a core Dungeon Maker responsibility. Rewards are tied to the progression and capabilities of the Dungeon Maker who created them rather than to static loot tables; as a Dungeon Maker unlocks new technologies, materials, enchantments, and techniques, the ceiling of producible rewards rises. *(Source.)*

**The adventurer side plays off this system (Elideus, 2026-07-05).** What a Maker's dungeon *drops* is not the end of an item's life: an adventurer can **level a dropped item up over time**, raising a modest low-tier find into something worth carrying at level 10 — the same item, grown, at a large investment of time, money, mana, and research. It is a Long-Term Objective and a real trade (which item is worth *that*?). The mechanics of this adventurer-side enhancement live in Book I (Weapons and Affinity; Long-Term Objectives); this section owns the Maker-side creation the adventurer plays off of.

The source session ended with exactly this item open: "How magical item creation actually works: crafted directly, procedurally generated from components, assembled from Words of Power, derived from materials, or some combination" — identified as the next major design document. The stack below is a candidate answer, not a ruling.

### The Artificing Stack **[PROPOSED]**

Source of the stack: Elideus, recompile brief 2026-07-02. Each rung cites its Book IV basis; rungs whose application to items appears nowhere in Book IV are flagged as such. (Basis map in ledger C1AF172E.)

1. **Material.** An item begins as materials — Book III's element, compound, and mineral tables; the source notes' elemental system generates what the world offers.

2. **Noun-residency.** Book IV: "Every interactive object in the game world is assigned a magical name (**noun**)" — and material nouns exist in its dictionaries: **Ferrum** (Metal, gloss broadened 2026-07-04), **Saxum** (Stone, now exclusive of Terra, ledger 44F17D1E), **Lignum** (Wood), **Crystallum** (Crystal — "often used in enchanting, crafting, or reinforcing magical objects"). **[PROPOSED]**: the nouns resident in an item's materials determine what magic the item can carry. Book IV grounds the residency; the consequence is brief-sourced.

3. **Artificing.** Book IV's Magical Domains include **Artificing** — "Imbuement of items with magical properties." **[PROPOSED]**: artificing is the act that binds words to the item.

4. **Adjective-quality.** Book IV's adjectives grade intensity and scale — **Vas** (Greater) / **Bet** (Lesser), **Fortis** (Strong) / **Debilis** (Weak), **Magna** (Large) / **Parva** (Small). **[PROPOSED]**: adjectives serve as quality grades on an imbued property. Basis gap: Book IV applies adjectives to spells, never to item grades — the application is brief-sourced only. The adjective slot's collisions (Altus/Altior and Infimus/Humilis duplicates, and Umbra in two grammar slots) were resolved 2026-07-04 (ledgers 44F17D1E, 94774F28) — that blocker is cleared; the item-grade application itself remains brief-sourced.

5. **Shape-proc.** Book IV's shapes — **Orbis**, **Serratus**, **Rectus**, **Chevronis**, **Lunaris**, **Undula**, **Spiralis**, one per school — define how magic interacts with space. **[PROPOSED]**: a shape gives an item its triggered (proc) effect form. **[OPEN QUESTION]** — basis gap: Book IV never attaches shapes to items. It ties shape access to school synergy through a ring progression, now defined as a per-school mastery structure (ledger B92EE74A, resolving 63B28A6B) — but whether an item's shape-proc is constrained by its maker's ring standing, and whether a shape-proc marks the item with a school identity, remains unresolved. (Ring-gated **reagent grade**, same ledger, is a plausible second constraint on artificed items — a focus or talisman is itself a candidate reagent-substitute, per Book IV's Reagent Grades — but this is not ruled here.)

### Deterministic Legendary Naming **[PROPOSED]**

Basis: Seed canon 10 (ledger C936766F) — platform-wide, the same composition yields the same identity (UUID5 from a locked namespace); the same principle is the intended pattern at the loot layer, where it is [PROPOSED], not ratified.

The pattern is the **Dracolich composition rule**: as Book IV composes **Vas** + **Draco** + **Mortuus** into a Dracolich, a legendary item's name derives deterministically from what it is made of. Two items of identical composition are the same legendary — same name, same identity, everywhere, always.

Unresolved dependency (ledger 67DB3F76): the exemplar's constituent words — **Draco** and **Mortuus** — are defined in no Book IV dictionary, and their grammar slots are unassigned. The identity math requires ratified constituent words; until the dictionaries close that gap (and the slot collisions noted above), deterministic naming has a canon-grade principle and source-work vocabulary.

> **[OPEN QUESTION]** Whether the artificing stack is the whole answer to the source's open item or one component of "some combination" (crafted directly / procedurally generated from components / assembled from Words of Power / derived from materials) remains the fork the source itself left open.

# IV. Magic System Design

Our game's magic system offers a dynamic, flexible spellcasting experience built on a procedural framework without static spells. Players input magical words—**nouns**, **verbs**, **adjectives**, **adverbs**, and **shapes**—to express their intent, which the server interprets to generate real-time interactions.

*[EDITORIAL — 2026-07-02 realignment, ledger FF1F7955: this book's "school" language predates the ruling that each color fields two schools (one per aspect — fourteen schools among the seven worlds) and that philosophies belong to colors, not schools. Its color-as-school phrasing is retained pending the deferred engine-side reconciliation (ledger 33173716); read "school" below as "color" unless an aspect is named.]*

### **Core Mechanics**

Every interactive object in the game world is assigned a magical name (**noun**) that defines its nature and how it can be manipulated. For example, a fire elemental responds to **"Flam"** (Fire), while a stone wall reacts to **"Terra"** (Earth).

Spells are crafted by combining nouns with verbs and modifiers. For instance, **"Vas Flam Vectus"** translates to **"Greater Fire in a Straight Line"**, manifesting as a firewall. The server interprets these inputs along with player actions to determine spell effects.

### **Spell Research and Creation**

Players cannot cast spells by simply inputting words in real time; they must first **research and create** spells. This involves discovering valid combinations of magical words to form functional spells, which are then **memorized** for later use. This process encourages exploration and careful crafting of spells. For example, to cast **"Vas Flam Vectus"**, a player must first research and validate this combination within the game mechanics.

**Clarification (2026-07-05): the word-grammar is the language, not an invitation to invent from nothing.** The fourteen schools have stood for roughly **one hundred thousand years** (Book VI), and each has an established canon of actual spells that already exist within the fiction — a **Grimoire** (Book VIII, scoped below), not a blank combinatorial space. "Research and create" describes the player's *discovery* of an existing spell — the same word-grammar the game exposes for flavor and for how a discovery is presented to the player — not literal invention of a spell nobody in the world has ever cast. A mage researching "Vas Flam Vectus" is recovering a working already known to that school's tradition, not authoring a new one. This does not change any mechanic above — the noun/verb/modifier/shape grammar is still exactly how a spell is expressed and processed — it changes what "research" means in-fiction: excavation of the school's grimoire, not free invention. **[OPEN QUESTION]** Elideus flagged that unlocking a spell could carry gameplay requirements beyond simply reaching the school that holds it — what those requirements are is undesigned; see Book VIII.

### **Multiple Paths to the Same Effect**

Different magic schools can achieve similar results through their unique themes. To extinguish a flame:

- A **Red Magic** mage might use **"Ex Flam"** (*Banish Fire*).
- A **Black Magic** practitioner could cast **"In Umbra"** (*Create Shadow*) to smother it.
- A **Blue Magic** user might employ **"In Aqua"** (*Create Water*) to douse the fire.

This allows players to solve problems in ways that align with their magical affinities, promoting diverse gameplay experiences.

### **Interaction with the World**

Each spell follows an order of operations:

1. **Core Effect:** Determined by nouns and verbs.
2. **Modifiers:** Adjectives and adverbs adjust characteristics like intensity and duration.
3. **Shape Definition:** Shapes like **"Vectus"** (Straight Line) or **"Spiralis"** (Spiral) define spatial interaction.

This system ensures consistent processing and limitless possibilities, while requiring players to research and validate spells for balance.

### **The Constructor Kit Philosophy**

Embracing a **constructor kit** approach, we empower players and Dungeon Makers to shape almost every aspect of the game world:

- **Crafting:** Players use procedural tools to create unique weapons and armor.
- **NPC Behavior:** Customizable modules and animations allow NPCs to interact in unique ways.
- **Environmental Interaction:** Players influence how NPCs and objects engage with the environment, leading to dynamic gameplay.

By fully embracing procedural generation, we offer endless possibilities for creativity and experimentation, allowing players to shape their ever-evolving world.

The magic system is built on **Words of Power**, which allow players to cast spells, interact with objects, and shape the world using a combination of **nouns**, **verbs**, **adjectives**, **adverbs**, and **shapes**. These words form the core of how players interact with the game’s magical systems, and their combinations create diverse spell effects and interactions.

### **Nouns**
Nouns define the objects, elements, and forces that the magic system can interact with. Every object in the world that is magical or can be affected by magic is associated with one or more nouns.

- **Vita** – Life  
  Represents all living things or forces that embody vitality and growth. Often used in healing or life-giving magic.

- **Lux** – Light  
  Refers to sources of light, whether natural or magical, and can be used to create illumination or banish darkness.

- **Flam** – Fire  
  The element of fire, heat, and combustion. It is used to describe destructive or warming forces.

- **Aqua** – Water  
  The element of water. Aqua is used in spells related to fluidity, healing, and the force of rivers and seas.

- **Terra** – Earth  
  Represents solid ground and soil. Terra is invoked in spells dealing with nature, stability, and physical strength. *(Narrowed 2026-07-04, ledger 44F17D1E: dropped "stone" from scope — that belongs to Saxum.)*

- **Aer** – Air  
  The element of air, wind, and the sky. Aer can be used for spells related to speed, movement, and unseen forces.

- **Mors** – Death  
  Represents death, decay, and the ending of life. Mors is essential for necromantic spells or anything associated with finality.

- **Kaos** – Disorder  
  The embodiment of chaos, unpredictability, and randomness. Kaos is used in spells that disrupt order or create confusion.

- **Vis** – Force  
  A general term for energy or applied physical force, excluding gravity specifically (see Grav). Vis can be used to describe the application of power in various contexts.

- **Grav** – Gravity  
  Specifically refers to gravitational and kinetic force — weight, momentum, and attraction. Grav can be invoked for spells manipulating momentum or attraction. *(Resolved 2026-07-04, ledger 44F17D1E/94774F28: re-glossed from "Energy" to "Gravity," and Vis narrowed to exclude it, closing the overlap.)*

- **Ignis** – Heat  
  The essence of warmth and heat, distinct from flame. Ignis is used to raise temperatures or imbue warmth without combustion.

- **Umbra** – Shadow  
  Represents darkness and shadow, including the absence of light or the concealment of forms. Umbra can be invoked in stealth or shadow-related spells. *(Resolved 2026-07-04, ledger 94774F28: Umbra keeps the noun slot; the adjective "Dark" is now Ater.)*

- **Glacies** – Cold  
  Refers to freezing temperatures, frost, or ice. Glacies is essential for spells that slow, freeze, or lower temperatures.

- **Nox** – Poison  
  Represents toxic substances, venom, and corruption. Nox is used in spells that damage over time or create harmful effects.

- **Sanguis** – Blood  
  Refers to the blood that runs through living creatures. Sanguis is often invoked in life-stealing or blood-related rituals.

- **Crystallum** – Crystal  
  Represents minerals and gems, often used in enchanting, crafting, or reinforcing magical objects.

- **Mentis** – Mind  
  Represents thought, intellect, and psychic forces. Mentis is used in spells that influence mental clarity, control, or confusion.

#### Additional Common Objects
- **Gladius** – Sword  
  A blade or sharp-edged weapon. Represents martial strength or cutting power.

- **Clava** – Club  
  A blunt weapon, often used for bashing or concussive force.

- **Falcis** – Scythe  
  A curved blade, traditionally used for harvesting but in magic, often associated with death or reaping.

- **Malleus** – Hammer  
  A blunt tool used to shape or destroy. In magic, it represents force and the power of crafting or breaking.

- **Arcus** – Bow  
  A ranged weapon, symbolizing precision and distance in spellcasting.

- **Scutum** – Shield  
  Represents protection and defense. Invoked in spells related to guarding, blocking, or absorbing damage.

- **Ferrum** – Metal  
  A term for metals generally, particularly iron. Ferrum is used in spells related to strength, resilience, and crafting. *(Broadened 2026-07-04, ledger 44F17D1E: gloss now matches the description.)*

- **Saxum** – Stone  
  Represents stone or rock, often invoked for solid, immovable objects or defenses. *(Resolved 2026-07-04, ledger 44F17D1E: Saxum now owns stone/rock exclusively; Terra's scope was narrowed to drop it.)*

- **Lignum** – Wood  
  Represents organic, woody material. Used in spells for nature, construction, or growth.

### **Verbs**
Verbs describe the actions performed in spells and magical interactions. They form the foundation of how nouns are manipulated.

- **Kal** – Summon  
  To call forth a creature, object, or force. Often used in conjunction with nouns to summon specific elements or beings.

- **Por** – Move  
  To cause motion or displacement. Por can be used to move objects, elements, or even people.

- **Rel** – Change  
  To alter or transform something. Rel is essential in spells that modify existing objects or beings.

- **In** – Create  
  To bring something into existence. Used in conjuration or creation spells.

- **An** – Negate  
  To cancel or undo. An is used in spells that dispel, counter, or neutralize effects.

- **Ex** – Free or Banish  
  To release something from confinement or to drive something away. Often used in exorcism or banishment spells.

- **Sanct** – Protect  
  To safeguard or shield. Sanct is used in defensive or protective spells.

- **Uus** – Raise  
  To elevate or bring to life. Uus is often invoked in resurrection or elevation spells.

- **Des** – Lower  
  To reduce or bring down. Des is the opposite of Uus, used to diminish or decrease.

- **Profan** – Curse  
  To place a harmful or malevolent effect on something. Profan is used in dark magic, hexes, and curses.

- **Jux** – Capture  
  To trap or imprison. Jux is invoked in binding or capturing spells.

### **Adjectives**
Adjectives modify the noun or spell, adding descriptive elements like size, intensity, or position.

- **Vas** – Greater  
  Describes something stronger or more powerful. Used to intensify a spell’s effect.

- **Bet** – Lesser  
  Describes something weaker or smaller. Used to reduce the power or size of an effect.

- **Altus** – Upper  
  Refers to something above or higher in position. Stack with Vas/Bet for degree (e.g. "Vas Altus" for much higher) rather than a separate comparative word.

- **Infimus** – Lower  
  Refers to something below or lower in position. Stack with Vas/Bet for degree, as with Altus.

- **Celer** – Swift  
  Describes speed or quickness.

- **Lentus** – Slow  
  Describes something that is slow or delayed.

- **Lumen** – Bright  
  Refers to something with high luminosity or brightness.

- **Ater** – Dark  
  Describes something with low light or shadow. *(Resolved 2026-07-04, ledger 94774F28: renamed from Umbra, which stays a noun; Ater — Latin for a dire, ominous black — takes the adjective slot.)*

- **Fortis** – Strong  
  Describes great physical or magical strength.

- **Debilis** – Weak  
  Describes something fragile or easily broken.

- **Magna** – Large  
  Refers to large size or scale.

- **Parva** – Small  
  Refers to something of small size or lesser extent.

- **Glacialis** – Cold  
  Describes freezing or cold temperatures.

- **Calidus** – Hot  
  Describes warmth or heat.

### **Adverbs**
Adverbs modify verbs, adding details about how actions are performed.

- **Cito** – Quickly  
  Describes a fast action.

- **Tarde** – Slowly  
  Describes a slow action.

- **Magnopere** – Greatly  
  Describes something performed to a large extent.

- **Parvopere** – Slightly  
  Describes something performed to a small extent.

- **Diu** – Lastingly  
  Describes an effect that endures over time. *(Resolved 2026-07-04, ledger 94774F28/44F17D1E: replaces "Durare," which was a verb infinitive misplaced in the adverb slot, with the true Latin adverb of duration.)*

- **Breviter** – Briefly  
  Describes a short-lived action. *(Resolved 2026-07-04: replaces "Breve" — properly formed via the same -iter pattern as Fortiter/Debiliter below — and absorbs the now-redundant "Brevis.")*

- **Fortiter** – Strongly  
  Describes a powerful or intense action.

- **Debiliter** – Weakly  
  Describes a weak or fragile action.

- **Prope** – Nearby  
  Describes proximity to the caster or object.

- **Superior** – Above  
  Describes something happening above or higher up.

- **Inferior** – Below  
  Describes something happening below or lower down.

- **Longe** – Distant  
  Describes something far away. *(Resolved 2026-07-04, ledger 94774F28/44F17D1E: re-glossed to its true classical meaning — "far off" — absorbing the now-redundant "Longius"; duration is Diu's job, not this word's.)*

- **Iterum** – Repeatedly  
  Describes an action done multiple times.

- **Semel** – Once  
  Describes something done a single time.

### **Shapes**

Shapes are an integral part of spellcasting, defining how the magic interacts with space and direction. **Shapes are not ring-gated** (ruled 2026-07-05): shape unlocking was always going to be problematic in gameplay, and with the Grimoire established (Book VIII) there is no need for a gated shape grammar — a spell is excavated whole, its shape baked in. The shape **vocabulary** stays spoken and **expands broadly**: the seven traditional school figures below, and beyond them both abstract geometry and concrete forms — spiral, crescent, chevron, but also wall, arrow, ballistic arc, slice, and on — coined Latin-esque per the word standard (ledger AA124D5D). What the synergy system gates is **school access**, not shapes (see the ring anchors below).

**Rings are a magic-mastery structure internal to this book** — a per-school milestone in the Words of Power research track, not a Book I character-progression tier (resolving fork 63B28A6B in favor of Option B; ledger B92EE74A). Ranking up a school's ring grants three things at once: **vocabulary breadth** (new nouns/verbs/adjectives/adverbs in that school become researchable), **grammatical complexity headroom** (how many modifiers may stack in a single researched spell string), and **reagent grade** (see **Reagent Grades**, below). Cross-school synergy access at the 4th and 7th ring, described next, is one instance of this — not the totality of what a ring grants. *(An earlier reading gated shapes at those anchors; superseded 2026-07-05 — shapes are ungated, the anchors gate school access.)*

### The Ring as Key [PROPOSED, 2026-07-05]

Elideus's design for what a ring physically *is*: **a ring is a literal worn ring, and it is the mage's key to magic** — the item that unlocks what shapes, synergies, and schools they can actually use. "Ring standing" (Book I, Weapons and Affinity, where a catalyst already scales off it) is not only an abstract rank but the item on the mage's hand. This reconciles the abstract-milestone reading with a worn item: the item is *how the milestone is carried*.

**Three rings make a full-schools mage.** A mage wears up to **three** physical rings, and each covers a band of the seven ring-ranks:

| Worn ring | Gained at | Covers ring-ranks | Nature | Marks |
|---|---|---|---|---|
| First ring | start of magical study | 1st – 3rd | gemmed | apprentice through journeyman |
| Second ring | reaching the **4th rank** | 4th – 6th | gemmed | first synergy-school access (below) |
| Third ring | a **special quest** | 7th | **not gemmed — quest-won regalia (ring, crown, amulet, …)** | secondary synergy schools; a **full-schools mage** |

So a mage holding all three rings has climbed 1-2-3, then 4-5-6, then 7 — the whole progression — which is exactly why the existing synergy anchors fall at the **4th and 7th ring**: those thresholds are where a *new physical ring* is earned, not just a new rank. (This gives the long-standing 4th/7th anchors, below, a physical reason for being the anchors.)

**Gems socket schools into the first two rings.** On the first and second rings, new schools are unlocked by **adding gems** — a gem per school opened. The ring is the key; the gems are which doors it turns.

**The third ring is special — a quest ring, not a gemmed one (Elideus, 2026-07-05).** The 7th-rank ring is **always earned through a special quest**, and it is not the gem-socket kind: it is a singular crafted thing — a **special alloy** worked into a focus/talisman-grade item (Book IV, Reagent Grades; a permanent focus in its own right). It represents the capstone the whole system builds toward: **harnessing all the shapes and all the colors at once**. That is meant to be **hard even for a long-lived character and practically impossible for a normal-aged mortal** — the reach exceeds a short life. This makes the third ring **another face of the racial longevity tradeoff** (Book I, race-as-tempo; Book V, Peoples): the near-immortal races that the endgame's Muse-path already requires (Book I; ledger EC054473) are the ones who can realistically wear all three. A full-schools master, then, is defined less by gems than by having completed the third-ring quest — three rings, the last of them earned, is the mechanical expression of mastery of all fourteen schools — both Domains of every color (ledger 6B2048A6, reversing A8FF6AB0's "at least one school of every color").

**The capstone need not be a ring at all (Elideus, 2026-07-05).** "Third ring" names the *tier* — the third rank-band — but the regalia that carries it can take any masterwork form: a **ring, a necklace, a bracelet, a crown**. What form it takes, and the **nature of the quest that earned it**, are individual to the master who completed it — which turns each capstone into a **story object**: two full-schools masters are marked by different regalia won through different trials, and those trials are direct hooks for story content (the personalized, lore-bearing counterpart to the gem-socketed lower rings, and a natural fit for the Story Content Pack machinery of Book II). The **Order of Three Rings** (Book VI) is named for the tier, not for a uniform of literal rings.

**[OPEN QUESTION]** The exact gem-to-school-to-color mapping (seven colors, fourteen schools — how many gems, and whether a gem keys a school or a color) is unspecified; so is what the third-ring quest actually is, what the special alloy is, whether the three rings are worn on set fingers, whether losing/removing a ring suspends its ranks, and how Black — excluded from ring-based synergy (below) — sockets or quests at all. Structure ratified in direction by Elideus ("I think we can go with it like this"); these details are not.

> **[MAJOR OPEN — cross-system]** Rings, spells, and casting must be **locked in together with the stats and combat systems** (Elideus, 2026-07-05): **casting cost** and **reagent consumption** are combat-economy quantities, so ring standing, spell complexity (below), reagent grade, and the Book I resource bars (Mana especially) and stat lanes are one interdependent design that must land together — not piecemeal. This is the magic-side instance of the standing rule that items, combat, and stats interdepend and must be designed as a unit (ledger 1E5C0E3C). Much of this is still missing by Elideus's own assessment ("definitely missing a lot").

> **[OPEN QUESTION]** The exact ring count beyond the 4th/7th anchors below, and the per-ring vocabulary/complexity unlock table, are unratified. (Ledger B92EE74A.)

**Spell complexity by ring [PROPOSED, 2026-07-05]** — a candidate curve for the "grammatical complexity headroom" named above, simple words at low rings scaling to real power at high ones:

| Ring | Grammar available | Example |
|---|---|---|
| 1st | Bare core only — one noun + one verb, no modifiers. | **Ex Flam** (Banish Fire) |
| 2nd–3rd | One modifier slot opens — a single adjective *or* adverb. | **Vas Flam** (Greater Fire) |
| 4th | Second modifier slot — adjective *and* adverb together — plus first synergy-school access (below). | **Vas Flam Vectus** (Greater Fire in a Straight Line) |
| 5th–6th | Modifier stacking — multiple adjectives and adverbs chained for finer intensity, duration, and area control. | **Vas Fortis Flam Cito Vectus** (Greater, Strong Fire, Quickly, in a Straight Line) |
| 7th | Secondary synergy-school access, and the school's practical grammar ceiling for a single-core spell. | — |
| Beyond 7th | **Expansion / "forbidden fruit" territory** (Elideus, 2026-07-05) — not ordinary progression. Reserved for extremely rare, extremely costly workings: compound spells (multiple noun+verb cores in one casting), and at the far extreme **world-shifting magic** — spells that reach the geology simulation itself (Book III), gated as Long-Term Objectives (Book I) rather than sold as power. Ring count itself is open. | *(idea-grade: a spell that raises a permanent, account-tagged volcano as the terminal step of a lifelong quest — Book I, LTO layer)* |

This is a starting curve, not a ruling — it fills in the shape the existing ring canon already implies (vocabulary breadth + complexity headroom + reagent grade, all growing together) without adding any new mechanic.

- **At the 4th ring**, a mage gains access to the **workings of their two synergy schools** — the schools adjacent to their primary school in the synergy cycle. For example, a mage specializing in **White (Light and Prophecy Magic)** gains access to **Yellow (Lightning and Air Magic)** and **Green (Life and Earth Magic)** at the 4th ring. *(Superseded 2026-07-05: this access was previously framed as gaining those schools' shapes; shapes are now ungated, and the anchor grants cross-school access itself.)*

- **At the 7th ring**, mages unlock the **secondary synergy schools** — the next tier out on the cycle. Continuing the **White** example: **Red (Fire and Blood Magic)** and **Blue (Water and Illusion Magic)** at the 7th ring.

- **Black (Death Magic)** is unique in that it does not participate in this synergy system. Black mages have special gameplay mechanics that grant their cross-school reach through other means, maintaining their distinct approach to magic.

In the full synergy cycle—**White → Yellow → Red → Orange → Black → Blue → Green**, the perimeter ring of the **septisigil** (see `septisigil.svg`)—Black is excluded from the ring-based synergy progression. For example, an **Orange (Chaos Magic)** mage gains access to **Red (Fire Magic)** and **Blue (Water Magic)** at the 4th ring, bypassing Black entirely. The one color a school's rings never open — the septisigil leftover — is its **opposite** (Book VIII, Anti-Colors; ledger 5E86ED4A).

**The two orderings are the two layers of the septisigil** (resolved 2026-07-04, ledger 244A27FF; the figure is `septisigil.svg`). The synergy cycle and Book V's school genealogy were never a single ordering in conflict — they are two different systems drawn on the same seven-pointed sigil, which is why both are true at once:

- **The synergy cycle is the sigil's perimeter ring** — the seven schools set around a heptagon, clockwise from White at the crown: **White → Yellow → Red → Orange → Black → Blue → Green**, and back to White. This governs *magic mechanics*: ring-based synergy access and school adjacency. Black holds its seat on the ring but is **bypassed** by the synergy progression — which is precisely what "Black combines with nothing" means in this system. (Removing Black leaves the six-school working loop the Specialty Domains trace.)

- **The genealogy is the sigil's inner weave** — the lines struck across the figure: White→Black and White→Orange; Black→Green and Orange→Yellow; Green→Red and Yellow→Blue. This governs *cosmology* (Books V–VI): the descent of the colors, in two lineages from White — the **Black line** (Black → Green → Red) and the **Orange line** (Orange → Yellow → Blue), matching the creation myth in which chaos gives rise to air-and-lightning and thence to water, and death gives rise to earth-and-life and onward.

Because the two systems describe different relationships — mechanical adjacency versus cosmological descent — they coexist by design. Black is a genealogical *parent* on the weave (death gives rise to life) yet combines with nothing on the *ring*; there is no contradiction between the two. (Closes ledger 244A27FF.)

The septisigil is not only a diagram. **It is the Chalice constellation** — the hidden thirteenth, the Lonely Muse's, at the galactic heart (Book VI; ledger 244A27FF): its seven stars are these seven schools set in the ring, drawn as the winged sigil. The figure on the page and the figure in the sky are one and the same.

---

The seven classic figures below are the **traditional school associations** — lore, not gates (shapes ungated, 2026-07-05). They are the oldest entries in a shape vocabulary that is now **open and growing**: abstract geometry and concrete forms alike (wall, arrow, ballistic arc, slice, and kin), each to be coined as a Latin-esque word of power when its first Grimoire entry needs it.

- **Orbis** – Circle - The Shape of White  
  A perfect circular shape, typical to area effect spells.

- **Serratus** – Sawtooth Wave - The Shape of Yellow  
  A zigzag pattern resembling a lightning bolt.

- **Vectus** – Straight Line - The Shape of Red  
  A direct and forceful line, often used in spells focused on precision.

- **Chevronis** – Chevron - The Shape of Orange  
  An angled shape, resembling a sharp, piercing wave, or cone.

- **Lunaris** – Crescent - The Shape of Black  
  A curved, crescent-like shape, often associated with shields and shadows.

- **Undula** – Sine Wave - The Shape of Blue  
  A flowing, wave-like shape.

- **Spiralis** – Spiral - The Shape of Green  
  A continuous, coiling shape.

### **Reagent Grades**

Spells draw on reagents in three kinds (ledger B92EE74A):

- **Consumable reagents** — a single material, burned per cast. Book III's alchemy and material tables are the natural source.
- **Charge-limited talismans** — a crafted item good for a fixed number of casts before it is spent or needs recharging.
- **Permanent foci** — an equipped item (wand, staff, amulet) that never depletes, but must be held to cast that word or spell, or that waives the consumable cost while worn.

A spell's required reagent kind is not fixed — ranking up a school's ring is intended to let a mage satisfy the same spell with a cheaper-grade reagent (consumable → talisman → focus-only), giving ring progression a felt payoff beyond vocabulary size.

> **[OPEN QUESTION]** The exact ring-to-grade curve — which ring downgrades which reagent tier, and whether the downgrade is per-spell or per-word — is unratified. (Ledger B92EE74A.)

### **Reagents by Color [PROPOSED, 2026-07-04]**

A starter list, not an exhaustive catalogue — seed content for each color's spells, in the same spirit as Book VII's archetype matrix. Each reagent names its kind (Consumable / Talisman / Focus) and its **source register**, since the four mortal colors, White, and the Orange/Black pair each gather reagents through a different mechanism:

**Biome-sourced (Green, Yellow, Red, Blue)** — drawn from Book VII's nine biomes (Appendix E), same recruit-by-biome logic as creatures: gathering the rarer grades likely tracks the same learn-once, use-anywhere pattern as Monster Mastery (Book II), though this is not yet ruled. The Domain world matching each color (Terria/Green, Aerolon/Yellow, Sanguia/Red, Aquia/Blue) is expected to generate a heavier share of that color's matching biomes when those worlds' terrain is built out — Green leaning Temperate Forest/Jungle/Underland, Yellow leaning Mountain-Highland/Tundra's thin air, Red leaning Volcanic/Desert, Blue leaning Coastal-Ocean/Wetland — making the endgame Domain worlds richer reagent grounds for their own color, not the exclusive source.

- **Green — Vitae Root** (Consumable): a fibrous root common to Temperate Forest and Tropical/Jungle undergrowth; the default Vita-aligned healing/growth reagent.
- **Green — Verdant Heart** (Focus): a calcified seed-node found only in Underland root-caverns beneath old-growth forest; rare, permanent, never depletes.
- **Yellow — Storm-Glass** (Consumable): fulgurite — lightning-fused sand or rock — found on Mountain/Highland peaks and High Steppe after storms.
- **Yellow — Zephyr Down** (Talisman): down-feathers from high-altitude fauna, charge-limited, recharged by exposure to open sky.
- **Red — Cinder Salt** (Consumable): mineral residue scraped from active Volcanic vents.
- **Red — Bloodstone** (Focus): a crystallized mineral found at Desert/Volcanic borders, formed where Sanguis-aligned creatures have bled for generations.
- **Blue — Tidewrack Pearl** (Talisman): harvested from Coastal/Ocean reefs, charge-limited by tidal cycle.
- **Blue — Mirrorwater** (Consumable): still water drawn from a Wetland fen at a specific hour, the base reagent for Illusion/scrying work.

**Celestial-sourced (White)** — White channels eternity, not terrain (ledger BC54B7D7); its reagents key to the sky's deterministic schedule (celestial primacy, ledger 3312B2AB) rather than any biome.

- **White — Chalice Dew** (Consumable): condensation collectible only during a named constellation's alignment; foretold, never random, and vanishingly rare by design.
- **White — Lucent Glass** (Focus): glass fused by direct moonlight at a specific lunar phase (the moon's real-ephemeris cycle, ledger D2AAFEEC).

**Event/site-sourced (Orange, Black)** — neither color's magic is created for mortals to farm from terrain (ledger BC54B7D7: both are the First Titan's own two faces); their reagents come from where their color's *events* have already happened, not from a place the ecological simulation grows.

- **Orange — Wrongflesh** (Consumable): tissue harvested from a creature killed at a chaos-scarred site (a Transformation-domain content-pack location, Book VII Appendix C); inherently unstable, short shelf life.
- **Orange — Warp-Glass** (Talisman): glass slumped by raw transformation backlash, found only where Orange-aligned experiments have gone wrong.
- **Black — Grave Ash** (Consumable): ash from a ruin or barrow that decayed all the way through the erosion system (Book II, ledger 95EAF28D) — the more thoroughly abandoned, the purer the ash.
- **Black — Hollow Locket** (Focus): a personal effect recovered from a genuine site of death; per the Black/Void constraint, it reflects nothing new — it only ever channels what was already there.

**[OPEN QUESTION]** Whether White/Orange/Black's Domain worlds (the Realm of Light, the First Titan's Laboratory and Study) run their own biome-like register for reagent purposes, or whether reagents there are purely event-triggered with no terrain analogue at all, is unresolved — same fork as Book VII's open question on whether the seven Domain worlds share the nine-biome model or run a separate bestiary/resource register (Book VII, Biome Seeding Table).

### **Creatures**

1. **Undead Beings** – *Mortivus*  
   Represents creatures like vampires, zombies, and liches.

2. **Lycanthropes and Shapeshifters** – *Mutarex*  
   Refers to werewolves, selkies, and other transformative beings.

3. **Spirits and Ghosts** – *Umbraxis*  
   Covers spirits such as wraiths, banshees, and other ghostly entities.

4. **Eldritch Abominations** – *Tenebryth*  
   Represents unspeakable horrors from beyond, like Great Old Ones and Shoggoths.

5. **Demons and Infernal Beings** – *Inferaxis*  
   Refers to demons, devils, ifrits, and other infernal creatures.

6. **Elemental Entities** – *Elementor*  
   Represents elemental beings like fire elementals, sylphs, and undines.

7. **Fae and Fairy Folk** – *Faerilis*  
   Refers to faeries, leprechauns, spriggans, and other fae creatures.

8. **Divine and Semi-Divine Beings** – *Divinor*  
   Encompasses gods, demigods, and other divine entities.

9. **Mythical Beasts and Monsters** – *Bestyros*  
   Refers to creatures like dragons, griffins, and manticores.

10. **Hybrid Creatures** – *Chimerus*  
    Represents centaurs, minotaurs, and other mixed-beast creatures.

11. **Constructs and Animated Beings** – *Automivus*  
    Refers to golems, animated statues, and other artificial beings.

12. **Giants and Titans** – *Titanox*  
    Covers giants, titans, and other colossal entities.

13. **Sea and Water Creatures** – *Aquorim*  
    Represents krakens, merfolk, and sea serpents.

14. **Celestial and Cosmic Beings** – *Astralyth*  
    Refers to star beasts, cosmic dragons, and celestial entities.

15. **Shadow and Darkness Entities** – *Noctilaris*  
    Represents creatures that embody darkness and shadow, like shades and shadow beasts.

16. **Magical Animals and Beasts** – *Mystivora*  
    Refers to magical creatures like unicorns, hippogriffs, and pegasi. *(Phoenixes moved to Pyrravus below, resolved 2026-07-04, ledger 8E29A0D3.)*

17. **Tricksters and Illusionists** – *Illusorix*  
    Represents beings skilled in illusion and trickery, like kitsune and Pookas.

18. **Underground and Earth Dwellers** – *Terradun*  
    Refers to creatures like dwarves, kobolds, and gnomes.

19. **Cursed and Enchanted Beings** – *Maleforix*  
    Represents cursed creatures like witches, enchanted knights, and gorgons. *(Liches moved to Mortivus only, resolved 2026-07-04, ledger 8E29A0D3 — a lich is definitionally undead, not merely cursed: "a skeleton of, reanimated from, but not living," Book VII.)*

20. **Desert and Middle-Eastern Creatures** – *Aridusol*  
    Covers desert-dwelling creatures like sphinxes, lamassu, and rocs.

21. **Forest and Nature Spirits** – *Sylvaris*  
    Refers to dryads, ents, and other nature-related beings.

22. **Beings of Fate and Time** – *Tempivor*  
    Represents creatures associated with time and fate, like the Moirai and Norns.

23. **Heavenly Hierarchies** – *Aetheron*  
    Refers to seraphim, cherubim, and other celestial beings in heavenly hierarchies. *(Resolved 2026-07-04, ledger 8E29A0D3: gloss narrowed to match the description — infernal beings are Inferaxis's exclusively.)*

- **Pyrravus** – Mythological Birds  
    Represents creatures like phoenixes, thunderbirds, and other mythological avian creatures.

- **Insectilis** - Insectoid and Arachnid Creatures  
  Refers to giant spiders, scarab beetles, and other insect-like entities.

- **Animalia** – Animal  
  Represents general animals and beasts found in nature.

- **Reptilia** – Reptile  
  Denotes reptiles such as snakes, lizards, and other cold-blooded creatures.

- **Magus** - Magic User
  Represents a magic-user.

**Note:** Undead dragons (**Dracolich**) and undead magic users (**Lich**) are formed by combining **Vas** (Greater) with the appropriate noun (e.g., **Draco** for dragon or **Magus** for magic user) and **Mortuus** (Undead), possibly influenced by the spell's location. *[OPEN QUESTION — Draco and Mortuus are defined in no dictionary (the undead category noun is Mortivus), and their grammar slots are unassigned; no words added this pass. Ledger 67DB3F76.]*

## Combat Mechanics

Some spells will have one or more of the following effects. These are the primary combat mechanics impacting effects, which may be called different things in each school.

### Counter Physical Capabilities

- **Root** (cannot move)

  This effect causes the target to stop all movement. While rooted, the target can still cast spells but cannot move from their position for a period of time.

- **Snare** (move slowly)

  This effect reduces the target’s movement speed. They will move at a slower maximum speed for a period.

- **Slow** (attack speed reduction)

  This effect decreases the target's animation speed. Attacks and recovery actions take longer to execute for a period.

- **Expose** (reduced physical defenses)

  This effect lowers the target's physical defenses, causing physical damage to be mitigated to a lesser degree.

### Special Cases

- **Poison / Disease** (damage over time)

  This effect inflicts damage on a periodic basis until cured. The damage can vary based on the potency of the poison or disease.

- **Stun** (interrupted, frozen)

  This effect causes the target to halt all actions, including movement and casting. They are unable to move or cast spells for a period.

### Counter Magical Capabilities

- **Silence** (cannot cast)

  This effect prevents the target from casting spells that require verbal components or the utterance of words of power.

- **Distract / Confound** (increase cast time / cooldown)

  This effect reduces the target’s casting efficiency. Casting times and cooldown periods are increased, and there is an elevated chance for spells to fizzle or fail.

- **Sunder** (reduce magical defenses)

  This effect diminishes the target's magical defenses, causing magical damage to be mitigated to a lesser degree.

### Counter Player Control

- **Fear** (uncontrollable, run away)

  This effect causes the player to lose control of their avatar, which then moves in a direction away from the caster, often in a panic.

- **Panic** (uncontrollable, move at random)

  This effect results in the player losing control of their avatar, which then moves about in random directions, potentially into danger.

- **Charm** (take control of another unit)

  This effect transfers control of the player's avatar to the caster. The player can see what their avatar is doing but cannot influence its actions.

---

## Magical Domains

In addition to spells being arranged by school and ring, each spell falls within one or more domains. Trainers for each domain can be found in most schools, though some are less common than others. *[The "ring" structure referenced here is defined in no book — see the open question in the Shapes section (ledger 63B28A6B).]*

*[EDITORIAL — 2026-07-02: the categories below are the **disciplines** — the types of spells in the ruled structure color → Domain (aspect) → aligned school → discipline → spell, and each discipline is a skill in the skill system (ledgers DCB350D4, 232B6EEE; the skill design itself is a major open issue, F618F695). The word **Domain** properly names the fourteen school-aspects (Book V); the headings below keep their legacy "Domain" labels pending the deferred engine-side reconciliation (ledger 33173716). Each spell belongs to a discipline (Elideus's phrasing is singular; this intro's "one or more" is an open wobble — ledger 9D85D076).]*

### Magical Domains

All spells fall within one of the following domains:

- **Abjuration** – Protection, warding, and banishing.
- **Cursing** – Debuffs and damage over time effects.
- **Conjuration** – Summoning elements, entities, and objects.
- **Divination** – Knowledge, prophecy, and revelation.
- **Enchantment** – Control, domination, and influence over minds.
- **Evocation** – Elemental forces, destruction, and direct damage.
- **Artificing** – Imbuement of items with magical properties.
- **Transmutation** – Transformation of material states and properties.
- **Invocation** – Calling upon the power of the gods through supplication.

### Specialty Domains

Additionally, spells may be listed as one of these specializations, and each specialization is associated with at least two schools that are particularly adept in that specialization.

- **Thaumaturgy** – Divine magic associated with healing and protection (White/Green).
- **Astromancy** – Scrying and celestial magic involving stars and planets (White/Yellow).
- **Electromancy** – Manipulation of lightning and electrical forces (Yellow/White).
- **Aeromancy** – Control over air and wind elements (Yellow/Red).
- **Pyromancy** – Mastery over fire and heat (Red/Yellow).
- **Hemomancy** – Manipulation of blood and life forces (Red/Orange).
- **Elementalism** – General control over elemental forces (Orange/Red).
- **Chronomancy** – Manipulation of time and temporal energies (Orange/Blue).
- **Mentalism** – Illusion and mental manipulation (Blue/Orange).
- **Hydromancy** – Control over water and liquid elements (Blue/Green).
- **Necromancy** – Magic involving death and the undead (Black). *[Black stands alone by canon — Black combines with nothing (Book V, Canon Rulings) — so the "at least two schools" rule above does not extend to Black.]*
- **Geomancy** – Earth magic focusing on soil and minerals (Green/Blue).
- **Biomancy** – Life magic related to growth and healing (Green/White).

### Ancient Domains

In times before the classification of magic by color, magic was more primitive. Some still study these ancient domains for academic reasons:

- **Eldritch Magic** – Forbidden magic harnessing the fundamental forces of the universe and reality itself.
- **Spirit Magic** – Forbidden magic dealing with the soul and the essence of life.
- **Animancy** – Forgotten magic that gains power through sacrifice and ritual sacrament.
- **Logomancy** – Forgotten magic utilizing runes imbued with words of power.
- **Fetismancy** – Forgotten magic using fetishes, talismans, totems, and sigils as focal points.

---

## Types of Casting

Magic can be performed using various methods, each requiring different skills and components.

- **Incantation**

  Using spoken words of power to create magical effects. Incantations often require precise pronunciation and timing.

- **Channeling**

  Utilizing a focus item, such as a wand or staff, to direct and amplify magical energy stored within the item.

- **Ritual Magic**

  Involving multiple casters who use focus items and coordinated actions to perform more powerful spells that are beyond the capability of a single caster.

- **Sympathetic Magic**

  Employing symbolic constructs or representations to focus magic. This includes voodoo dolls, effigies, or other items symbolically linked to the target.

- **Alchemical Magic**

  Using physical catalysts and substances to create magical effects. This type often involves potions, elixirs, and transmutations.

---

## Psionics

### Overview

Psionics is a distinct form of magic that relies on innate mental abilities rather than external components, incantations, or traditional magical energies. Practitioners of psionics, known as psions, harness the power of their minds to influence the physical world, manipulate elements, and interact with other minds. Psionics is considered a separate discipline from traditional magic and requires specialized training and mental discipline.

> **[OPEN QUESTION]** How psionics relates to the seven-school system is an open fork — outside the Prism by design, a casting style mapped to an existing school/stat pair, or unresolved legacy content pending redesign. The content in this section is retained as-is pending that ruling. (Ledger 7609E7E9.)

**Canon (ruled 2026-07-03, ledger CC7C057B):** there are three parallel paths to magic, all effectively the same underlying power: **Magic** (the word-grammar system above — researched, memorized, deep), **Cantrips** (below — simple, teachable to anyone with sufficient intelligence), and **Psionics** — its own path with its own costs, mental and innate rather than external-component-based. Psionics is one discipline, not two. The abilities below run the same spectrum from ordinary to extreme that Magic runs from cantrip to archmage-tier spell — the Inverted Principality's cosmic-horror bestiary entries (Book VII: Static Choir, Loom-things, the Unseen Tally) are this same roster's monstrous end, not a separate system.

### Abilities

- **Telepathy**

  The ability to read or communicate thoughts directly to another being's mind without verbal communication.

- **Telekinesis**

  The power to move or manipulate objects and matter with the mind alone.

- **Pyrokinesis / Cryokinesis**

  - **Pyrokinesis**: The ability to generate or control fire using mental focus.
  - **Cryokinesis**: The ability to generate or control ice and cold temperatures mentally.

- **Hydrokinesis**

  Manipulating water in all its forms—liquid, ice, or vapor—through mental control.

- **Eidetic Projection**

  Projecting vivid mental images or illusions into the minds of others, creating sensory experiences that seem real.

- **Geokinesis**

  Controlling earth, rock, and minerals with the mind, allowing manipulation of terrain and geological materials.

- **Aerokinesis**

  The ability to influence air currents and wind patterns mentally.

- **Chronokinesis**

  Manipulating the perception or flow of time, potentially slowing down or speeding up events from the psion's perspective.

- **Biokinesis / Hemokinesis**

  - **Biokinesis**: Altering biological functions or structures in living organisms.
  - **Hemokinesis**: Specific control over blood flow and properties within organisms.

- **Photokinesis / Umbrakinesis**

  - **Photokinesis**: Manipulating light, including bending light to create illusions or invisibility.
  - **Umbrakinesis**: Control over darkness and shadows, potentially concealing areas or creating constructs.

- **Technopathy**

  The ability to interact with and control electronic devices and machinery mentally.

- **Metakinesis**

  Manipulating kinetic energy, influencing the movement and momentum of objects and beings.

- **Morphokinesis**

  Altering physical forms and shapes, including self-transformation or changing the form of objects.

- **Necropathy**

  Communicating with or sensing the presence of spirits and the dead, possibly controlling undead entities.

### Limitations and Training

Psionic abilities demand intense mental discipline and focus. Practitioners often engage in rigorous meditation and mental exercises to enhance their cognitive capacities and control. Overexertion can lead to mental fatigue, physical exhaustion, or unintended side effects. Psions must also be cautious of ethical considerations, especially when manipulating other minds or life forms.

---

## Cantrips

### Overview

Cantrips are simple spells that can be cast by anyone with sufficient intelligence and basic magical training. They are the foundational spells taught to apprentices and novice spellcasters, requiring minimal magical energy and components. Cantrips are primarily used for utility purposes and are valuable tools in a spellcaster's repertoire.

### List of Common Cantrips

- **Blink**

  Teleport a short distance in any direction within the caster's line of sight. Useful for evading obstacles or quickly repositioning.

- **Create Food**

  Conjure a simple, nourishing meal sufficient to satisfy hunger for a short time.

- **Create Water**

  Summon clean, drinkable water, either filling a container or creating a small rainfall.

- **Light**

  Generate a small, hovering light source that illuminates the immediate area. The light can be attached to an object or float freely.

- **Mend**

  Repair minor damage to an object, such as fixing a broken chain link, mending a tear in clothing, or sealing a cracked glass.

- **Meditate**

  Enter a state of deep concentration to regain mental clarity, reduce stress, and recover a small amount of magical energy or stamina.

- **Prestidigitation**

  Perform minor magical effects like creating harmless sensory illusions, cleaning or soiling items, warming or chilling food, or producing small trinkets.

- **Message**

  Whisper a message to a target within a limited range, allowing for private communication. The target can reply in a whisper only the caster can hear.

- **Detect Magic**

  Sense the presence of magical auras within a certain radius, identifying the school of magic if applicable.

- **Mage Hand**

  Create a spectral hand capable of manipulating objects at a distance, such as opening doors, retrieving items, or pouring liquids.

- **Spark**

  Ignite a small flame to light candles, torches, or campfires. Useful for starting fires without flint or tinder.

- **Chill Touch**

  Summon a ghostly hand that delivers a chilling touch to a target, causing minor necrotic damage and potentially hindering undead creatures.

- **Resistance**

  Grant a minor boost to an ally's ability to resist harmful effects, providing a small bonus to saving throws.

- **Minor Illusion**

  Create a simple illusionary image or sound that can deceive observers, useful for distractions or minor deceptions.

- **Guidance**

  Bestow a small enhancement to an ally's ability to perform a task, providing a brief boost to skill checks.

### Learning and Practice

Cantrips are often the first spells learned by aspiring mages and serve as a gateway to more advanced magic. Regular practice helps the caster improve control and efficiency, laying the groundwork for mastering higher-level spells. Despite their simplicity, cantrips can be creatively applied in various situations, showcasing the versatility of magic.


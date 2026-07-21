# I. Adventurer's Design

This book defines the adventurer's side of Prism: the character — their stats, their combat, their life, and their family line. Adventurer gameplay is a classic MMO. The Dungeon Maker population never interacts with adventurers directly; the coupling is asynchronous — dungeons are built, published, discovered, and run, never contested head-to-head (Book II; ledger C91261FB).

---

## The Seven Stats

The seven stats are canonical, one per color of magic (Book V, Canon Rulings 2026-07-02; ledger 2ABDD515, realigned by FF1F7955). Each stat is the capacity that its color's realm studies and reinforces, paired with that color's philosophy pair (Book V). Each color fields **two schools** — one per Domain (aspect), fourteen among the seven shard worlds, with disciplines inside each school: the types of spells documented in Book IV, each discipline a skill in the skill system (ledgers FF1F7955, A8FF6AB0, DCB350D4, 232B6EEE) — and the philosophy belongs to the color: followers of both poles study at both of its schools. The two poles are deliberately unlabeled and unjudged — which is right is the player's own conclusion (ledger E4F46BA5).

| Stat | Color | Schools | Philosophy |
|---|---|---|---|
| Wisdom | White | Light and Prophecy | Enlightenment vs Revelation |
| Dexterity | Yellow | Air and Lightning | Courage vs Audacity |
| Strength | Red | Fire and Blood | Valor vs Conquest |
| Luck | Orange | Chaos and Transformation | Diligence vs Cunning |
| Intelligence | Blue | Water and Illusion | Knowledge vs Truth |
| Endurance | Green | Earth and Life | Perseverance vs Hunger |
| Constitution | Black | Death and Void | Transcendence vs Ambition |

Stats measure **capacities, not actions**. Charisma is excluded on exactly this rule: charm is something a character *does*, not something a character *has* — and where a charisma-like check is required, it uses **Luck**. (Ledger 2ABDD515.)

Constitution and Endurance never collapse into one stat. **Constitution (Black) is the capacity to be hit. Endurance (Green) is the capacity for sustained effort.** One measures what the body withstands; the other measures what the body spends. (Ledger 2ABDD515.)

Every stat must cash out in combat-relevant terms (ledger 2ABDD515), and each is designed to be useful in as many ways as possible — no single-purpose stats (ledger DE7E8605). The combat lanes are ruled (2026-07-02; Constitution and Endurance from 2ABDD515, the other five from DE7E8605):

| Stat | Color | Combat lane |
|---|---|---|
| **Constitution** | Black | **Survivability.** The hit-budget — how much punishment absorbed before falling — plus poise and resistance to death-tier afflictions (instant-death, rot, curse/void). Governs the **Health** bar. |
| **Endurance** | Green | **Action economy.** The sustained-effort resource every exertion spends — attack, dodge, block, sprint — and the rate it recovers and stagger clears. Governs the **Stamina** bar. |
| **Strength** | Red | **Force.** Heavy-weapon scaling, guard-break / poise-damage, heavy-impact stagger, and bleed buildup (the Blood domain). |
| **Dexterity** | Yellow | **Speed & precision.** Finesse/ranged scaling, attack and recovery speed, evasion / i-frame efficiency, ranged accuracy, and interrupt/stun on precise strikes (Lightning). |
| **Intelligence** | Blue | **Technique, the mind, and knowledge.** In combat: status/mind-affliction potency and resistance, weakpoint/critical knowledge (*expose*), and combat-art access. Out of combat: the **research gate** — the requirement to unlock higher learning. |
| **Wisdom** | White | **Foresight & the divine.** Telegraph-reading clarity and parry/guard-counter window width (the fairness law as a stat), divine-magic and warding potency, and resistance to fear/illusion/mind effects. |
| **Luck** | Orange | **Fortune & chaos.** Critical chance, the reliability of procs (does your bleed/stun land, or theirs on you), ambush/first-strike advantage, **loot and discovery fortune**, and any charisma-shaped check (ledger 2ABDD515). |

Three prior open questions close with this table (ledger DE7E8605): **Intelligence gates research** — you cannot unlock higher learning without meeting its requirement, and rings, affinities, shapes, and access to Words are all gated by research (see *Areas of Focus and Talent Graphs*, and the Long-Term-Objective layer under Progression); **Luck governs loot and discovery**; and **Wisdom does *not* read the celestial sky** — reading the deterministic heavens is the literal player's own skill, out-of-character, not a character stat.

> **[BUILD]** The *derived-value formulas* — how each stat computes its bar, poise, crit %, proc reliability, parry-window frames, telegraph clarity, bleed/status power, and weapon-scaling grades — are the next design pass. The lanes are ruled; the numbers are not yet.

---

## Combat Framework

Combat is **Dark Souls–style action combat**. The combat simulation runs on a **frame-accurate combat tick clock, decoupled from render framerate**. Attacks, hitboxes, invulnerability frames, and effects are authored as **TAE-style event timelines** — events keyed to combat ticks on an animation timeline. (Ledger 9D249EE4, Seed canon 5. No engine document yet describes this design; the ledger entry is the canon source.)

Per "data is truth; code and geometry are render targets" (ledger 65C82EE0), tick rates, timeline formats, and other implementation specifics belong to engine documentation, not this book. What belongs here is the design truth: combat outcomes are decided by a deterministic tick simulation, and everything a combatant does is an authored timeline of events on that clock.

**Source material.** Ideation-only design notes from 2014 are archived at `sources/combat-systems-ideation-2014.md` (intake ruling: ledger 78C70439). They are twelve-year-old ideation, not canon, but carry the payload Elideus marked durable: a status-effect taxonomy treated as **direct game mechanics** — stun, snare, slow, root, poison, confound/distract, aphasia, fear, confuse, silence, expose, tashina, mesmerize — plus time/distance/movement constraints: attack property flags (physical/magic × melee/ranged/point-blank AE/targeted AE/cone/line), projectile travel, channeling as a self-root with tick-applied effects, displacement as velocity-on-hit, and cooldown discipline. The stat-to-mechanics coupling is design-and-build work still ahead (ledger 78C70439); the notes inform the [STUB] below without resolving it.

**The fairness law.** Canon, in Elideus's words (Half-Life 1998 design lineage; ledger 95EAF28D): *"If the player dies, they can only die because they missed something… The player cannot blame the designer for failure."* Combat is telegraph-and-dodge. Every threat carries two readable layers: a **spatial tell** — danger is a legible place, entered by choice — and a **temporal tell** — the attack telegraphs before it lands, and the counter-window keys to the tell. Lethality may be extreme (one-shot kills are permitted); legibility is non-negotiable. The canonical exemplar: shadow wisps that decapitate in one hit live only in mud, and the mud bubbles before they strike — avoid the mud and you are categorically safe; enter it, and the bubbles are the contract.

**Resources — three bars, no numbers.** Combat runs on three meters — **Health** (Constitution), **Stamina** (Endurance), and **Mana** (the caster resource). Each is **scaled to 100, drawn at a fixed size with no markers, and shown only as a normalized fraction (0.0–1.0)** — damage is real and exact beneath the surface, but the player never sees an absolute number (ledger BFBE6085). This is the surviving, light form of a wider **philosophy of ambiguity**: telling the player as little as possible about the machinery beneath, the opposite of Souls' explicit stat accuracy. *(An earlier "OneBar" concept — a single blended Health/Stamina/Mana meter damaged as Physical / Effort / Mental — was considered and discarded: it conflicts with Prism's very explicit crafting, skill, and requirement systems. How far the ambiguity philosophy reaches beyond the number-hiding bars and the journal (see Progression) is itself a major open decision.)*

> **[BUILD]** The combat frame is set; the numbers are not. Still to design: poise and stagger thresholds, lock-on and targeting, block/parry/dodge frame values, damage typing and resistances, ranged and magical delivery timing, and how each stat's derived values parameterize the simulation. (Resources, weapons, and advancement are framed in the sections that follow.)
> - **Resolved:** Endurance governs the **Stamina** bar — the action resource every attack, dodge, and block spends (ledgers 2ABDD515, BFBE6085).
> - **[OPEN QUESTION]** Spells are researched and memorized before casting (Book IV). Do spell casts occupy TAE-style combat timelines the same way weapon attacks do?
> - **[OPEN QUESTION]** Celestial events are global modifiers across all tiers and regions (ledger 8FB95281). How do they express in combat terms — damage, resources, timeline behavior?
> - **Combat death is not terminal** (ruled 2026-07-02, ledger 0B98560B, resolving D25A57E1): you **respawn** — however lore explains it (magic; "it doesn't matter, literally") — and recover your body **EQ-style**, sometimes dragging your corpse back from where you fell. Ordinary combat death is a setback, not the end of a life. See *Death and Disposition* below.

---

## Weapons and Affinity

*(Framework [PROPOSED]; specific requirements, grades, and movesets are [BUILD]. The ruled points — affinity is abstract, roles are open — are noted inline.)*

Weapons are drawn from real materials and read as real materials: **normal metals, gothic and grim, with at most the slight tints of their alloys.** Any color a weapon throws is earned — the red glow of an enchant, the flash of a proc — never the base blade (ledger DE7E8605). Under that realistic surface, every weapon carries an **affinity**: an abstract, mechanical alignment to one of the seven colors — *not* a visual color — set by its material and shiftable by artificing, which determines the stat its output scales with. A Red-affinity arm scales Strength; a Yellow-affinity arm scales Dexterity; and so on down the seven (Pillar B; ledger DE7E8605). Wielding is governed two ways: a **requirement** — a stat minimum to use the weapon without escalating penalty (you *can* swing anything, badly) — and **scaling**, the affinity above. Each category is also a **moveset**, a family of authored TAE timelines (the seam where combat meets the animation pipeline); stats parameterize the numbers on those timelines, not their shape.

| Category | Role | Requires | Scales (via affinity) | Signature |
|---|---|---|---|---|
| Heavy arms (greatswords, mauls, greataxes) | **Breaker** | High Str + End | Str | Poise-break, huge stagger, slow |
| Martial arms (swords, spears, axes, maces) | **Soldier** | Moderate Str & Dex | Str or Dex | Reliable, balanced |
| Finesse arms (daggers, rapiers, curved blades, whips) | **Assassin** | Dex (+ Luck) | Dex | Fast; crit/bleed on precision |
| Ranged physical (bows, crossbows, thrown) | **Archer** | Dex (Str for warbows) | Dex | Projectile travel; weakpoints |
| Reach / polearms (pikes, glaives, halberds) | **Skirmisher** | Str/Dex | Str or Dex | Spacing, control, anti-approach |
| Fist / martial (gauntlets, claws, unarmed) | **Brawler** | Dex/Str | Dex or Str | Stamina-cheap, rapid, close |
| Shields & guard gear | **Warden** | Con/End to block; Wis to parry | Con/End (block), Wis (parry) | Block scales toughness; parry scales foresight |
| Catalysts & foci (staves, wands, talismans, holy symbols) | **Caster** | Int (literacy) + ring standing | Int/ring; Wis for divine foci | Channel Words; reagent-grade foci never deplete (Book IV) |
| Exotic / chaos arms | **Gambler** | Luck | Luck | Proc-heavy, unpredictable |

> **[OPEN QUESTION]** Whether triggering a weapon's *elemental proc* (a Red blade's fire, a Blue blade's illusion) additionally requires matching color/school attunement, or anyone can set it off. Unrecorded.

**Items are enhanced here, not made here.** Item *creation* belongs to the Dungeon Maker system (Book II); adventurers **mostly enhance** what they find rather than forge it new (ledger DE7E8605). Enhancement runs on the same seams as scaling — affinity sets the scaling stat, the loot stack's adjective rung sets the grade, its shape rung sets the on-hit proc (Book II) — and any lasting enchantment demands ongoing mana upkeep, the same law as everything else (ledger 91F84075).

**Leveling a dropped item is an LTO (Elideus, 2026-07-05).** You are **not stuck with what drops.** A modest item off a low-tier dungeon — say a *Poison Dagger of the Wisp* carrying some minor roll (a "+2 Lunar" bonus, illustrative only, not a real stat) — can be **raised over a character's life** into something worth carrying at level 10: a *Level 10 Poison Dagger of the Wisp, +2 Lunar* is the same item, grown, not a new drop. But the raising costs **time, money, mana, and research investment on a large scale** — a Long-Term Objective (see *Long-Term Objectives*), and a real trade: the investment is big enough that committing it to *this* item is a genuine choice, not something done to every drop. This is the adventurer-side face of the same artificing that the Dungeon Maker kit runs (Book II, Loot Creation); rings work the same way — you invest in what you choose to unlock (Book IV, The Ring as Key).

---

## Areas of Focus and Talent Graphs

*(Frame ruled; the specific graphs and nodes are [BUILD]; the deep skill-scope system is reserved — ledger F618F695.)*

Prism has **no character classes.** This is **open build, in the lineage of the Souls-likes**: a character is defined by their affinities and by where they have spent, not by a label chosen at creation. The **Roles** above — Breaker, Soldier, Assassin, Archer, Skirmisher, Brawler, Warden, Caster, Gambler — are **areas of focus**: any character may research and spend experience in any of them, and their "role" *emerges* from that investment (ledger DE7E8605).

Each Role is a natural fit for a **simple talent graph**, kept deliberately light — easy decisions, clear outcomes, in the register of early-World-of-Warcraft talents — because there are many of them. **Hybrid graphs** bridge adjacent Roles, and the set may be arranged in the **septisigil wheel** (tentative). Unlocking a graph can take work: research and stat requirements gate the higher reaches (Intelligence is the research gate).

**Every investment is permanent.** There is no respec — you may spread your points across many focuses, but no choice is ever undone. This is the build-side face of the character's own "no take backsies" (ledger 91F84075), the same law as the mana-upkeep economy: nothing in Prism is free to reverse. The strategic weight therefore lands on *what to pursue* — planned as a **Long-Term Objective** for the account, not optimized after the fact — and the family tapestry is itself that strategic game (ledger DE7E8605).

> **[BUILD]** The actual graphs, their nodes and costs, the hybrid bridges, and whether the septisigil arrangement holds. The **layered skill-scope system** beneath all of this (how "learning about axes teaches wood, farming, and combat," across three or four nested scopes) is a **major reserved topic** (ledger F618F695) and is not elaborated here.

---

## Group Combat and the Threat Model

*(Framework [PROPOSED]; the numbers are [BUILD]. Group combat takes the Souls action model to EverQuest scale.)*

Group combat is **Dark Souls action combat at EverQuest scale** — large-monster fights few games attempt, kept honest by the fairness law even when forty people share one boss. The **event system is what makes it possible**: because every attack is an authored TAE timeline (Combat Framework), a boss can **select targets for ranged attacks** and **catch clusters of players in a single swing**, and **every player caught reacts on their own dodge and parry window.** Forty individual skill-checks resolve against one boss's timeline at once (ledger A573A152).

**Threat, not class, decides who the boss hits.** Aggro is a **threat table** any build can climb by what it does; the boss swings at whoever holds it. **Roles are jobs, not classes** (open build; *Areas of Focus*): a "tank" is *hold threat + survive the incoming*, and survival is paid either in armor (Constitution/block — the Warden) **or** in pure skill (perfect parry — Wisdom). A cloth-clad dagger master can tank a boss the whole fight — holding aggro, parrying every swing — **when the boss has parryable attacks and they can keep threat.** Parry-tanking is legitimate but conditional; **each combat is unique and its solutions are multiple** — no dominant strategy survives contact with the next boss.

**Casters are power bought with fragility.** Spells occupy combat timelines like weapon swings — cast times, interrupts, rotations (resolving the Combat Framework's open question). Casters put out the damage (death mages burning a boss with black flame) and **draw aggro they cannot survive** unless a tank pulls it off them; healers run full **heal rotations** and, when needed, throw **crowd control** to keep adds off the casters. Roles flex to the fight.

**The stakes are real.** **Combat resurrection is limited**, backups are few, and death is a corpse run (below) — "don't fuck up." The design **rewards the impossible line: be inventive, do it ridiculous, king plays are expected.** A game that hides its numbers and forbids respec earns the right to demand mastery.

**Party scales are canon, and tuning is fixed** (ruled 2026-07-03, ledger CA50B875): **dungeons are 5–7 players** (the clan-dungeon framework builds to that range, Book II), **mid-tier raids 10–15**, **large raids 25–35**, and **mega-raids run 72 with 100 as the aspiration**. There is **no dynamic scaling** — "a dungeon designed for 7 is for 7. Not 5. Not 3." The recovered design direction beneath those numbers is EverQuest's asymmetry ([PROPOSED], basis `sources/combat-party-scale-and-damage-math-chatgpt.md`, ledger A40C5CC7): player power grows on a lower curve than monster power (~square vs ~cube in EQ's case), so soloing is near-impossible, party play is mandatory, a single monster can wipe an uncoordinated 5–7 group, and the **puller** — the specialist who manages the feed of monsters into the group — is a real tactical role again. A **party logbook** (enemy behaviors, configurations, notes from prior runs — the group-scale sibling of the personal journal) is called for from the same source.

> **[BUILD]** The threat formula and pull mechanics; parry/dodge frame windows at raid scale; resurrection rules and their interaction with corpse recovery; boss-side raid-wide mechanics and how cluster/targeted events are authored (the Dungeon-Maker seam, Book II).

---

## The Contested World

*(Ruled — the world's social stance. Ledgers A573A152, 6BEB00D3.)*

Prism's world is **non-instanced**: real-time world spawns in shared space, not per-group copies. **Cleared ground stays cleared** — on the order of a week — so holding a beachhead is a real tactic and a wipe means retaking territory, not resetting an instance. And the world is **contested**: **loot stealing and claim jumping are allowed** — intended, not exploits. "Get what you can when you can, and live with the results."

The game **does not prohibit antisocial play — and it does not systematize the consequence either, because it doesn't need to.** There is no reputation score, no pariah meter, no karma system. Steal loot and jump claims and **people simply won't want to play with you** — and in a persistent, non-instanced world where the biggest content takes a guild's worth of people who trust you, that is the whole punishment, and it is enough. The consequence is other human beings, remembering. This is the old-school (EverQuest / Ultima Online) ethos, of a piece with permanent decisions and honest, unbending rules — a **world, not a ride**.

---

## Factions and Standing

*(System design, 2026-07-05. Distinct from The Contested World above: that governs how **players** judge players — no meter, only memory. This governs how the world's **factions** respond to you, which **is** a mechanical system — but a hidden one.)*

**A hidden hostility value.** Each faction holds a single numerical **standing** toward you, running from murderous to devoted. Illustrative anchors (not final values):

| Standing | Response |
|---|---|
| −5000 | kill on sight |
| 0 | neutral |
| +1000 | friendly |
| +5000 | you are their patron |

**Nothing is ever shown.** There is no faction bar, no numeric readout, no meter anywhere in the game — consistent with the wider philosophy of ambiguity (no absolute numbers, ledger BFBE6085; the Journal replaces XP popups). The player reads only the **behavior**: a merchant's price, a gate that opens or doesn't, a patrol that nods or draws steel. *They attack you — that is the signal.* The AI acts appropriately to the value; the value stays behind the curtain.

**Standing is composed in layers.** Your standing with any one faction is not a single stat you carry but the sum of several influences, so that *who you are* and *how you play* both feed it:

- **Family / tapestry** — your account's inherited standing (the family-wide faction standing of Character Lifecycle): a benefactor threads the line in, a bad actor taints it.
- **Racial** — the standing your race carries by default.
- **Skill / specialty** — what you *do*: a trader reads as friendly to other traders; a pirate does not read well to those same traders.
- **Political / lore** — standing inherited from the world's **historical factions** and their canon alliances, betrayals, and rivalries (Book VI, Factions) — the drama layer, from which set-piece conflicts are built.
- **The account itself is a faction.** Your account is a faction entity in the same model, so factions can hold a disposition toward *you specifically* (not only toward your race or trade), and **players can establish specific dispositions toward other players' factions** — deliberate alliances and rivalries, war and welcome, that the AI honors. This is the hidden, *deliberate* diplomacy layer, and it does not contradict The Contested World: emergent social reputation (who wants to group with you) is still just human memory and unsystematized; this is the separate, opt-in machinery of set faction relationships, and it too shows the player no numbers — only how the world behaves.

**What it's for.** Factions exist to give the world **differing responses based on who you are and how you play** — the same gate, guard, merchant, or quest-giver meets a trader-born faction-patron differently from a pirate of a rival line. It is the mechanical substrate under the world reacting to a character's whole history.

**The layers are weighted, and the weighting is each faction's fingerprint (Elideus, 2026-07-05).** The layers do not simply sum equally — **each faction weights them by its own values**, which is a characterization tool as much as a math one. A culture that prizes **honor** over family will let a reputation as a **thief** weigh heavier than any amount of wealth; a mercantile power will do the reverse, and forgive the thief the moment the ledger balances. The same deeds read differently from faction to faction because the weightings differ. Exact weights are left to future balance — the principle is that the weighting is per-faction and expresses who that faction *is*.

**Declared alliances override the personal layer (Elideus, 2026-07-05).** Players can **formally declare an alliance**, and a formal declaration is a **100% weight override** of the personal-faction standing — the alliance's terms replace whatever your individual standing would otherwise compute. An alliance can carry a **contract**: obligations such as delivering set materials over a span of time, met by your **NPC labor** — the settlement roster (scheduled caravans and automated haulers are a possible extension, but **de-prioritized** and not to be built out here). An alliance is thus a living commitment, not merely a flag.

> **[PROPOSED] — declared alliances as the guild system (Elideus, 2026-07-13, ledger FEC89726; leaning useful, unresolved).** The declared-alliance mechanic doubles as Prism's **guild system** — and the lean is toward a **political-party** shape over a conventional guild: you **declare your membership/allegiance** to a House rather than being rostered into it, the declaration carrying the same standing-override weight as any alliance. A plainer "join my guild/family" roster model is the alternative. This is the canon substrate beneath the ideation "Houses as narrative containers" concept (Book VI §Factions; `sources/story-engine-houses-as-narrative-containers.md`): the *unit* is favored even though that source's story-engine is not canon. Open: declared-allegiance (party) vs invited-roster (guild); how a House relates to the account **Family** (Character Lifecycle) and to cross-account tapestry-binding; and whether a player holds one membership or several.

**Faction states can take physical form [idea-grade].** Because nothing is ever a meter, a faction state can surface as a **world artifact** instead of a number. A kill-on-sight standing might spawn a **bounty document** — the offender's face sketched, a price named — carried and posted by the faction that wants them dead; a patron standing might be an actual **contract**, or an **artifact conferring rule over a settlement**. Not canon — an illustration of what the system makes possible: diegetic objects doing the work a UI meter does elsewhere, appearing in the world at high enough standings to matter.

**[OPEN QUESTION]** Resolved in direction (2026-07-05): the layers are **per-faction weighted** (values above), and account-to-account relations are set by **formal alliance declaration** with a **100% override**. Still open: the exact weight values (future balance); faction count and granularity; decay and growth rates; who may declare an alliance for an account (any member? a guild/family head?); and where the political/lore faction roster is authored (Book VI).

---

## Account, Friends, and Co-op Building

*(Account-social layer, 2026-07-05. Sits alongside factions and the tapestry as the third face of the account: factions are how the *world* reacts to you, the tapestry is your *family*, and this is your chosen *social graph* with other players.)*

**The friends list and communication.** An account carries a **friends list** — a personal social graph, distinct from both factions and the family tapestry: who you choose to keep track of and play with. Players communicate by **text chat** in-game and through a **companion mobile app**; **proximity voice chat** is a likely later addition (idea-grade).

**Co-op building runs on a rights system.** Building is not strictly solo. A claim's owner can **grant other accounts access to their claim** at **graduated degrees of control** — a security-and-rights system, not an all-or-nothing share. The rights span a range the owner sets per grantee:

- **Edit** — place and remove build elements directly (the voxelmancy/template tools, Book II).
- **Operate the roster** — assign the settlement's NPCs to tasks, change their jobs, reset their priorities (the NPC roster of Character Lifecycle).
- **Interact** — the minimal tier: use and "touch" things without altering the build or the workforce.

This is the machinery by which friends, declared allies, and bound families (Factions and Standing; Tapestries) actually build together on shared ground, and it keeps a claim's control in its owner's hands by default. **[OPEN]** the exact rights tiers, whether they nest or combine freely, revocation, and how co-op edits reconcile with the Dungeon Maker construction grammar (Book II) are undesigned.

---

## The Advent Raids — Endgame Horizon

*(Horizon content — [PROPOSED], scheduled for the v1.1–v1.5 range, not launch. Ledgers 021891E4, A573A152, 639B9FD9; the zones themselves are Book V/III material.)*

The apex of adventuring is the **Advent raids** — the pentagon resonance zones (Book V), reached not by travel but by **quest**: physical travel there is not meant to happen (the pentagons are unreachable by design). Entry needs the **skill to teleport a group in** and **material attunement**, and the **first trip is roughly ten times harder than returning**, since it yields the materials that ease the way back. Play is EverQuest-classic — **camp the zone, send scouts** — at **72-person, week-long** scale, in **contested, non-instanced** space.

Each zone is an **abstract, rule-shifting plane** — the seven color-realms rendered as places that express their nature (the Void where you swim in all directions; a Chaos plane that rewrites itself; an Air realm with no floor), plus surreal challenge spaces (an infinite city where one step crosses light-years; Escher geometry; Geiger horror). **The zone's strange rule *is* the challenge** — and the fairness law still holds: insane, but legible to anyone open enough to perceive it (the resonance/perception principle, Book VI).

**These zones are curated content — and top-tier DM dungeons are the doors to them** (ruled 2026-07-03, ledgers CA50B875, 8E571A6E). Raid-zone construction is the deep end of the complex **DM tech tree** (Book II): the highest tier of Dungeon Makers unlocks extremely-high-level raid construction — many tech unlocks, high rating required — and, with enough advancement, the capability to **place an Advent gateway at the end of a dungeon**. The chain is strict: **the dungeon must be defeated to unlock the Advent behind it** — clear first, then the gate opens; the means to advance are available to any Maker willing to climb. The Advent zones themselves are **future design: carefully curated puzzle zones — essentially the game's only designed, fixed content** in an otherwise procedural, player-built world — authored by the community or as Elideus's "personal imaginarium of advent insanity projects."

> **[OPEN QUESTION]** Whether and how the **Advent eclipse** (the celestial event) gates or reshapes a zone — the sky's hook into the raids (ledgers 35A9CEAE, 3312B2AB); the mapping of the twelve pentagon points to the destination realms (73EA6D39); in-zone resurrection and party-composition rules.

---

## Character Lifecycle and the Generational Dynasty System

An adventurer is one life in a family line. The character system is **generational**: a **family tapestry** records the line; characters leave active play through defined **retirement flows**; and the peoples of Prism trade lifespan against generational turnover — longer-lived races trade turnover for continuity, shorter-lived races cycle through the family tapestry faster. (Ledger 10A6150F, Seed canon 12; corroborated by Book V, Peoples of Prism.) The races themselves are Book V material and are currently a stub there.

**Mortality is the spine of the life (Elideus, 2026-07-13; ledger 07F4289D; recovered adventurer's-system design, `sources/legacy-adventurers-system-recovered.md`).** A character is **born, lives, ages, and dies — and death *matters*.** Lifespan varies by race and circumstance (race-as-tempo, above; Book V, Peoples). Ordinary combat death is never the end: you respawn and recover your body (Combat Framework; ledger 0B98560B). The **one death that cannot be cheated is age** — a character not first given another disposition will, in the fullness of time, grow old and die, and unlike combat death that death is real and final. Its moment, and the character's last acts, are recorded permanently in the Tapestry. Everything that removes a character short of old age is either a *chosen* terminal disposition (below) or the rare, extremely costly sin of Burning; age is the death no one escapes and no magic reverses. This is the design's hardcore-RPG spine: characters are not disposable alts but permanent threads, so beginning a new life is itself a weighty, permanent choice.

**A life moves through stages, and its roles move with it (recovered design, 2026-07-13).** As a character ages they drift out of adventuring and into the settled roles the line needs. In **youth** they are adventurers, explorers, and mercenaries; in **middle age** they turn to crafting, strategy, architecture, and governance; in **old age** to scholarship, sagecraft, trade, and eccentric mastery; and the rare **elder statesman** becomes a manor-lord, cult-founder, or living legend of rare influence. Each stage is a different way to serve the family, and the terminal dispositions below are how a life — at whatever stage it ends — is deposited onto the account. The building and roster systems below are where those later-stage roles are actually played out.

**Aging is a resource, and hard magic spends it (Elideus, 2026-07-13; ledger 07F4289D; recovered design, ruled canon).** Time is not the only thing that ages a character. The heavy use of **wild magic and psionics**, the weight of **curses**, and the toll of **resurrection** all accelerate aging or destabilize the body — power drawn hard is paid for in years, the same no-free-lunch law as mana upkeep, invested children, and the no-respec build (ledger 91F84075). This makes the racial **longevity** axis matter all the more: a long-lived vessel can spend where a short-lived one cannot afford to, the longevity-gated third ring (Book IV, ledger 35C6066B) grows harder still under a spent-down clock, and the endgame's near-immortal-race requirement (ledger EC054473) reads partly as the margin that aging leaves. Exact rates are `[BUILD]`.

Two further facts are canon (ledger EC054473):

- **Every new game begins with your first character, a settlement, and three family NPCs — Mother, Father, and Patron** ("Patron" and "benefactor" are the same role; named 2026-07-05). You **found a settlement** at the start — a **claim**, in the Landmark sense — and your three NPCs live there. You **create them the way you create your character**, but they are always NPCs. The opening is survival-hard: a hostile world, scarce resources, a weak character, and nothing in hand but the people standing with you. These three plus your adventurer are the tapestry's founding banners.
- **Retirement is a category, not one ending** (enriched 2026-07-02, ledger 0B98560B). A terminal exit has **many dispositions, and every one *adds* to the account** — the design never takes a character away, it grows the family: **retire into a settlement as an NPC** (this is how a city grows), **die heroically** for special status, **die of old age** to become a **patron of legend**, or walk the Muse's path (the apex disposition; see The Endgame Arc). Each deposits a permanent marker on the account's tapestry, and the **roster is limited only by time** — an account played for years can hold epic power in the sum of a large, disposed roster.

**The starting camp teaches the whole system (Elideus, 2026-07-05).** Those three NPCs at your claim are the player's first lesson in what **retirement** does: a retired character is **added to the settlement's NPC roster** the same way — which is why the game hands you an NPC household before it ever asks you to retire anyone into one. The roster *is* the living settlement (the "retire into a settlement as an NPC — this is how a city grows" disposition above), and its members can be **assigned jobs and tasks** according to the skills you gave them:

- **Mother and Father** are the domestic and support core: they gather food and supplies (you are periodically restocked from what they bring in), repair and mend, feed and clothe you, and fight off animals roaming near the claim. What they can actually do depends on the skills you designed into them.
- **The Patron / benefactor** is different — a **broad-effects** NPC, not a camp laborer, and **often away**: they travel, return, and confer ongoing benefits — faction membership, access to places or networks others can't reach, special items, buffs, and reputation that grows over time. The archetype is a **lore hook the player fills**: a **trader uncle** (better prices, and entry to other players' bazaar spaces — a private trader-network); a **military cousin** (weapons others can't get); a **spy grandfather** (access to the secret-keepers). The world's factions and networks reach the player through *who their benefactor is*. This is also how a line **builds** family-wide standing (below) — the constructive counterpart to how a single bad actor can tear it down.

Growing that reputation and **planning your NPC roster** — who you retire into which role, which benefactor threads you into which network — is itself long-game play. The endgame is all LTOs (see *Long-Term Objectives*).

The tapestry is also the game's real achievement system: prestige lives in-world as permanent account markers, not in platform popups (ledger EC054473).

The lifecycle's economics are ruled (Elideus, 2026-07-02; ledger 91F84075):

- **The child.** You add an offspring by a **symbolic interaction with the Tapestry** — a family choice, not simulated biology, requiring **no gender and no romantic pairing** (the line grows inclusively, by intent; recovered design, 2026-07-13). From that choice an offspring is an investment requiring maintenance: ongoing payment of in-game currency builds the child to adventurer's age — an incubation the currency represents — and a raised child is what a new character is eventually rolled from, inheriting traits, gear, or quests (see the inheritance fork below). The investment need not begin the moment they come of age — but it cannot be skipped and cannot be bought around: **nothing is pay-to-play, ever.** "Old school RPG for life."
- **The adventurer.** The character you play — **no take backsies**. Skill them up, live, explore, learn, acquire; at **level 10** comes a yearning to retire: you recognize your strengths and choose your benefits for the family, as permanent effects. Other endings feed the same system — a heroic sacrifice in an epic battle can become a permanent badge of glory for the account; any number of effects can be built off this system.
- **Enchanted items obey the same law.** Long-term enchantments require regular mana investment at the enchanting table. You can't fake anything — the same rule as the child, the claim, and the dungeon: nothing persists that no one maintains.
- **Philosophy expresses in the settlement** (ledger E4F46BA5, stated as likely-intent). The pole a player's line follows shapes their NPCs — choose selfish traits and your settlement grows more hostile to outsiders; communal traits confer different benefits. Strengths and weaknesses both ways, and the design passes no judgment: which path is right is the player's own conclusion.

**Character creation is background-driven (Elideus, 2026-07-05).** A character is built, not merely rolled, in two layers. A **guided creation route** poses ethical dilemmas whose answers drive starting stats — the Ultima virtue-questionnaire lineage, where who the character *is* falls out of choices rather than sliders. Over it sits a **background builder** in the GURPS advantages/disadvantages register: the player selects aspects of the character's history, and each confers advantages and disadvantages derived from that history. Background is chosen and built, not random. (Direction, not a final system.)

**Banners grow the roster (Elideus, 2026-07-05).** The family tapestry **is** the character-select screen, and each character occupies a **banner** on it. You expand your roster by **investing to grow a new banner** — a slot that, once grown, you fill with a new character and a new background of your choosing, always part of your account's single **Family**. Growing banners is the sanctioned way a roster gets larger, and like everything it costs real investment (nothing pay-to-play).

**One Tapestry, many branches (Elideus, 2026-07-13; ledger 07F4289D).** There is **one Tapestry per account**, but within it the banners can form **distinct branches** — separate family lines, cultural themes, magical disciplines, or political roles growing on the single cloth. The variety an earlier design chased through *multiple* Tapestries (up to five separate legacies) lives instead on branches of the one: the merit of the idea is kept, its UI and implementation cost is not (recovered "Tapestry Multiplicity / Split / Merge" idea, deliberately compressed). Opening a new branch or **family slot** is itself **limited and earned** — the same invest-to-grow-a-banner cost, never free and never many at once. Merging in an outside line is the cross-account **tapestry-binding** below; splitting or re-organizing branches within your own Family is a management affordance, not a way to discard a story (nothing is ever deleted but by Burning).

**Tapestries can be united (Elideus, 2026-07-05).** Growing a banner need not be a solo act: you can **bind a character from another account** into your tapestry, **growing the banner through combined contribution** and, in return, **gaining benefits from the other family's association** — its standing, its networks, its benefactors. This is how two players' lines formally join — the dynasty-scale counterpart to the account-level alliances of *Factions and Standing*, and a concrete answer to how members join beyond invested children and adoption. What the binding costs, whether it is reciprocal, and how a bound character's own account relates to the shared banner are undesigned.

**The dead endure as ghosts, and ghosts stay reachable (Elideus, 2026-07-13; ledger 07F4289D; recovered design, ruled canon — naming dispositions under the open category of ledger 0B98560B).** A character who dies passes to a **ghost state** on the Tapestry, rendered as a **grayscale portrait** — a legacy entity rather than a living presence, their final acts recorded. Ghosts are not gone: **common mid-tier magic** to commune with or summon the deceased is widely available, so a line can readily speak with its elders, seek guidance, and recover knowledge tied to the family (this is the canon form of the "ghost guidance" bloodline hook below). Ongoing play acts on ghosts, too — **honoring the dead** (statues, dedicated places, rites) empowers a ghost; **tethering** binds one to an object, building, or location; and high-tier ritual permits **possession or projection**, a temporary embodiment or a hand on present events. A ghost is also the state a truly problematic retiree sits in, and Burning (below) is the act of erasing one.

**A few of the dead transcend into immortal dispositions (Elideus, 2026-07-13; ledger 07F4289D; ruled canon — the apex of the additive category).** Beyond the ordinary ghost, rare and hard-won ends carry a character further: **divine ascension** to demigod or godhood through legendary deeds; **mythic resurrection**, a return to life bearing permanent change; the **eternal guardian**, a spirit bound to a place or object as its protector and font of wisdom; the **permanent haunting**, a spirit that becomes a fixed, cursed presence shaping a location indefinitely; and **imbuement**, a soul fused into a relic or arm so the item wakes, speaks, and demands reverence from those who would wield it — the adventurer-side face of legendary items (Book II). These transcendent dead are marked on the Tapestry not in grayscale but in **radiant or color-shifted overlays**, and interacting with them may demand divine magic or specialized access. The **Muse's path** (The Endgame Arc) is the maximal member of this same category — the personal ascension that retires a character yet leaves the shared world standing; a recovered "personal ascension into a world-spanning deity" is its **superseded ancestor**, folded into the Muse's arc rather than kept as its own path (2026-07-13). All of these remain **additive** — they grow the account; only Burning subtracts.

**You retire characters; you do not delete them (Elideus, 2026-07-05).** A character leaves active play by **retiring** — additive, keeping its banner and depositing a marker on the tapestry, per the dispositions above. Permanent deletion is deliberately *not* the normal path:

- **Burning a banner** — erasing a character from the tapestry entirely — is available **only as a difficult activity**, an LTO in its own right: you must *learn how to burn a ghost from your tapestry*, and it is hard and slow by design.
- It is also a **sin.** Permanently burning a banner is treated in-world as an **abomination** — **one of the very few moral judgments the gods pass on players at all**, and the deliberate exception to the design's otherwise-unjudged stance (ledger E4F46BA5; the "we do not preach, we reveal" principle bends here and nowhere lightly). Reserve it for a character whose disposition is *truly* problematic to the family's larger goals.
- *The archetypal case:* your line is Blue-aligned and deep into an epic Blue LTO that requires questing alongside a particular Green faction — but one family member once did enough damage in that faction's eyes that the **whole family is disliked** and the quest is walled off. Resolving that — by mending the standing, or at the extreme by learning to burn the offending ghost from the tapestry — is itself a Long-Term Objective.

This refines Burning (ledger 0B98560B): its **banner-erasing form is the sinful abomination** described here, while its **sacrifice-play form** — spending a character for a lasting *positive* effect (a heroic sacrifice that leaves a permanent badge, a world-shifting LTO) — leaves a legacy and is its honorable counterpart, not a sin. **[PROPOSED distinction, 2026-07-05.]**

> Faction standing is **shared across the family tapestry** — the **family/tapestry layer** of the faction system (see *Factions and Standing*): built up over generations, since a benefactor threads the line into a faction or network (above) and characters' deeds raise standing — and torn down the same way, since one character's misdeeds taint the whole line's standing (which is what makes a single bad disposition a family-level problem — the archetypal burn-the-ghost case above).

> **[PROPOSED]** A wider thread-state sketch exists from the same session — Living → Retired (role computed from the life actually lived) → Dead (tombstone) → Ghost (earned re-activation) → Burned (spent forever for a lasting effect); race as a tempo dial, lifespan trading against offspring slots; multi-generation dynasty tracks unlocking bloodline entitlements. Elideus's verdict was "not quite what I had in mind, but pretty close" — unratified. (Ledger A8FEC45C.) **Burning is now clarified** (2026-07-02, ledger 0B98560B): it is the **one subtractive exit** — either a **permanent delete** of the character from the tapestry, or a **sacrifice play** spending the character for a lasting effect — and either way it is meant to be **extremely costly**, the deliberate exception to "we never take a character away." **Update (2026-07-05):** the sketch's **Ghost and Burned** states and the **banner** unit are now in active use (see Banners and the retire-vs-burn rules above) — a ghost is the tapestry-state a problematic retiree can sit in, and burning is the act of erasing it; that part of the model is firming, though the full state machine and dynasty tracks remain unratified. **Update (2026-07-13):** the **Ghost state is now canon** as the *default* disposition of any character who dies of age (grayscale portrait, reachable by common commune magic, honorable/tetherable/possessable — see *The dead endure as ghosts* above), and the sketch's "earned re-activation" reads as the possession/projection and spectral-echo interactions on that state; **Dead (tombstone) → Ghost** collapses toward a single legacy-entity state with dispositions, and the transcendent (ascended/immortal) dead are its radiant-overlay branch. The full state machine and dynasty tracks still remain unratified.

> **[PROPOSED] — recovered inheritance and bloodline hooks (2026-07-13, `sources/legacy-adventurers-system-recovered.md`; candidates for the open inheritance fork D2C52C88, *not* a resolution of it).** The recovered design names concrete things a line might pass down — **heirlooms, journals, spells, titles, and unfinished quests** — and holds that **traits** may be inherited or diluted across generations. It also names bloodline *narrative* hooks: **areas locked to a particular bloodline**; **resurfacing relics** (artifacts tied to past characters, recovered and reactivated by descendants); **ancestral quests** that unfold across generations (already a canon cross-generational LTO — see Long-Term Objectives); and **ghost guidance** to descendants (now canon via the ghost-communion mechanics above). These are candidate answers feeding D2C52C88 and sit alongside the [PROPOSED] EVE-style account-wide bloodline entitlements (ledger 5A4E7B22); the fork stays open until Elideus rules what actually transfers.

> **[STUB]** The mechanics of the lifecycle and dynasty system are unrecorded beyond the facts above.
> - **[OPEN QUESTION]** What transfers across generations — items? school mastery? reputation? ring progress? (Ledger D2C52C88.)
> - **Resolved (2026-07-02, ledger 0B98560B):** ordinary combat death does **not** end a life — you respawn and recover your corpse (EQ-style). A life *leaves active play* only through a terminal disposition (retirement at level 10's yearning; heroic-sacrifice badge, ledger 91F84075; old-age patron; the Muse's path) — all additive — or through **Burning**, the one subtractive exit (delete or sacrifice; extremely costly).
> - **[OPEN QUESTION]** Is the family tapestry a literal in-game artifact and interface, or a structural name for the family tree? Unrecorded.
> - **[OPEN QUESTION]** How members join, partially answered (2026-07-05): invested children (ledger 91F84075), adoption (the Muse-vessel gate, EC054473), and now **cross-account tapestry-binding** (above) are the known paths. Still open: the binding's costs and reciprocity, ordinary marriage/recruitment, and the unit of inheritance (one-parent, whole-tapestry, or chosen-mentor — posed unanswered, ledger A8FEC45C).
> - **[OPEN QUESTION]** Does one player steward one line, or several? Unrecorded.

---

## Progression

The only progression fact currently ratified is economic: an uncompletable dungeon yields no XP and no traffic (ledger 49A3A9D6, Seed canon 8). Experience exists, and completing dungeons is at least one source of it; fairness is enforced by that incentive structure, not by moderation.

**Experience is the adventurer's alone, and it is delivered as a story, not a number.** Only adventurers earn experience, and only from beating completable content (ledgers 02D30987, 49A3A9D6): the builder crafts the challenge; the hero who overcomes it is paid. But the payment is never a popup. Instead the game **logs what a character does while away from their settlement**, and when they return home it writes a **permanent, generated journal entry** — a diary reporting the day in vague, human terms: *"I felt stronger and more competent with my broadsword after today's adventure"* might stand in for several thousand experience and two skill-ups (ledger BFBE6085). The intent is to detach the player from watching the numbers — aggregate narrative recaps, the way a session ends at a tabletop.

Three further facts are canon (ledger 91F84075): **levels exist, and level 10 is the retirement threshold** — the point at which the yearning to retire arrives; the **skill system is deep, complex, and completely opaque to the player**, learnable only through brutal trial and error, while remaining honest — deterministic, predictable, and fair (the fairness law, applied to progression); and **the rules never bend** — costs are paid in full over real time, with no accommodation for schedules and nothing pay-to-play, ever.

The skill system's shape is sketched but undesigned (ledger F618F695): **each discipline is a skill**, and skills are **layered** — "learning about axes teaches about wood and farming and gardening and combat" — with "probably three or four layers of skill scopes." Flagged by Elideus as a **major open issue for Sonnet collaboration**; nothing here may be elaborated until that session.

**The ring progression belongs to Book IV, not here.** Rings are a magic-mastery structure internal to spell research — a per-school milestone gating vocabulary breadth, spell-string complexity, and reagent grade, with cross-school synergy access (4th/7th ring) as one instance of it — resolving fork 63B28A6B in favor of Option B (ledger B92EE74A; shapes themselves ungated as of 2026-07-05). Book I tracks whole-character levels and retirement (above) as a separate axis from a mage's per-school ring standing.

> **[OPEN QUESTION]** How ring advancement interacts with the unresolved synergy-cycle orderings (ledger 244A27FF) and with Black's exclusion from ring-based synergy (Book IV; ledger 442D03E8 — Black combines with nothing).
> - **[OPEN QUESTION]** Whether ring progress transfers across generations (ledger D2C52C88).
> - **[OPEN QUESTION]** Progression beyond rings — levels, XP curves, skill training — is entirely unrecorded.

---

## Long-Term Objectives — the Endgame Layer

**Canon (Elideus, 2026-07-05):** Prism's endgame is **not content in the traditional sense** — not a weekly treadmill of instances and gear tiers to consume. The endgame is **Long-Term Objectives (LTO)**: pursuits measured in real effort and real time, not clears per week. This is the "Long-Term-Objective layer" the stats table points to (Intelligence's research gate, *The Seven Stats*), and it is the game's actual apex. In Elideus's words, *"LTO is the game."*

An LTO is marked by one or more of:

- **Once-only** — things that can be done a single time, ever, on an account or in the world.
- **High-effort / many-attempts** — objectives demanding enormous work, or many failed runs before one succeeds.
- **Lifelong** — epic quests spanning a character's whole playable life, or several.
- **Cross-generational** — things a character does specifically to unlock possibilities for their *descendants* rather than themselves (the dynasty tapestry above; open inheritance fork, ledger D2C52C88).

The systems already ruled are LTOs in exactly this sense, not separate features: the terminal dispositions that each *add* to the account (retire-as-NPC, heroic-sacrifice badge, old-age patron — ledgers 0B98560B, 91F84075); **Burning** a character as a sacrifice play for a lasting effect (the one subtractive exit); and the **Muse's-path endgame arc** (below) — the canonical maximal LTO, seven retired lifetimes and a Chalice eclipse spent on a single terminal choice. They are instances of one endgame philosophy.

**[OPEN QUESTION / idea-grade] World-shifting LTOs — reframed as *local*, leaning plausible (Elideus, 2026-07-13; ledger 75FA3234).** Elideus floats a rarer class: objectives whose completion alters the persistent world itself. The illustrative idea — *a lifelong quest whose final step demands the character sacrifice themselves and, in doing so, raises a new mountain permanently tagged as belonging to that account* — is no longer read as the ruled-out class. The line is **local vs global**: **player-driven *global* effects stay ruled out** (ledger FEC89726 — the world changes at scale only from the deterministic sky, celestial primacy 3312B2AB; "permanent world-scale change is not simulated," The Endgame Arc, ledger EC054473), but a raised mountain is **local** — "a small section of a single hex on a planet of ~40,000 hexes" — the same order as the buildings players already raise and the emergent terrain the sim already produces (Book III, §Compaction and Emergent Terrain). So it is **not the forbidden class; it leans plausible** — though Elideus stops short of committing (*"maybe not quite that extreme, but I can't rule it out"*). Still open: how extreme a local change the design will admit, its cost and gating (**rings-beyond-7 / expansion / "forbidden fruit" territory**, Book IV), and the reach into the geology simulation itself (Book III). The Muse's path stays the world-*untouched* apex; this would be its world-*marking* cousin.

---

## The Endgame Arc

The endgame of the adventurer's game spans character lifetimes: **master a school of every color across the generations of a family line, and, during a celestial alignment, reunite or re-shatter the Prism.** (Ledger 10A6150F, Seed canon 12; "all seven schools" ruled to mean at least one school from each color — ledger A8FF6AB0.) It is the canonical maximal **LTO** (above).

Celestial events are global modifiers, deterministic and foretold: an alignment is never scripted or random — it emerges from real orbits, and those who learn the heavens can predict it (ledger 8FB95281, Seed canon 6). The endgame's moment is therefore something the whole world can see coming — and since the moon runs on real wall-clock ephemeris (Book V; ledger D2AAFEEC), the appointment is computable from the actual sky.

**The arc is the Muse's path** (ruled 2026-07-01/02; ledgers BDB707A5, EC054473). The player walks the road the Lonely Muse walked — apprentice across all seven colors in one lifetime, mastering at least one school of each (ledger A8FF6AB0), and stand at the summit she was denied (Book VI). Completing it is a **terminal epic quest**: the choice between reuniting and re-shattering is offered, made personally, and **retires the character**. The shared world persists — permanent world-scale change is not simulated; "that's where a character would retire." Nobody does this by accident, or on their first character, or their tenth.

The gate stack is canon (ledger EC054473):

1. The account's tapestry must already record a **leveled and retired character in each of the seven colors** — at least one school per color, seven completed lifetimes as the price of eligibility (ledger A8FF6AB0).
2. The vessel must be a **near-immortal race** (an elf-type with five human lifetimes of learning done by age one hundred) — or a character of another race prepared through an adoption setup and massive tutoring investment.
3. The quest must be completed **during a celestial eclipse of the Chalice** — the moon–Death alignment that reveals the Muse's hidden constellation (Book VI; ledger AF819A62).
4. The reward is a **permanent account marker on the tapestry** — a major permanent account-wide bonus ("a permanent patron of all magic, something along those lines").

*[Resolved 2026-07-02: "seven schools" means at least one school from each color — fourteen is excessive by ruling. Ledger A8FF6AB0, closing FCB93BC0.]*

> **[OPEN QUESTION]** **Reunite vs re-shatter — the meaning.** The mechanical scale is ruled (personal, terminal); what each choice *means* — mythologically, and in whatever the retiring character leaves behind — is unrecorded. The mythic frame gives stakes without specifics: reunification reads as restoring the unity the Titans broke; re-shattering as renewing the containment for another age. What the choice options actually are, and whether factions align to them (Book VI, Those Who Remember the Muse), remains open.
>
> **[OPEN QUESTION]** **Inheritance beyond mastery.** School mastery accrues at the account level — one retired character per school (ruled above). What else transfers across generations — items, reputation, ring progress — is still the open inheritance fork (ledger D2C52C88), as is what combat death costs the line (ledger D25A57E1).

Related forks held elsewhere:

- Whether "all seven schools" includes psionics — a discipline outside the seven schools — is unresolved (ledger 7609E7E9).
- Black combines with nothing (ledger 442D03E8) and is excluded from ring-based synergy (Book IV). Whether the path to Black mastery differs structurally from the other six schools is unrecorded.

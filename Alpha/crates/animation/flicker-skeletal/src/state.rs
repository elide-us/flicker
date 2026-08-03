//! Animation / combat **state machine** + **TAE event timeline** (combat-spine slice 1).
//!
//! This is the first slice of the combat spine designed in
//! `docs/flicker-combat-animation-handoff.md`. It sits ON TOP of the CPU-authoritative
//! pose layer (`pose.rs`) — it decides *which clip plays and at what tick*; `pose.rs`
//! turns that into bone transforms.
//!
//! Shape:
//! - A **state graph**. Each state names a clip (later: a blend) and carries a list of
//!   input-gated **transitions** and a **TAE event timeline**.
//! - Advanced on a **fixed 60 Hz tick** — the same clock the clips are baked to (the
//!   fixed-tick combat clock from project canon). [`StateMachine::tick`] is the atomic
//!   step; [`StateMachine::advance`] accumulates a frame's `dt` into whole ticks.
//! - The **TAE event timeline** is authored, tick-stamped combat metadata (hitbox
//!   windows, cancel/combo windows, i-frames, footsteps, sfx, …). The runtime FIRES the
//!   events for the ticks it crosses and reports the windows currently active; *acting*
//!   on them (hitbox capsules, i-frame invulnerability) is a later slice. Building the
//!   timeline as the spine from day one is deliberate — canon points at TAE-as-authority,
//!   not a plain enum machine with events bolted on later.
//! - Transitions **hard-cut** for now (reset the play-head, swap the clip). Crossfade
//!   blending is the next slice.
//!
//! The graph is authored as a **separate `flicker.pack` JSON** so `flicker.rig` stays a
//! purely mechanical FBX-derived atom — combat data lives in the pack, not the rig.
//! Clips are referenced by **name** and resolved to indices at [`StateMachine::build`]
//! time, mirroring how the rig loader resolves clip tracks to bones by name.
//!
//! Several fields (e.g. `root_motion`, window labels) are recorded now for later slices
//! (capsule locomotion, hitbox binding) — hence the module-wide dead-code allowance.
#![allow(dead_code)]

use std::collections::HashMap;
use std::path::Path;

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

// ─────────────────────────────── authored (wire) types ────────────────────────

/// A content pack file. For this slice we only read `state_machine`; later slices add
/// the rig reference, mesh/skin variants, weapon packs, and ability defs alongside it.
#[derive(Debug, Serialize, Deserialize)]
pub struct PackFile {
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub format: String,
    #[serde(default)]
    pub version: u32,
    /// Hand-authored provenance/doc note (`_note` key). Round-tripped so the editor's
    /// save never erases it (there is no `deny_unknown_fields`, but this is the one
    /// non-schema key real packs carry).
    #[serde(default, rename = "_note", skip_serializing_if = "String::is_empty")]
    pub note: String,
    pub state_machine: StateMachineDef,
}

/// The authored state graph.
#[derive(Debug, Serialize, Deserialize)]
pub struct StateMachineDef {
    /// Name of the state to start in.
    pub initial: String,
    /// Crossfade duration (in ticks) applied to any transition that doesn't set its
    /// own `blend_ticks`. `0` (the default) = hard-cut everywhere unless a transition
    /// opts in — so an un-annotated pack keeps the original snap behaviour.
    #[serde(default, skip_serializing_if = "is_zero_u32")]
    pub default_blend_ticks: u32,
    /// Tick rate (Hz) of the clip library this pack drives — the clock its clips'
    /// `duration_ticks` are baked to. Defaults to 60 (the legacy Katanami export rate).
    /// The Motifect retarget bakes at 30, so the Prism pack sets `tick_rate_hz: 30`;
    /// otherwise the machine advances a 30 fps clip at 60 Hz and plays it double-speed.
    #[serde(default = "default_tick_rate_hz", skip_serializing_if = "is_default_tick_rate")]
    pub tick_rate_hz: u32,
    /// Transitions evaluated from **every** state, before the per-state ones — the
    /// "from any state" edges (hit reaction, death). Highest-precedence.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub any: Vec<TransitionDef>,
    pub states: Vec<StateDef>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct StateDef {
    pub name: String,
    /// Clip name to play in this state (resolved to an index at build).
    pub clip: String,
    /// Does the clip loop? Locomotion/idle loop; attacks/reactions play once.
    #[serde(default = "default_true", skip_serializing_if = "is_true")]
    pub looping: bool,
    /// Auto-advance to this state when a non-looping clip completes (e.g.
    /// `Jump_Start` → `Jump_Loop`). A convenience alternative to a `clip_done`
    /// transition; explicit transitions of higher priority still win.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub next: Option<String>,
    /// Locomotion authority: `true` = the clip's root track drives translation
    /// (opt-in, for climbs/specials); `false` (default) = in-place, capsule-driven.
    /// Recorded now; the capsule mover lands in a later slice.
    #[serde(default, skip_serializing_if = "is_false")]
    pub root_motion: bool,
    /// Stamina paid on ENTRY, not at a tick — a combo's second swing is its own state, so
    /// the cost is a property of entering this state rather than a window inside it.
    #[serde(default, skip_serializing_if = "is_zero_f32")]
    pub stamina_cost: f32,
    /// This state is holding guard. Blocking is a HELD state, not a window: the
    /// block→stamina conversion is computed by the resolver from the incoming hit and is
    /// never authored per-tick.
    #[serde(default, skip_serializing_if = "is_false")]
    pub blocking: bool,
    /// Half-angle (degrees) of the guard arc while [`Self::blocking`]. Absent = the
    /// resolver's default arc.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub guard_angle: Option<f32>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub transitions: Vec<TransitionDef>,
    /// The TAE event timeline for this state's clip.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub events: Vec<EventDef>,
}

fn default_true() -> bool {
    true
}

fn default_tick_rate_hz() -> u32 {
    60
}

// `skip_serializing_if` predicates so a saved pack omits schema defaults and keeps the
// clean, hand-authored shape (no `looping: true`, `priority: 0`, `next: null`, `[]` noise).
fn is_zero_f32(v: &f32) -> bool {
    *v == 0.0
}

fn is_zero_u32(v: &u32) -> bool {
    *v == 0
}
fn is_zero_i32(v: &i32) -> bool {
    *v == 0
}
fn is_true(v: &bool) -> bool {
    *v
}
fn is_false(v: &bool) -> bool {
    !*v
}
fn is_default_tick_rate(v: &u32) -> bool {
    *v == 60
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TransitionDef {
    /// Target state name.
    pub to: String,
    /// Input/condition that gates this transition.
    pub on: Trigger,
    /// Only permit the transition while the play-head is inside this tick window — a
    /// cancel/combo window. Absent = allowed any time.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub window: Option<TickWindow>,
    /// Higher priority is evaluated first (default 0). Ties keep authored order.
    #[serde(default, skip_serializing_if = "is_zero_i32")]
    pub priority: i32,
    /// Crossfade duration (ticks) into the target state, overriding the machine's
    /// `default_blend_ticks`. `Some(0)` forces a hard cut (e.g. a snappy hit react);
    /// `None` (default) inherits the machine default.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub blend_ticks: Option<u32>,
    /// **The reactive transition.** Admissible only while an *incoming* attack of this
    /// shape is live — the counter-move answer (step on the thrust, leap the sweep).
    ///
    /// Distinct from [`Self::window`], which gates on the actor's OWN playhead: this gates
    /// on someone else's. The client is never told a counter window is open, so it must
    /// read the animation and the server decides whether the answer was available —
    /// cheat-proof by construction, because there is no flag to read.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub on_incoming: Option<HitType>,
}

/// An inclusive `[start, end]` tick interval.
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct TickWindow {
    pub start: u32,
    pub end: u32,
}

impl TickWindow {
    fn contains(&self, t: u32) -> bool {
        t >= self.start && t <= self.end
    }
}

/// What can gate a transition. Movement modifiers (`move`/`run`/`crouch`) and the
/// directional variants are **held** states; `jump`/`attack`/`hit`/`die` are **edges**
/// (true only the tick pressed); `clip_done` fires the tick a clip completes.
///
/// `move` = moving in any direction. The directional forms combine movement with a held
/// direction: `move_forward` = moving with no side/back held, `move_left`/`move_right`/
/// `move_back` = moving with that direction held. Used to select strafe/directional
/// locomotion clips (a discrete-state stand-in for a 2D locomotion blend space).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Trigger {
    Move,
    MoveStop,
    MoveForward,
    MoveLeft,
    MoveRight,
    MoveBack,
    Run,
    RunStop,
    Crouch,
    CrouchStop,
    /// Crouch + run held together — a **combo/decision-matrix** trigger: while crouched,
    /// the run modifier (Shift) is overloaded from sprint to *stillness* (a crouch-idle
    /// "challenge mode"). Fires when both `crouch` and `run` are held.
    CrouchStill,
    Jump,
    Attack,
    Hit,
    Die,
    ClipDone,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EventDef {
    /// Tick this event fires at (a one-shot) or the start of its window.
    pub tick: u32,
    /// Inclusive end tick for a **window** event (hitbox-active, i-frames, cancel);
    /// absent = a one-shot at `tick`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub end: Option<u32>,
    pub kind: EventKind,
    /// Free-form tag — a bone name for a hitbox capsule, an sfx id, a foot label, …
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub label: String,
    /// Optional souls-combat metadata (hitbox capsule + damage, cues). Absent on plain
    /// locomotion events, so existing packs are unaffected.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub combat: Option<CombatMeta>,
}

/// The souls-like combat-metadata vocabulary carried on the timeline. Fired/reported
/// by this slice; consumed (capsules, invulnerability, …) by later slices.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EventKind {
    Footstep,
    /// A hitbox capsule (bound to `label` bone) is live for this window.
    HitboxActive,
    /// Invulnerability (dodge/roll i-frames) for this window.
    Iframe,
    /// Cancel/combo window — the feel of souls combos.
    CancelWindow,
    /// Parry / deflect window. **Its `tick` is the highest-stakes number in a pack:**
    /// clip-start → catch-window-open is the server's commit horizon, so `min(tick)` across
    /// every parry state sets the netcode latency budget.
    Parry,
    /// Poise cannot be broken for this window — the attacker trades stagger for commitment.
    HyperArmor,
    /// The attack is announced for this window: the readable warning a perilous attack owes
    /// the player. Its width is the player's reaction budget.
    Telegraph,
    Sfx,
    /// Weapon appears in hand / ground pickup swap.
    Equip,
    /// Weapon-trail VFX on for this window.
    WeaponTrail,
}

impl EventKind {
    /// Short HUD tag.
    pub fn tag(self) -> &'static str {
        match self {
            EventKind::Footstep => "step",
            EventKind::HitboxActive => "HITBOX",
            EventKind::Iframe => "IFRAME",
            EventKind::CancelWindow => "CANCEL",
            EventKind::Parry => "PARRY",
            EventKind::HyperArmor => "ARMOR",
            EventKind::Telegraph => "TELL",
            EventKind::Sfx => "sfx",
            EventKind::Equip => "equip",
            EventKind::WeaponTrail => "trail",
        }
    }
}

/// Souls-combat metadata authored onto a TAE event — the hitbox capsule + damage numbers
/// and the feedback cues the TAE editor's inspector edits. Every field is optional so a
/// plain `footstep` carries none and pre-existing packs deserialize unchanged.
///
/// **Authored only.** The state machine still merely FIRES/REPORTS windows; acting on
/// these (spawning capsules, applying damage/poise) is the mechanics/collision slice.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct CombatMeta {
    /// Bone the hitbox capsule / effect binds to (e.g. `Weapon_R`). Distinct from
    /// [`EventDef::label`], which stays the free-form tag.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub attach_bone: Option<String>,
    /// Hitbox capsule radius, in metres.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub capsule_radius: Option<f32>,
    /// Damage dealt as `[min, max]`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub damage: Option<[f32; 2]>,
    /// Poise damage dealt.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub poise_damage: Option<f32>,
    /// How the hit connects — selects the damage/reaction class.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hit_type: Option<HitType>,
    /// SFX cue id fired with this event (e.g. `sfx_spear_heavy`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sfx_cue: Option<String>,
    /// VFX cue id fired with this event (e.g. `vfx_ember_arc`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub vfx_cue: Option<String>,
    /// What this window does to whoever it lands on, beyond damage. The motion channels
    /// ride the hitbox window, so a melee proc and a spell snare share ONE surface.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub effects: Vec<EffectSpec>,
    /// Which answers this attack admits. Defaults to all; narrowing it to exactly one is
    /// what makes an attack **perilous**.
    #[serde(default, skip_serializing_if = "ResponseMask::is_all")]
    pub response_mask: ResponseMask,
    /// Shrinks the defender's effective catch window about its centre (1.0 = unchanged).
    ///
    /// **Authored on the ATTACKER, deliberately:** a perfect-parry-only move is hard
    /// because of what the attacker did, not because the defender's parry got worse — the
    /// defender's own `parry_active` window is untouched.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parry_window_scale: Option<f32>,
}

/// How a hit connects — the motion/reaction class of a hitbox window.
///
/// `Sweep` and `Grab` complete the perilous-attack taxonomy: they are the discriminators a
/// **reactive transition** ([`TransitionDef::on_incoming`]) matches against, so a defender
/// can author a response admissible only against a specific incoming attack shape.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HitType {
    Slash,
    Thrust,
    Strike,
    /// Low horizontal arc — answered by jumping over it.
    Sweep,
    /// Unblockable seize — answered by getting out of range.
    Grab,
}

/// One answer a defender may give an incoming attack.
///
/// A normal attack accepts a MENU of these; a **perilous** attack accepts exactly one —
/// which is the whole of the perilous-attack mechanism.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Response {
    Block,
    Parry,
    Dodge,
    Jump,
    Counter,
}

impl Response {
    pub const ALL: [Response; 5] =
        [Response::Block, Response::Parry, Response::Dodge, Response::Jump, Response::Counter];

    fn bit(self) -> u8 {
        match self {
            Response::Block => 1 << 0,
            Response::Parry => 1 << 1,
            Response::Dodge => 1 << 2,
            Response::Jump => 1 << 3,
            Response::Counter => 1 << 4,
        }
    }
}

/// Which answers an attack admits. A compact bitmask at runtime (40 raiders each resolve
/// their own answer against one boss window), but authored and serialized as a readable
/// list of names — `["dodge"]` for a grab, `["jump"]` for a sweep.
///
/// **Default is ALL**, so every pack authored before perilous attacks existed keeps
/// admitting every answer, and the field never appears in a save unless it narrows something.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ResponseMask(u8);

impl ResponseMask {
    /// Every answer permitted — the default, and what an un-annotated attack means.
    pub const ALL: ResponseMask = ResponseMask(0b1_1111);

    pub fn from_slice(items: &[Response]) -> Self {
        ResponseMask(items.iter().fold(0, |m, r| m | r.bit()))
    }

    pub fn allows(self, r: Response) -> bool {
        self.0 & r.bit() != 0
    }

    pub fn is_all(&self) -> bool {
        *self == ResponseMask::ALL
    }

    /// Exactly one answer admitted — the definition of a perilous attack.
    pub fn is_perilous(self) -> bool {
        self.0.count_ones() == 1
    }

    pub fn responses(self) -> impl Iterator<Item = Response> {
        Response::ALL.into_iter().filter(move |r| self.allows(*r))
    }
}

impl Default for ResponseMask {
    fn default() -> Self {
        ResponseMask::ALL
    }
}

impl Serialize for ResponseMask {
    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        self.responses().collect::<Vec<_>>().serialize(s)
    }
}

impl<'de> Deserialize<'de> for ResponseMask {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        Ok(ResponseMask::from_slice(&Vec::<Response>::deserialize(d)?))
    }
}

/// Which of the actor channels an authored effect writes.
///
/// Tracks the four-channel motion algebra plus its two non-motion channels: every control
/// effect writes **exactly one** of these and nothing else, so a melee proc and a spell
/// snare share one authoring surface with no bespoke per-effect code.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EffectChannel {
    /// Who produces the actor's inputs (fear, charm, stun).
    InputSource,
    /// Multiplier on tick advance (slow, haste, stun at 0).
    PlayheadRate,
    /// Multiplier on locomotion velocity (snare, root at 0).
    MoveScale,
    /// Triggers removed from the actor's input set (silence, disarm).
    TriggerMask,
    /// Mitigation modifier (expose, sunder).
    MitigationMod,
    /// Damage over time (poison, disease).
    Dot,
}

/// An effect an authored window applies to whoever it lands on.
///
/// **Authored only**, like the rest of [`CombatMeta`] — the resolver that reads these is a
/// later slice. Deliberately one flat shape rather than a variant per effect: the channel
/// says *what* is written, `magnitude` *how much*, `duration_ticks` *how long*.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct EffectSpec {
    pub channel: EffectChannel,
    /// Channel-dependent: a rate/scale multiplier, a mitigation delta, damage per tick.
    #[serde(default, skip_serializing_if = "is_zero_f32")]
    pub magnitude: f32,
    /// How long it persists. `0` = instantaneous / single application.
    #[serde(default, skip_serializing_if = "is_zero_u32")]
    pub duration_ticks: u32,
    /// Free-form authoring tag (`poison_light`, `snare_web`).
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub label: String,
}

// ─────────────────────────────── runtime (resolved) form ──────────────────────

/// A transition with its target resolved to a state index.
struct Transition {
    to: usize,
    on: Trigger,
    window: Option<TickWindow>,
    priority: i32,
    /// Crossfade duration (ticks) into the target; `0` = hard cut. Resolved at build
    /// from the transition's `blend_ticks` or the machine's `default_blend_ticks`.
    blend_ticks: u32,
}

/// A timeline event (identical to [`EventDef`], kept separate so the wire type can
/// evolve without touching the runtime).
struct Event {
    tick: u32,
    end: Option<u32>,
    kind: EventKind,
    label: String,
}

/// A resolved state: its clip index/duration plus resolved transitions + timeline.
struct State {
    name: String,
    /// Index into the model's clip list, or [`usize::MAX`] if the clip was missing.
    clip: usize,
    /// Clip length in ticks (≥ 1).
    duration: u32,
    looping: bool,
    next: Option<usize>,
    root_motion: bool,
    /// Sorted by priority descending (highest first); ties keep authored order.
    transitions: Vec<Transition>,
    events: Vec<Event>,
}

/// Per-tick control input. `move_`/`run`/`crouch` and the `left`/`right`/`back`
/// direction modifiers are held; `jump`/`attack`/`hit`/`die` are edges the caller sets
/// true only on the frame the control was pressed. Forward is the implied direction when
/// `move_` is held with none of `left`/`right`/`back`.
#[derive(Debug, Default, Clone, Copy)]
pub struct Inputs {
    pub move_: bool,
    pub left: bool,
    pub right: bool,
    pub back: bool,
    pub run: bool,
    pub crouch: bool,
    pub jump: bool,
    pub attack: bool,
    pub hit: bool,
    pub die: bool,
}

/// A clip's identity for name→index resolution at build time. Built by the caller from
/// the loaded model so this module stays decoupled from the `format` types.
#[derive(Debug, Clone, Copy)]
pub struct ClipRef<'a> {
    pub name: &'a str,
    pub duration_ticks: u32,
}

/// A timeline event that fired on a crossed tick (one-shot, or a window's start tick).
#[derive(Debug, Clone)]
pub struct FiredEvent {
    pub kind: EventKind,
    pub label: String,
    pub tick: u32,
}

/// A window event currently open at the settled play-head (for HUD / later hitbox
/// activation).
#[derive(Debug, Clone)]
pub struct ActiveWindow {
    pub kind: EventKind,
    pub label: String,
}

/// A crossfade in progress: the outgoing pose is frozen at `(from_clip, from_tick)`
/// while the current (incoming) state plays from tick 0, and `weight` ramps 0 → 1.
/// The caller samples both clips and blends the LOCAL poses by `weight`
/// (`pose::blend_local_poses`). Absent = no blend, sample the current state only.
#[derive(Debug, Clone, Copy)]
pub struct BlendView {
    /// Clip index of the outgoing state ([`usize::MAX`] if it had no clip → rest pose).
    pub from_clip: usize,
    /// Play-head of the outgoing state, frozen at the moment of transition.
    pub from_tick: u32,
    /// Weight of the INCOMING (current) pose: `0.0` = fully outgoing (the pre-transition
    /// pose), `1.0` = fully incoming. Eased (smoothstep).
    pub weight: f32,
}

/// Internal crossfade state.
struct Blend {
    /// State index of the outgoing pose (its clip is looked up on demand).
    from_state: usize,
    from_tick: u32,
    /// Ticks elapsed since the crossfade began.
    elapsed: u32,
    /// Total crossfade length in ticks (≥ 1; a `0`-tick transition never makes a Blend).
    duration: u32,
}

/// The outcome of advancing the machine.
#[derive(Debug, Default)]
pub struct TickReport {
    /// The state index after advancing.
    pub state: usize,
    /// Did a transition happen while advancing?
    pub transitioned: bool,
    /// Events whose tick was crossed (one-shots + window starts), in order.
    pub fired: Vec<FiredEvent>,
    /// Windows open at the settled tick.
    pub active: Vec<ActiveWindow>,
}

/// The running state machine.
pub struct StateMachine {
    states: Vec<State>,
    any: Vec<Transition>,
    initial: usize,
    current: usize,
    /// Play-head within the current state's clip, in ticks.
    tick: u32,
    /// A non-looping clip has finished and is holding its last frame (so `clip_done`
    /// fires exactly once and the head stops advancing until a transition fires).
    completed: bool,
    /// The active crossfade, if a blended transition is still ramping. A single blend
    /// (not a stack): a new transition mid-blend snapshots the current incoming pose as
    /// the next outgoing and drops the older one.
    blend: Option<Blend>,
    /// Crossfade length used by the `next` clip-done auto-advance (which has no
    /// `TransitionDef` to carry its own). Named transitions resolve their own at build.
    default_blend_ticks: u32,
    /// Fractional-tick accumulator, so a frame's `dt` advances whole ticks.
    accum: f32,
    tick_rate_hz: u32,
    warnings: Vec<String>,
}

impl StateMachine {
    /// Build a runnable machine from an authored graph + the model's clips (referenced
    /// by name). Unresolved clip/state names are collected as [`Self::warnings`] rather
    /// than hard errors (a state with a missing clip holds the rest pose); only a
    /// missing `initial` state is fatal.
    pub fn build(def: &StateMachineDef, clips: &[ClipRef]) -> Result<Self> {
        let state_index: HashMap<&str, usize> = def
            .states
            .iter()
            .enumerate()
            .map(|(i, s)| (s.name.as_str(), i))
            .collect();
        let clip_index: HashMap<&str, (usize, u32)> = clips
            .iter()
            .enumerate()
            .map(|(i, c)| (c.name, (i, c.duration_ticks)))
            .collect();

        let mut warnings = Vec::new();

        let default_blend = def.default_blend_ticks;
        let resolve_transitions =
            |defs: &[TransitionDef], from: &str, warnings: &mut Vec<String>| -> Vec<Transition> {
                let mut out: Vec<Transition> = Vec::new();
                for t in defs {
                    match state_index.get(t.to.as_str()) {
                        Some(&to) => out.push(Transition {
                            to,
                            on: t.on,
                            window: t.window,
                            priority: t.priority,
                            // Per-transition override, else the machine default.
                            blend_ticks: t.blend_ticks.unwrap_or(default_blend),
                        }),
                        None => warnings.push(format!(
                            "transition from '{from}' targets unknown state '{}'",
                            t.to
                        )),
                    }
                }
                // Highest priority first; stable so authored order breaks ties.
                out.sort_by_key(|t| std::cmp::Reverse(t.priority));
                out
            };

        let mut states = Vec::with_capacity(def.states.len());
        for s in &def.states {
            let (clip, duration) = match clip_index.get(s.clip.as_str()) {
                Some(&(i, d)) => (i, d.max(1)),
                None => {
                    warnings.push(format!(
                        "state '{}' references unknown clip '{}' — will hold rest pose",
                        s.name, s.clip
                    ));
                    (usize::MAX, 1)
                }
            };
            let next = match s.next.as_deref() {
                Some(n) => match state_index.get(n) {
                    Some(&i) => Some(i),
                    None => {
                        warnings.push(format!(
                            "state '{}' next targets unknown state '{n}'",
                            s.name
                        ));
                        None
                    }
                },
                None => None,
            };
            let events = s
                .events
                .iter()
                .map(|e| Event {
                    tick: e.tick,
                    end: e.end,
                    kind: e.kind,
                    label: e.label.clone(),
                })
                .collect();
            states.push(State {
                name: s.name.clone(),
                clip,
                duration,
                looping: s.looping,
                next,
                root_motion: s.root_motion,
                transitions: resolve_transitions(&s.transitions, &s.name, &mut warnings),
                events,
            });
        }

        let any = resolve_transitions(&def.any, "<any>", &mut warnings);
        let initial = *state_index
            .get(def.initial.as_str())
            .with_context(|| format!("initial state '{}' not in the graph", def.initial))?;

        Ok(Self {
            states,
            any,
            initial,
            current: initial,
            tick: 0,
            completed: false,
            blend: None,
            default_blend_ticks: def.default_blend_ticks,
            accum: 0.0,
            tick_rate_hz: def.tick_rate_hz.max(1),
            warnings,
        })
    }

    // ── queries (for the render/HUD path) ──

    pub fn current_state_name(&self) -> &str {
        &self.states[self.current].name
    }
    /// Clip index into the model's clip list ([`usize::MAX`] if the state's clip was
    /// missing — the caller falls back to the rest pose).
    pub fn current_clip(&self) -> usize {
        self.states[self.current].clip
    }
    pub fn current_tick(&self) -> u32 {
        self.tick
    }
    pub fn current_duration(&self) -> u32 {
        self.states[self.current].duration
    }
    /// Whether the current state uses root-motion translation (in-place otherwise).
    pub fn current_root_motion(&self) -> bool {
        self.states[self.current].root_motion
    }
    pub fn warnings(&self) -> &[String] {
        &self.warnings
    }

    /// The active crossfade, if a blended transition is still ramping. The caller
    /// samples the outgoing clip (`from_clip` at `from_tick`) and the current clip
    /// ([`Self::current_clip`] at [`Self::current_tick`]) and blends their LOCAL poses
    /// by [`BlendView::weight`]. `None` = no blend, sample the current state only.
    pub fn blend(&self) -> Option<BlendView> {
        self.blend.as_ref().map(|b| BlendView {
            from_clip: self.states[b.from_state].clip,
            from_tick: b.from_tick,
            // elapsed ∈ [0, duration] here (cleared past duration in `tick`).
            weight: smoothstep(b.elapsed as f32 / b.duration as f32),
        })
    }

    /// Reset to the initial state (play-head at 0).
    pub fn reset(&mut self) {
        self.current = self.initial;
        self.tick = 0;
        self.completed = false;
        self.blend = None;
        self.accum = 0.0;
    }

    /// Force an immediate transition to the named state — a "game commands this reaction"
    /// escape hatch, for when the *driver* (not the input graph) picks the target: e.g. a
    /// randomly-selected hit reaction from a group of Dame clips. Crossfades with the
    /// machine's default blend. Returns `false` if the name is unknown. Keeps the state
    /// machine itself deterministic — the randomness lives in the caller.
    pub fn force_state_by_name(&mut self, name: &str) -> bool {
        let Some(target) = self.states.iter().position(|s| s.name == name) else {
            return false;
        };
        let mut fired = Vec::new();
        self.enter(target, self.default_blend_ticks, &mut fired);
        true
    }

    // ── stepping ──

    /// Advance one fixed tick (the atomic combat-clock step). Returns what fired.
    pub fn tick(&mut self, inputs: &Inputs) -> TickReport {
        let mut fired = Vec::new();

        // 0. Elapse an in-flight crossfade (the incoming clip plays as the blend
        //    ramps). Clear once past its duration — the settled weight hits 1.0 on the
        //    duration-th tick, then the next tick drops to the plain incoming pose.
        if let Some(b) = &mut self.blend {
            b.elapsed += 1;
            if b.elapsed > b.duration {
                self.blend = None;
            }
        }

        // 1. Advance the play-head; note whether the clip completed this tick.
        let dur = self.states[self.current].duration;
        let looping = self.states[self.current].looping;
        let mut clip_done = false;
        if !self.completed {
            self.tick += 1;
            if self.tick >= dur {
                clip_done = true;
                if looping {
                    self.tick = 0;
                } else {
                    self.tick = dur.saturating_sub(1);
                    self.completed = true;
                }
            }
        }

        // 2. Fire the current state's events at the settled tick.
        self.fire_events_at(self.current, self.tick, &mut fired);

        // 3. Evaluate transitions (any-state first, then per-state, then `next`).
        let mut transitioned = false;
        if let Some((target, blend_ticks)) = self.pick_transition(inputs, clip_done) {
            self.enter(target, blend_ticks, &mut fired);
            transitioned = true;
        }

        TickReport {
            state: self.current,
            transitioned,
            fired,
            active: self.active_windows(),
        }
    }

    /// Accumulate a frame's `dt` and run the whole ticks it buys (capped, so a long
    /// frame can't spiral). Merges the per-tick reports.
    pub fn advance(&mut self, dt_secs: f32, inputs: &Inputs) -> TickReport {
        self.accum += dt_secs * self.tick_rate_hz as f32;
        let mut merged = TickReport {
            state: self.current,
            ..Default::default()
        };
        let mut budget = 8;
        while self.accum >= 1.0 && budget > 0 {
            self.accum -= 1.0;
            budget -= 1;
            let r = self.tick(inputs);
            merged.transitioned |= r.transitioned;
            merged.fired.extend(r.fired);
        }
        merged.state = self.current;
        merged.active = self.active_windows();
        merged
    }

    // ── internals ──

    fn fire_events_at(&self, state: usize, t: u32, out: &mut Vec<FiredEvent>) {
        for e in &self.states[state].events {
            if e.tick == t {
                out.push(FiredEvent {
                    kind: e.kind,
                    label: e.label.clone(),
                    tick: t,
                });
            }
        }
    }

    /// Windows open at the current play-head of the current state.
    fn active_windows(&self) -> Vec<ActiveWindow> {
        let mut out = Vec::new();
        for e in &self.states[self.current].events {
            if let Some(end) = e.end {
                if self.tick >= e.tick && self.tick <= end {
                    out.push(ActiveWindow {
                        kind: e.kind,
                        label: e.label.clone(),
                    });
                }
            }
        }
        out
    }

    /// Choose the transition to take this tick as `(target, blend_ticks)`, or `None`.
    /// Any-state edges win over per-state ones; `next` is the lowest-precedence
    /// fallback on clip completion (blended by the machine default).
    fn pick_transition(&self, inputs: &Inputs, clip_done: bool) -> Option<(usize, u32)> {
        for t in &self.any {
            if satisfied(t.on, inputs, clip_done) && t.window.is_none_or(|w| w.contains(self.tick)) {
                return Some((t.to, t.blend_ticks));
            }
        }
        for t in &self.states[self.current].transitions {
            if satisfied(t.on, inputs, clip_done) && t.window.is_none_or(|w| w.contains(self.tick)) {
                return Some((t.to, t.blend_ticks));
            }
        }
        if clip_done {
            if let Some(n) = self.states[self.current].next {
                return Some((n, self.default_blend_ticks));
            }
        }
        None
    }

    /// Enter a state: swap, reset the play-head, fire the entered state's tick-0
    /// events. When `blend_ticks > 0`, start a crossfade from the outgoing pose
    /// (snapshotted at the state/tick we're leaving) instead of hard-cutting.
    fn enter(&mut self, target: usize, blend_ticks: u32, fired: &mut Vec<FiredEvent>) {
        self.blend = (blend_ticks > 0).then_some(Blend {
            from_state: self.current,
            from_tick: self.tick,
            elapsed: 0,
            duration: blend_ticks,
        });
        self.current = target;
        self.tick = 0;
        self.completed = false;
        self.fire_events_at(target, 0, fired);
    }
}

fn satisfied(on: Trigger, inputs: &Inputs, clip_done: bool) -> bool {
    let dir_none = !inputs.left && !inputs.right && !inputs.back;
    match on {
        Trigger::Move => inputs.move_,
        Trigger::MoveStop => !inputs.move_,
        Trigger::MoveForward => inputs.move_ && dir_none,
        Trigger::MoveLeft => inputs.move_ && inputs.left,
        Trigger::MoveRight => inputs.move_ && inputs.right,
        Trigger::MoveBack => inputs.move_ && inputs.back,
        Trigger::Run => inputs.move_ && inputs.run,
        Trigger::RunStop => !inputs.run,
        Trigger::Crouch => inputs.crouch,
        Trigger::CrouchStop => !inputs.crouch,
        // Overload the run modifier to "be still" while crouched (the decision matrix).
        Trigger::CrouchStill => inputs.crouch && inputs.run,
        Trigger::Jump => inputs.jump,
        Trigger::Attack => inputs.attack,
        Trigger::Hit => inputs.hit,
        Trigger::Die => inputs.die,
        Trigger::ClipDone => clip_done,
    }
}

/// Smoothstep easing on a normalized `t` (clamped to `[0, 1]`) — an ease-in-out ramp
/// (`3t² − 2t³`) that starts and ends with zero slope, so a crossfade eases into and
/// out of the transition rather than blending linearly.
fn smoothstep(t: f32) -> f32 {
    let t = t.clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

/// Load a `flicker.pack` JSON and return just its state-machine definition — the common
/// read-only path (packeditor/paperdoll only need the graph).
pub fn load_pack(path: &Path) -> Result<StateMachineDef> {
    Ok(read_pack(path)?.state_machine)
}

/// Read a whole `flicker.pack` JSON — the `format`/`version`/`_note` header plus the
/// state machine. The editor holds the full [`PackFile`] so a save round-trips the
/// header (notably the hand-authored `_note`), which `load_pack` drops.
pub fn read_pack(path: &Path) -> Result<PackFile> {
    // Gz-transparent read (flicker-core::compression) — package content is gz
    // at rest; loose dev files read the same way.
    let text = flicker_core::compression::read_text(path)
        .with_context(|| format!("reading pack {}", path.display()))?;
    serde_json::from_str(&text).with_context(|| format!("parsing pack {}", path.display()))
}

/// Write a `flicker.pack` JSON (pretty-printed, trailing newline) — the inverse of
/// [`read_pack`] and the editor's save path. Emits the gz-at-rest form via the
/// shared seam: a logical `<name>.pack.json` path lands as `<name>.pack.json.gz`
/// (package content at rest is always gz), and [`read_pack`] reads it back by
/// the same logical path.
pub fn write_pack(path: &Path, pack: &PackFile) -> Result<()> {
    let mut text = serde_json::to_string_pretty(pack)
        .with_context(|| format!("serializing pack {}", path.display()))?;
    text.push('\n');
    flicker_core::compression::write_text(path, &text)
        .map(|_| ())
        .with_context(|| format!("writing pack {}", path.display()))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every real pack in the content tree.
    fn real_packs() -> Vec<std::path::PathBuf> {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../../../Alpha/content/package/characters");
        let mut out = Vec::new();
        let Ok(dirs) = std::fs::read_dir(&root) else {
            return out;
        };
        for d in dirs.flatten() {
            let Ok(files) = std::fs::read_dir(d.path()) else { continue };
            for f in files.flatten() {
                let p = f.path();
                if p.to_str()
                    .is_some_and(|s| s.ends_with(".pack.json") || s.ends_with(".pack.json.gz"))
                {
                    out.push(p);
                }
            }
        }
        out
    }

    /// **THE ADDITIVE-SERDE GUARD.** Every combat field added for the authoring contract is
    /// `#[serde(default, skip_serializing_if = …)]`, so a pack authored before they existed
    /// must re-serialize without acquiring a single one of them. Without this, the first
    /// save in the editor would silently rewrite every hand-authored pack in the repo.
    #[test]
    fn existing_packs_do_not_acquire_the_new_combat_fields_on_save() {
        let packs = real_packs();
        if packs.is_empty() {
            return; // content tree absent in this checkout
        }
        for path in packs {
            let pack = read_pack(&path).expect("real pack parses");
            let out = serde_json::to_string_pretty(&pack).expect("re-serializes");
            for field in [
                "stamina_cost",
                "blocking",
                "guard_angle",
                "effects",
                "response_mask",
                "parry_window_scale",
                "on_incoming",
            ] {
                assert!(
                    !out.contains(field),
                    "{}: saving acquired `{field}` — a default leaked into the file",
                    path.display()
                );
            }
        }
    }

    /// Load → save → load must be stable: the second parse equals the first. Proves the
    /// editor's Save is idempotent rather than drifting a pack a little on every write.
    #[test]
    fn real_packs_round_trip_stably() {
        for path in real_packs() {
            let a = read_pack(&path).expect("parses");
            let text = serde_json::to_string(&a).expect("serializes");
            let b: PackFile = serde_json::from_str(&text).expect("re-parses");
            let a_sm = &a.state_machine;
            let b_sm = &b.state_machine;
            assert_eq!(a_sm.initial, b_sm.initial);
            assert_eq!(a_sm.states.len(), b_sm.states.len());
            assert_eq!(a.note, b.note, "{}: _note lost", path.display());
            for (x, y) in a_sm.states.iter().zip(&b_sm.states) {
                assert_eq!(x.name, y.name);
                assert_eq!(x.clip, y.clip);
                assert_eq!(x.events.len(), y.events.len(), "{}: events lost", x.name);
                assert_eq!(x.transitions.len(), y.transitions.len());
            }
        }
    }

    /// The mask defaults to "every answer", so an un-annotated attack is unchanged; exactly
    /// one answer is the definition of a perilous attack.
    #[test]
    fn response_mask_defaults_to_all_and_detects_perilous() {
        let all = ResponseMask::default();
        assert!(all.is_all());
        assert!(Response::ALL.iter().all(|r| all.allows(*r)));
        assert!(!all.is_perilous(), "a normal attack admits a menu");

        let sweep = ResponseMask::from_slice(&[Response::Jump]);
        assert!(sweep.is_perilous());
        assert!(sweep.allows(Response::Jump) && !sweep.allows(Response::Block));
        // Round-trips through the readable name list.
        let json = serde_json::to_string(&sweep).unwrap();
        assert_eq!(json, r#"["jump"]"#);
        assert_eq!(serde_json::from_str::<ResponseMask>(&json).unwrap(), sweep);
    }

    /// A reactive transition and the two new hit shapes must survive a round trip by name.
    ///
    /// NOTE the trigger here is `jump` (leap the sweep) because that is the only one of the
    /// five [`Response`] answers with a corresponding input [`Trigger`] today — there is no
    /// `parry`/`block`/`dodge`/`counter` input, so the Mikiri (`on_incoming: thrust`) has no
    /// input to gate on yet. Tracked as the contract's one unenumerated gap.
    #[test]
    fn reactive_transition_round_trips_by_name() {
        let json = r#"{"to":"LeapSweep","on":"jump","on_incoming":"sweep"}"#;
        let t: TransitionDef = serde_json::from_str(json).unwrap();
        assert_eq!(t.on_incoming, Some(HitType::Sweep));
        assert_eq!(serde_json::to_string(&t).unwrap(), json, "no default noise added");
        // The taxonomy the mask discriminates on.
        for (h, name) in [(HitType::Sweep, "\"sweep\""), (HitType::Grab, "\"grab\"")] {
            assert_eq!(serde_json::to_string(&h).unwrap(), name);
        }
    }

    /// Small synthetic graph exercising locomotion, a clip-done chain, a cancel window,
    /// timeline events, and an any-state hit edge.
    const GRAPH: &str = r#"{
      "format": "flicker.pack", "version": 1,
      "state_machine": {
        "initial": "Idle",
        "any": [ { "to": "Dame", "on": "hit" } ],
        "states": [
          { "name": "Idle", "clip": "idle",
            "transitions": [
              { "to": "Attack", "on": "attack", "priority": 5 },
              { "to": "Jump", "on": "jump", "priority": 5 },
              { "to": "Walk", "on": "move", "priority": 1 } ] },
          { "name": "Walk", "clip": "walk",
            "events": [ { "tick": 3, "kind": "footstep", "label": "L" } ],
            "transitions": [ { "to": "Idle", "on": "move_stop" } ] },
          { "name": "Jump", "clip": "jump", "looping": false, "next": "Idle" },
          { "name": "Attack", "clip": "attack", "looping": false,
            "events": [ { "tick": 2, "end": 4, "kind": "hitbox_active", "label": "Weapon_R" } ],
            "transitions": [
              { "to": "Attack", "on": "attack", "window": { "start": 6, "end": 8 }, "priority": 10 },
              { "to": "Idle", "on": "clip_done", "priority": 1 } ] },
          { "name": "Dame", "clip": "dame", "looping": false, "next": "Idle" }
        ]
      }
    }"#;

    fn clips() -> Vec<ClipRef<'static>> {
        vec![
            ClipRef { name: "idle", duration_ticks: 200 },
            ClipRef { name: "walk", duration_ticks: 10 },
            ClipRef { name: "jump", duration_ticks: 5 },
            ClipRef { name: "attack", duration_ticks: 10 },
            ClipRef { name: "dame", duration_ticks: 8 },
        ]
    }

    fn build() -> StateMachine {
        let def: PackFile = serde_json::from_str(GRAPH).unwrap();
        let sm = StateMachine::build(&def.state_machine, &clips()).unwrap();
        assert!(sm.warnings().is_empty(), "unexpected warnings: {:?}", sm.warnings());
        sm
    }

    /// Step `n` whole ticks with fixed inputs and return the last report.
    fn run(sm: &mut StateMachine, n: u32, inputs: &Inputs) -> TickReport {
        let mut last = TickReport::default();
        for _ in 0..n {
            last = sm.tick(inputs);
        }
        last
    }

    #[test]
    fn pack_round_trips_and_preserves_note() {
        // A pack with a `_note` header, an any-state edge, and both window + one-shot events.
        let src = r#"{
          "format": "flicker.pack", "version": 1,
          "_note": "hand-authored provenance — must survive a save",
          "state_machine": {
            "initial": "Idle",
            "any": [ { "to": "Dame", "on": "hit" } ],
            "states": [
              { "name": "Idle", "clip": "idle",
                "transitions": [ { "to": "Walk", "on": "move" } ] },
              { "name": "Walk", "clip": "walk", "looping": false, "next": "Idle",
                "events": [
                  { "tick": 3, "kind": "footstep", "label": "L" },
                  { "tick": 2, "end": 4, "kind": "hitbox_active", "label": "Weapon_R" } ] }
            ]
          }
        }"#;
        let pack: PackFile = serde_json::from_str(src).unwrap();
        assert_eq!(pack.note, "hand-authored provenance — must survive a save");

        let out1 = serde_json::to_string_pretty(&pack).unwrap();
        assert!(out1.contains("_note"), "note key must be written back");
        assert!(out1.contains("\"any\""), "any-state edges must survive");
        assert!(out1.contains("hitbox_active"), "window events must survive");

        // Idempotent once normalized: re-parse + re-serialize is byte-identical.
        let reparsed: PackFile = serde_json::from_str(&out1).unwrap();
        let out2 = serde_json::to_string_pretty(&reparsed).unwrap();
        assert_eq!(out1, out2, "save must round-trip stably");

        // Skips keep the output clean: no `null` Options, no default scalars, no `[]`.
        assert!(!out1.contains("null"), "None Options are omitted, not written as null");
        assert!(!out1.contains("\"priority\""), "default priority 0 is omitted");
        assert!(out1.contains("\"looping\": false"), "non-default looping is kept");
    }

    #[test]
    fn combat_metadata_is_optional_and_round_trips() {
        // A legacy pack (no `combat` key anywhere) still deserializes untouched.
        let legacy: PackFile = serde_json::from_str(GRAPH).unwrap();
        let walk = legacy.state_machine.states.iter().find(|s| s.name == "Walk").unwrap();
        assert!(walk.events[0].combat.is_none(), "legacy events carry no combat metadata");

        // An authored hitbox window with the full TAE-inspector payload round-trips.
        let src = r#"{
          "format": "flicker.pack", "version": 1,
          "state_machine": {
            "initial": "Attack",
            "states": [
              { "name": "Attack", "clip": "attack", "looping": false,
                "events": [ {
                  "tick": 12, "end": 17, "kind": "hitbox_active", "label": "Weapon_R",
                  "combat": {
                    "attach_bone": "R_Weapon_Tip", "capsule_radius": 0.35,
                    "damage": [42.0, 58.0], "poise_damage": 28.0, "hit_type": "thrust",
                    "sfx_cue": "sfx_spear_heavy", "vfx_cue": "vfx_ember_arc" } } ] }
            ]
          }
        }"#;
        let pack: PackFile = serde_json::from_str(src).unwrap();
        let combat = pack.state_machine.states[0].events[0].combat.clone().unwrap();
        assert_eq!(combat.attach_bone.as_deref(), Some("R_Weapon_Tip"));
        assert_eq!(combat.hit_type, Some(HitType::Thrust));
        assert_eq!(combat.damage, Some([42.0, 58.0]));

        let out = serde_json::to_string_pretty(&pack).unwrap();
        let reparsed: PackFile = serde_json::from_str(&out).unwrap();
        assert_eq!(reparsed.state_machine.states[0].events[0].combat, Some(combat));
    }

    #[test]
    fn starts_in_initial() {
        let sm = build();
        assert_eq!(sm.current_state_name(), "Idle");
        assert_eq!(sm.current_tick(), 0);
    }

    #[test]
    fn force_state_by_name_enters_target_and_ignores_unknown() {
        let mut sm = build();
        assert_eq!(sm.current_state_name(), "Idle");
        // A driver commanding a reaction (e.g. a random hit) enters it immediately.
        assert!(sm.force_state_by_name("Attack"));
        assert_eq!(sm.current_state_name(), "Attack");
        assert_eq!(sm.current_tick(), 0);
        // An unknown name is a no-op.
        assert!(!sm.force_state_by_name("Nope"));
        assert_eq!(sm.current_state_name(), "Attack");
    }

    #[test]
    fn move_and_stop_locomotion() {
        let mut sm = build();
        let moving = Inputs { move_: true, ..Default::default() };
        sm.tick(&moving);
        assert_eq!(sm.current_state_name(), "Walk");
        // Releasing move returns to Idle.
        let idle = Inputs::default();
        sm.tick(&idle);
        assert_eq!(sm.current_state_name(), "Idle");
    }

    #[test]
    fn clip_done_auto_advances_via_next() {
        let mut sm = build();
        // Jump on the first tick, then hold until the 5-tick clip completes.
        sm.tick(&Inputs { jump: true, ..Default::default() });
        assert_eq!(sm.current_state_name(), "Jump");
        run(&mut sm, 5, &Inputs::default());
        assert_eq!(sm.current_state_name(), "Idle", "Jump should fall through to Idle on clip_done");
    }

    #[test]
    fn timeline_event_fires_once() {
        let mut sm = build();
        sm.tick(&Inputs { move_: true, ..Default::default() }); // → Walk, tick 0
        let mut footsteps = 0;
        // Walk's footstep is authored at tick 3.
        for _ in 0..10 {
            let r = sm.tick(&Inputs { move_: true, ..Default::default() });
            footsteps += r.fired.iter().filter(|e| e.kind == EventKind::Footstep).count();
        }
        assert_eq!(footsteps, 1, "footstep at tick 3 should fire exactly once per loop");
    }

    #[test]
    fn hitbox_window_reports_active() {
        let mut sm = build();
        sm.tick(&Inputs { attack: true, ..Default::default() }); // → Attack, tick 0
        // Window is ticks [2,4]; advance to tick 3 and check it's reported active.
        let r = run(&mut sm, 3, &Inputs::default());
        assert_eq!(sm.current_tick(), 3);
        assert!(
            r.active.iter().any(|w| w.kind == EventKind::HitboxActive && w.label == "Weapon_R"),
            "hitbox window should be active at tick 3, got {:?}", r.active
        );
    }

    #[test]
    fn cancel_window_gates_the_combo() {
        let mut sm = build();
        sm.tick(&Inputs { attack: true, ..Default::default() }); // → Attack tick 0
        // Pressing attack at tick 1 (outside the [6,8] cancel window) must NOT combo.
        let r = sm.tick(&Inputs { attack: true, ..Default::default() });
        assert!(!r.transitioned);
        assert_eq!(sm.current_state_name(), "Attack");
        // Advance into the window and press attack → re-enters Attack (combo).
        run(&mut sm, 5, &Inputs::default()); // now at tick 6, inside the [6,8] window
        assert_eq!(sm.current_tick(), 6);
        let r = sm.tick(&Inputs { attack: true, ..Default::default() });
        assert!(r.transitioned);
        assert_eq!(sm.current_state_name(), "Attack");
        assert_eq!(sm.current_tick(), 0, "combo hard-cuts back to the start of Attack");
    }

    #[test]
    fn any_state_hit_interrupts() {
        let mut sm = build();
        sm.tick(&Inputs { move_: true, ..Default::default() }); // → Walk
        let r = sm.tick(&Inputs { hit: true, move_: true, ..Default::default() });
        assert!(r.transitioned);
        assert_eq!(sm.current_state_name(), "Dame", "a hit interrupts from any state");
    }

    #[test]
    fn missing_clip_is_a_warning_not_a_panic() {
        let def: PackFile = serde_json::from_str(
            r#"{ "state_machine": { "initial": "A",
                 "states": [ { "name": "A", "clip": "nope" } ] } }"#,
        )
        .unwrap();
        let sm = StateMachine::build(&def.state_machine, &clips()).unwrap();
        assert_eq!(sm.current_clip(), usize::MAX);
        assert!(!sm.warnings().is_empty());
    }

    #[test]
    fn directional_move_routes_by_held_direction() {
        let graph = r#"{ "state_machine": { "initial": "Idle", "states": [
            { "name": "Idle", "clip": "idle", "transitions": [
                { "to": "Walk_L", "on": "move_left", "priority": 2 },
                { "to": "Walk_R", "on": "move_right", "priority": 2 },
                { "to": "Walk", "on": "move", "priority": 1 } ] },
            { "name": "Walk", "clip": "walk", "transitions": [
                { "to": "Walk_L", "on": "move_left", "priority": 2 },
                { "to": "Idle", "on": "move_stop", "priority": 1 } ] },
            { "name": "Walk_L", "clip": "walk", "transitions": [
                { "to": "Walk", "on": "move_forward", "priority": 2 },
                { "to": "Idle", "on": "move_stop", "priority": 1 } ] },
            { "name": "Walk_R", "clip": "walk" }
        ] } }"#;
        let def: PackFile = serde_json::from_str(graph).unwrap();
        let mut sm = StateMachine::build(&def.state_machine, &clips()).unwrap();
        assert!(sm.warnings().is_empty(), "{:?}", sm.warnings());
        // move + left → the left strafe (a held direction beats plain `move`).
        sm.tick(&Inputs { move_: true, left: true, ..Default::default() });
        assert_eq!(sm.current_state_name(), "Walk_L");
        // release the direction while still moving → forward walk (`move_forward`).
        sm.tick(&Inputs { move_: true, ..Default::default() });
        assert_eq!(sm.current_state_name(), "Walk");
        // plain forward move from Idle picks the forward walk, never a strafe.
        let mut sm2 = StateMachine::build(&def.state_machine, &clips()).unwrap();
        sm2.tick(&Inputs { move_: true, ..Default::default() });
        assert_eq!(sm2.current_state_name(), "Walk");
    }

    // ── crossfade blending ──

    /// A 4-tick machine default, with `Attack` opting out (`blend_ticks: 0`) and
    /// falling through to `Idle` via `next` on completion (which uses the default).
    const BLEND_GRAPH: &str = r#"{
      "state_machine": {
        "initial": "Idle",
        "default_blend_ticks": 4,
        "states": [
          { "name": "Idle", "clip": "idle", "transitions": [
              { "to": "Walk", "on": "move" },
              { "to": "Attack", "on": "attack", "blend_ticks": 0, "priority": 5 } ] },
          { "name": "Walk", "clip": "walk", "transitions": [ { "to": "Idle", "on": "move_stop" } ] },
          { "name": "Attack", "clip": "attack", "looping": false, "next": "Idle" }
        ]
      }
    }"#;

    fn build_blended() -> StateMachine {
        let def: PackFile = serde_json::from_str(BLEND_GRAPH).unwrap();
        let sm = StateMachine::build(&def.state_machine, &clips()).unwrap();
        assert!(sm.warnings().is_empty(), "unexpected warnings: {:?}", sm.warnings());
        sm
    }

    #[test]
    fn blend_starts_ramps_and_clears() {
        let mut sm = build_blended();
        // Idle → Walk uses the machine default (4-tick) crossfade.
        sm.tick(&Inputs { move_: true, ..Default::default() });
        assert_eq!(sm.current_state_name(), "Walk");
        let b0 = sm.blend().expect("blend active right after a blended transition");
        assert_eq!(b0.from_clip, 0, "outgoing pose is Idle's clip (index 0)");
        assert!(b0.weight.abs() < 1e-6, "weight starts at 0.0 = fully outgoing");

        // Sample the weight after the transition tick and each following tick.
        let mut samples = vec![sm.blend().map(|b| b.weight)];
        for _ in 0..5 {
            sm.tick(&Inputs { move_: true, ..Default::default() });
            samples.push(sm.blend().map(|b| b.weight));
        }
        let weights: Vec<f32> = samples.iter().filter_map(|w| *w).collect();
        assert!(weights[0].abs() < 1e-6);
        assert!(
            (weights.last().unwrap() - 1.0).abs() < 1e-6,
            "weight reaches 1.0 at the end of the crossfade: {weights:?}"
        );
        for pair in weights.windows(2) {
            assert!(pair[1] >= pair[0] - 1e-6, "weight is monotonic non-decreasing: {weights:?}");
        }
        assert!(samples.last().unwrap().is_none(), "blend clears once past its duration");
    }

    #[test]
    fn zero_blend_ticks_is_a_hard_cut() {
        let mut sm = build_blended();
        // Attack overrides the 4-tick default with blend_ticks: 0.
        sm.tick(&Inputs { attack: true, ..Default::default() });
        assert_eq!(sm.current_state_name(), "Attack");
        assert!(sm.blend().is_none(), "a 0-tick transition hard-cuts — no crossfade");
    }

    #[test]
    fn next_autoadvance_blends_by_default() {
        let mut sm = build_blended();
        sm.tick(&Inputs { attack: true, ..Default::default() }); // → Attack (hard cut)
        assert!(sm.blend().is_none());
        // Attack is 10 ticks; run it to completion → `next` Idle with the 4-tick default.
        run(&mut sm, 10, &Inputs::default());
        assert_eq!(sm.current_state_name(), "Idle");
        let b = sm.blend().expect("clip-done auto-advance crossfades by the machine default");
        assert_eq!(b.from_clip, 3, "outgoing pose is Attack's clip (index 3)");
        assert!(b.weight.abs() < 1e-6, "fresh blend starts at weight 0.0");
    }
}

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
use serde::Deserialize;

// ─────────────────────────────── authored (wire) types ────────────────────────

/// A content pack file. For this slice we only read `state_machine`; later slices add
/// the rig reference, mesh/skin variants, weapon packs, and ability defs alongside it.
#[derive(Debug, Deserialize)]
pub struct PackFile {
    #[serde(default)]
    pub format: String,
    #[serde(default)]
    pub version: u32,
    pub state_machine: StateMachineDef,
}

/// The authored state graph.
#[derive(Debug, Deserialize)]
pub struct StateMachineDef {
    /// Name of the state to start in.
    pub initial: String,
    /// Transitions evaluated from **every** state, before the per-state ones — the
    /// "from any state" edges (hit reaction, death). Highest-precedence.
    #[serde(default)]
    pub any: Vec<TransitionDef>,
    pub states: Vec<StateDef>,
}

#[derive(Debug, Deserialize)]
pub struct StateDef {
    pub name: String,
    /// Clip name to play in this state (resolved to an index at build).
    pub clip: String,
    /// Does the clip loop? Locomotion/idle loop; attacks/reactions play once.
    #[serde(default = "default_true")]
    pub looping: bool,
    /// Auto-advance to this state when a non-looping clip completes (e.g.
    /// `Jump_Start` → `Jump_Loop`). A convenience alternative to a `clip_done`
    /// transition; explicit transitions of higher priority still win.
    #[serde(default)]
    pub next: Option<String>,
    /// Locomotion authority: `true` = the clip's root track drives translation
    /// (opt-in, for climbs/specials); `false` (default) = in-place, capsule-driven.
    /// Recorded now; the capsule mover lands in a later slice.
    #[serde(default)]
    pub root_motion: bool,
    #[serde(default)]
    pub transitions: Vec<TransitionDef>,
    /// The TAE event timeline for this state's clip.
    #[serde(default)]
    pub events: Vec<EventDef>,
}

fn default_true() -> bool {
    true
}

#[derive(Debug, Clone, Deserialize)]
pub struct TransitionDef {
    /// Target state name.
    pub to: String,
    /// Input/condition that gates this transition.
    pub on: Trigger,
    /// Only permit the transition while the play-head is inside this tick window — a
    /// cancel/combo window. Absent = allowed any time.
    #[serde(default)]
    pub window: Option<TickWindow>,
    /// Higher priority is evaluated first (default 0). Ties keep authored order.
    #[serde(default)]
    pub priority: i32,
}

/// An inclusive `[start, end]` tick interval.
#[derive(Debug, Clone, Copy, Deserialize)]
pub struct TickWindow {
    pub start: u32,
    pub end: u32,
}

impl TickWindow {
    fn contains(&self, t: u32) -> bool {
        t >= self.start && t <= self.end
    }
}

/// What can gate a transition. Movement modifiers (`move`/`run`/`crouch`) are **held**
/// states; `jump`/`attack`/`hit`/`die` are **edges** (true only the tick pressed);
/// `clip_done` fires the tick a clip completes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Trigger {
    Move,
    MoveStop,
    Run,
    RunStop,
    Crouch,
    CrouchStop,
    Jump,
    Attack,
    Hit,
    Die,
    ClipDone,
}

#[derive(Debug, Clone, Deserialize)]
pub struct EventDef {
    /// Tick this event fires at (a one-shot) or the start of its window.
    pub tick: u32,
    /// Inclusive end tick for a **window** event (hitbox-active, i-frames, cancel);
    /// absent = a one-shot at `tick`.
    #[serde(default)]
    pub end: Option<u32>,
    pub kind: EventKind,
    /// Free-form tag — a bone name for a hitbox capsule, an sfx id, a foot label, …
    #[serde(default)]
    pub label: String,
}

/// The souls-like combat-metadata vocabulary carried on the timeline. Fired/reported
/// by this slice; consumed (capsules, invulnerability, …) by later slices.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EventKind {
    Footstep,
    /// A hitbox capsule (bound to `label` bone) is live for this window.
    HitboxActive,
    /// Invulnerability (dodge/roll i-frames) for this window.
    Iframe,
    /// Cancel/combo window — the feel of souls combos.
    CancelWindow,
    /// Parry / deflect window.
    Parry,
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
            EventKind::Sfx => "sfx",
            EventKind::Equip => "equip",
            EventKind::WeaponTrail => "trail",
        }
    }
}

// ─────────────────────────────── runtime (resolved) form ──────────────────────

/// A transition with its target resolved to a state index.
struct Transition {
    to: usize,
    on: Trigger,
    window: Option<TickWindow>,
    priority: i32,
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

/// Per-tick control input. `move_`/`run`/`crouch` are held; `jump`/`attack`/`hit`/`die`
/// are edges the caller sets true only on the frame the control was pressed.
#[derive(Debug, Default, Clone, Copy)]
pub struct Inputs {
    pub move_: bool,
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
            accum: 0.0,
            tick_rate_hz: 60,
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

    /// Reset to the initial state (play-head at 0).
    pub fn reset(&mut self) {
        self.current = self.initial;
        self.tick = 0;
        self.completed = false;
        self.accum = 0.0;
    }

    // ── stepping ──

    /// Advance one fixed tick (the atomic combat-clock step). Returns what fired.
    pub fn tick(&mut self, inputs: &Inputs) -> TickReport {
        let mut fired = Vec::new();

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
        if let Some(target) = self.pick_transition(inputs, clip_done) {
            self.enter(target, &mut fired);
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

    /// Choose the transition to take this tick, or `None`. Any-state edges win over
    /// per-state ones; `next` is the lowest-precedence fallback on clip completion.
    fn pick_transition(&self, inputs: &Inputs, clip_done: bool) -> Option<usize> {
        for t in &self.any {
            if satisfied(t.on, inputs, clip_done) && t.window.is_none_or(|w| w.contains(self.tick)) {
                return Some(t.to);
            }
        }
        for t in &self.states[self.current].transitions {
            if satisfied(t.on, inputs, clip_done) && t.window.is_none_or(|w| w.contains(self.tick)) {
                return Some(t.to);
            }
        }
        if clip_done {
            if let Some(n) = self.states[self.current].next {
                return Some(n);
            }
        }
        None
    }

    /// Hard-cut into a state: swap, reset the play-head, fire the entered state's
    /// tick-0 events.
    fn enter(&mut self, target: usize, fired: &mut Vec<FiredEvent>) {
        self.current = target;
        self.tick = 0;
        self.completed = false;
        self.fire_events_at(target, 0, fired);
    }
}

fn satisfied(on: Trigger, inputs: &Inputs, clip_done: bool) -> bool {
    match on {
        Trigger::Move => inputs.move_,
        Trigger::MoveStop => !inputs.move_,
        Trigger::Run => inputs.move_ && inputs.run,
        Trigger::RunStop => !inputs.run,
        Trigger::Crouch => inputs.crouch,
        Trigger::CrouchStop => !inputs.crouch,
        Trigger::Jump => inputs.jump,
        Trigger::Attack => inputs.attack,
        Trigger::Hit => inputs.hit,
        Trigger::Die => inputs.die,
        Trigger::ClipDone => clip_done,
    }
}

/// Load a `flicker.pack` JSON and return its state-machine definition.
pub fn load_pack(path: &Path) -> Result<StateMachineDef> {
    let text = std::fs::read_to_string(path)
        .with_context(|| format!("reading pack {}", path.display()))?;
    let pack: PackFile =
        serde_json::from_str(&text).with_context(|| format!("parsing pack {}", path.display()))?;
    Ok(pack.state_machine)
}

#[cfg(test)]
mod tests {
    use super::*;

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
    fn starts_in_initial() {
        let sm = build();
        assert_eq!(sm.current_state_name(), "Idle");
        assert_eq!(sm.current_tick(), 0);
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
}

//! The editor's document model.
//!
//! **The authored [`PackFile`] IS the document** — lossless, and exactly what [`save`]
//! writes back (including the hand-authored `_note` header). The runtime
//! [`StateMachine`] is only a derived *preview*, rebuilt from the def whenever the graph
//! changes; it is never read back into a def, because `def → machine` is lossy (clip and
//! state names collapse to indices, and transitions get priority-sorted at build).
//!
//! [`save`]: EditorDoc::save

use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::{Context, Result};
use flicker_skeletal::format::{self, Model};
use flicker_skeletal::state::{
    self, ClipRef, EventDef, HitType, PackFile, Response, ResponseMask, StateDef, StateMachine,
    StateMachineDef, TransitionDef, Trigger,
};

/// The canvas tool the pointer drives.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Tool {
    #[default]
    Select,
    /// Click empty canvas to append a state.
    AddState,
    /// Press one card, release on another, to weave a transition.
    Link,
    /// Click a card to remove it (and every edge pointing at it).
    Delete,
}

impl Tool {
    pub const ALL: [Tool; 4] = [Tool::Select, Tool::AddState, Tool::Link, Tool::Delete];

    /// Short rail label (the rail is only 56px wide).
    pub fn label(self) -> &'static str {
        match self {
            Tool::Select => "Sel",
            Tool::AddState => "+St",
            Tool::Link => "Tr",
            Tool::Delete => "Del",
        }
    }

    pub fn id(self) -> &'static str {
        match self {
            Tool::Select => "tool_select",
            Tool::AddState => "tool_add",
            Tool::Link => "tool_link",
            Tool::Delete => "tool_delete",
        }
    }
}

/// Triggers the editor can author, in cycle order. The strings are the serde
/// `snake_case` wire names, so what the rail shows is exactly what lands in the JSON.
pub const TRIGGERS: [(Trigger, &str); 16] = [
    (Trigger::ClipDone, "clip_done"),
    (Trigger::Move, "move"),
    (Trigger::MoveStop, "move_stop"),
    (Trigger::MoveForward, "move_forward"),
    (Trigger::MoveLeft, "move_left"),
    (Trigger::MoveRight, "move_right"),
    (Trigger::MoveBack, "move_back"),
    (Trigger::Run, "run"),
    (Trigger::RunStop, "run_stop"),
    (Trigger::Crouch, "crouch"),
    (Trigger::CrouchStop, "crouch_stop"),
    (Trigger::CrouchStill, "crouch_still"),
    (Trigger::Jump, "jump"),
    (Trigger::Attack, "attack"),
    (Trigger::Hit, "hit"),
    (Trigger::Die, "die"),
];

/// Wire name of a trigger.
pub fn trigger_label(t: Trigger) -> &'static str {
    TRIGGERS
        .iter()
        .find(|(k, _)| *k == t)
        .map(|(_, s)| *s)
        .unwrap_or("?")
}

/// The next trigger in cycle order (wraps).
pub fn next_trigger(t: Trigger) -> Trigger {
    let i = TRIGGERS.iter().position(|(k, _)| *k == t).unwrap_or(0);
    TRIGGERS[(i + 1) % TRIGGERS.len()].0
}

// ── Pure graph edits over the authored def ───────────────────────────────────
//
// Kept free of `EditorDoc` (and therefore of `Model`/GPU) so the graph semantics —
// especially the dangling-reference cleanup on delete — are unit-testable directly.

/// Append a state with a unique `State N` name bound to `clip`. Returns its index.
pub fn add_state_def(def: &mut StateMachineDef, clip: &str) -> usize {
    let mut n = def.states.len() + 1;
    let name = loop {
        let candidate = format!("State {n}");
        if !def.states.iter().any(|s| s.name == candidate) {
            break candidate;
        }
        n += 1;
    };
    def.states.push(StateDef {
        name,
        clip: clip.to_string(),
        looping: true,
        next: None,
        root_motion: false,
        stamina_cost: 0.0,
        blocking: false,
        guard_angle: None,
        transitions: Vec::new(),
        events: Vec::new(),
    });
    def.states.len() - 1
}

/// Remove state `index` **and every reference to it** — per-state transitions,
/// any-state edges, and `next` chains — so a delete can never leave the pack with a
/// dangling target. If it was the `initial` state, `initial` moves to the first
/// surviving state.
pub fn remove_state_def(def: &mut StateMachineDef, index: usize) -> bool {
    let Some(removed) = def.states.get(index).map(|s| s.name.clone()) else {
        return false;
    };
    def.states.remove(index);
    for s in &mut def.states {
        s.transitions.retain(|t| t.to != removed);
        if s.next.as_deref() == Some(removed.as_str()) {
            s.next = None;
        }
    }
    def.any.retain(|t| t.to != removed);
    if def.initial == removed {
        def.initial = def
            .states
            .first()
            .map(|s| s.name.clone())
            .unwrap_or_default();
    }
    true
}

/// Add a `from → to` transition on `on`. Rejects unknown indices and exact duplicates
/// (same target AND trigger), so repeated drags don't pile up identical edges.
pub fn add_transition_def(def: &mut StateMachineDef, from: usize, to: usize, on: Trigger) -> bool {
    let Some(target) = def.states.get(to).map(|s| s.name.clone()) else {
        return false;
    };
    let Some(src) = def.states.get_mut(from) else {
        return false;
    };
    if src.transitions.iter().any(|t| t.to == target && t.on == on) {
        return false;
    }
    src.transitions.push(TransitionDef {
        to: target,
        on,
        window: None,
        priority: 0,
        blend_ticks: None,
        on_incoming: None,
    });
    true
}

/// Identity of one authored transition: the state that owns it, and its position in that
/// state's list. An index alone would be ambiguous — two states can each have a third
/// transition — and a name pair would be ambiguous too, since the same `from → to` may
/// exist several times on different triggers.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct EdgeRef {
    pub from: usize,
    pub index: usize,
}

/// Borrow a transition by reference.
/// Addresses one authored TAE event: which state owns it, and its index in that state's
/// list. An index alone is ambiguous across states, so both halves travel together.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct EventRef {
    pub state: usize,
    pub index: usize,
}

/// Borrow an authored event.
pub fn event(def: &StateMachineDef, e: EventRef) -> Option<&EventDef> {
    def.states.get(e.state)?.events.get(e.index)
}

fn event_mut(def: &mut StateMachineDef, e: EventRef) -> Option<&mut EventDef> {
    def.states.get_mut(e.state)?.events.get_mut(e.index)
}

/// Move an event's start tick. A window keeps its END fixed, so dragging the start never
/// silently inverts the window — it is clamped to stay at or before the end.
pub fn set_event_tick(def: &mut StateMachineDef, e: EventRef, tick: u32) -> bool {
    let Some(ev) = event_mut(def, e) else {
        return false;
    };
    let tick = match ev.end {
        Some(end) => tick.min(end),
        None => tick,
    };
    if ev.tick == tick {
        return false;
    }
    ev.tick = tick;
    true
}

/// Move an event's end tick. `None` turns a window back into a one-shot, which is how the
/// inspector reaches "this is a point event" from the same control. An end below the start
/// is clamped up rather than rejected, so the stepper never dead-ends.
pub fn set_event_end(def: &mut StateMachineDef, e: EventRef, end: Option<u32>) -> bool {
    let Some(ev) = event_mut(def, e) else {
        return false;
    };
    let end = end.map(|v| v.max(ev.tick));
    if ev.end == end {
        return false;
    }
    ev.end = end;
    true
}

/// Cycle the hit type, creating the combat block on first touch so an un-annotated event
/// can be given combat metadata from the inspector without a separate "add" step.
pub fn cycle_hit_type(def: &mut StateMachineDef, e: EventRef) -> bool {
    let Some(ev) = event_mut(def, e) else {
        return false;
    };
    let combat = ev.combat.get_or_insert_with(Default::default);
    // Cycles the full taxonomy — `Sweep` and `Grab` are the perilous shapes a reactive
    // transition discriminates on, so they must be reachable from the inspector.
    combat.hit_type = Some(match combat.hit_type {
        None | Some(HitType::Grab) => HitType::Slash,
        Some(HitType::Slash) => HitType::Thrust,
        Some(HitType::Thrust) => HitType::Strike,
        Some(HitType::Strike) => HitType::Sweep,
        Some(HitType::Sweep) => HitType::Grab,
    });
    true
}

/// Nudge the hitbox capsule radius in metres. Clamped to a sane authoring range and
/// rounded to centimetres so the stepper cannot accumulate float dust.
pub fn nudge_capsule(def: &mut StateMachineDef, e: EventRef, delta: f32) -> bool {
    let Some(ev) = event_mut(def, e) else {
        return false;
    };
    let combat = ev.combat.get_or_insert_with(Default::default);
    let cur = combat.capsule_radius.unwrap_or(DEFAULT_CAPSULE_M);
    let next = ((cur + delta).clamp(MIN_CAPSULE_M, MAX_CAPSULE_M) * 100.0).round() / 100.0;
    if combat.capsule_radius == Some(next) {
        return false;
    }
    combat.capsule_radius = Some(next);
    true
}

/// Toggle one answer in an event's response mask, creating the combat block on first
/// touch. Refuses to clear the LAST answer: a window no answer can meet is unbeatable, not
/// perilous, so the mask can narrow to one and no further.
pub fn toggle_response(def: &mut StateMachineDef, e: EventRef, r: Response) -> bool {
    let Some(ev) = event_mut(def, e) else {
        return false;
    };
    let combat = ev.combat.get_or_insert_with(Default::default);
    let cur = combat.response_mask;
    let next: Vec<Response> = if cur.allows(r) {
        cur.responses().filter(|x| *x != r).collect()
    } else {
        cur.responses().chain(std::iter::once(r)).collect()
    };
    if next.is_empty() {
        return false; // an unanswerable attack is a bug, not a difficulty setting
    }
    let next = ResponseMask::from_slice(&next);
    if next == cur {
        return false;
    }
    combat.response_mask = next;
    true
}

/// Nudge the attacker-authored parry-window scale. `1.0` (no change) is the resting value
/// and clears the field, so a save never bakes in a scale that means "unchanged".
pub fn nudge_parry_scale(def: &mut StateMachineDef, e: EventRef, delta: f32) -> bool {
    let Some(ev) = event_mut(def, e) else {
        return false;
    };
    let combat = ev.combat.get_or_insert_with(Default::default);
    let cur = combat.parry_window_scale.unwrap_or(1.0);
    let next = ((cur + delta).clamp(MIN_PARRY_SCALE, 1.0) * 100.0).round() / 100.0;
    let next = (next < 1.0).then_some(next);
    if combat.parry_window_scale == next {
        return false;
    }
    combat.parry_window_scale = next;
    true
}

/// A parry window can be tightened but never widened past the defender's own — the
/// attacker authors difficulty, not the defender's stat.
const MIN_PARRY_SCALE: f32 = 0.2;

/// Authoring range for a hitbox capsule, in metres.
const DEFAULT_CAPSULE_M: f32 = 0.35;
const MIN_CAPSULE_M: f32 = 0.05;
const MAX_CAPSULE_M: f32 = 2.0;

pub fn transition(def: &StateMachineDef, e: EdgeRef) -> Option<&TransitionDef> {
    def.states.get(e.from)?.transitions.get(e.index)
}

fn transition_mut(def: &mut StateMachineDef, e: EdgeRef) -> Option<&mut TransitionDef> {
    def.states.get_mut(e.from)?.transitions.get_mut(e.index)
}

/// Remove one transition. Unlike removing a state this needs no reference sweep — nothing
/// in the pack points AT a transition, so it can only ever unlink two states.
pub fn remove_transition_def(def: &mut StateMachineDef, e: EdgeRef) -> bool {
    match def.states.get_mut(e.from) {
        Some(s) if e.index < s.transitions.len() => {
            s.transitions.remove(e.index);
            true
        }
        _ => false,
    }
}

/// Nudge a transition's priority. Higher is evaluated first; ties keep authored order.
pub fn set_priority(def: &mut StateMachineDef, e: EdgeRef, priority: i32) -> bool {
    match transition_mut(def, e) {
        Some(t) => {
            t.priority = priority.clamp(PRIORITY_MIN, PRIORITY_MAX);
            true
        }
        None => false,
    }
}

/// Set a transition's crossfade override. `None` inherits the machine's default —
/// authored as a real absence, so a save does not bake the current default into the file.
pub fn set_blend_ticks(def: &mut StateMachineDef, e: EdgeRef, blend: Option<u32>) -> bool {
    match transition_mut(def, e) {
        Some(t) => {
            t.blend_ticks = blend.map(|b| b.min(BLEND_MAX));
            true
        }
        None => false,
    }
}

/// Re-point a transition at a different trigger.
pub fn set_trigger(def: &mut StateMachineDef, e: EdgeRef, on: Trigger) -> bool {
    match transition_mut(def, e) {
        Some(t) => {
            t.on = on;
            true
        }
        None => false,
    }
}

/// Authoring limits. Priority is a small hand-tuned dial, not an arbitrary integer, and a
/// blend longer than a second is a mis-key rather than an intention.
pub const PRIORITY_MIN: i32 = -9;
pub const PRIORITY_MAX: i32 = 9;
pub const BLEND_MAX: u32 = 60;

/// Which page of the bench is showing.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Tab {
    #[default]
    StateMachine,
    PackBrowser,
    CreatureComposer,
    TaeEditor,
}

impl Tab {
    /// Tab order, left to right (matches the Loomforge Bench design).
    pub const ALL: [Tab; 4] = [
        Tab::StateMachine,
        Tab::PackBrowser,
        Tab::CreatureComposer,
        Tab::TaeEditor,
    ];

    /// Display label in the tab bar.
    pub fn label(self) -> &'static str {
        match self {
            Tab::StateMachine => "State Machine",
            Tab::PackBrowser => "Pack Browser",
            Tab::CreatureComposer => "Creature Composer",
            Tab::TaeEditor => "TAE Editor",
        }
    }

    /// Stable id — the tab button's node id and fired action name.
    pub fn id(self) -> &'static str {
        match self {
            Tab::StateMachine => "tab_sm",
            Tab::PackBrowser => "tab_pack",
            Tab::CreatureComposer => "tab_creature",
            Tab::TaeEditor => "tab_tae",
        }
    }
}

/// A loaded pack plus the clip library it resolves against.
pub struct EditorDoc {
    path: PathBuf,
    pack: PackFile,
    /// Shared: the bench's `Doll`s pose from this same rig, so a screenful of previews
    /// costs one skeleton and one clip library, not one per doll.
    model: Arc<Model>,
    preview: Option<StateMachine>,
    warnings: Vec<String>,
    selected: Option<usize>,
    dirty: bool,
}

impl EditorDoc {
    /// Load a `.pack.json` plus the rig/clip directories it references.
    pub fn load(pack_path: &Path, clip_dirs: &[&Path]) -> Result<Self> {
        let pack = state::read_pack(pack_path)
            .with_context(|| format!("loading pack {}", pack_path.display()))?;
        let model = format::load_dirs(clip_dirs).context("loading the clip library")?;
        let mut doc = Self {
            path: pack_path.to_path_buf(),
            pack,
            model: Arc::new(model),
            preview: None,
            warnings: Vec::new(),
            selected: None,
            dirty: false,
        };
        doc.rebuild_preview();
        Ok(doc)
    }

    /// Build a document straight from an in-memory pack + model (tests / future
    /// in-app authoring of a brand-new pack).
    pub fn from_parts(path: PathBuf, pack: PackFile, model: Model) -> Self {
        let mut doc = Self {
            path,
            pack,
            model: Arc::new(model),
            preview: None,
            warnings: Vec::new(),
            selected: None,
            dirty: false,
        };
        doc.rebuild_preview();
        doc
    }

    pub fn path(&self) -> &Path {
        &self.path
    }
    pub fn pack(&self) -> &PackFile {
        &self.pack
    }
    pub fn def(&self) -> &StateMachineDef {
        &self.pack.state_machine
    }
    pub fn states(&self) -> &[StateDef] {
        &self.pack.state_machine.states
    }
    pub fn model(&self) -> &Model {
        &self.model
    }
    /// The rig the bench's dolls share — a handle, not a copy.
    pub fn model_arc(&self) -> Arc<Model> {
        Arc::clone(&self.model)
    }
    /// Unsaved edits pending — drives the Save button's enabled/dirty affordance.
    pub fn dirty(&self) -> bool {
        self.dirty
    }
    /// Soft validation from the last preview build (unresolved clip / state names).
    /// Surfaced by "Validate Rig"; never fatal.
    pub fn warnings(&self) -> &[String] {
        &self.warnings
    }
    pub fn preview(&self) -> Option<&StateMachine> {
        self.preview.as_ref()
    }
    pub fn preview_mut(&mut self) -> Option<&mut StateMachine> {
        self.preview.as_mut()
    }
    pub fn selected(&self) -> Option<usize> {
        self.selected
    }

    /// Clip names the editor can bind — the resolved library names (In-Place clips are
    /// bare stems, root-motion clips are `RM/`-prefixed).
    pub fn clip_names(&self) -> impl Iterator<Item = &str> {
        self.model.clips.iter().map(|c| c.name.as_str())
    }

    /// Index of a state by name.
    pub fn state_index(&self, name: &str) -> Option<usize> {
        self.pack
            .state_machine
            .states
            .iter()
            .position(|s| s.name == name)
    }

    /// Index of a clip in the resolved library — what a Stage doll poses with. `None`
    /// for a name the library never resolved, which stages as the rest pose.
    pub fn clip_index(&self, name: &str) -> Option<usize> {
        self.model.clips.iter().position(|c| c.name == name)
    }

    /// A clip's `(frames, ticks_per_second)` — the TAE timeline's frame axis and the rate
    /// its playhead advances at. `None` for a clip the library never resolved.
    pub fn clip_axis(&self, name: &str) -> Option<(u32, u32)> {
        self.model
            .clips
            .iter()
            .find(|c| c.name == name)
            .map(|c| (c.duration_ticks, c.tick_rate_hz))
    }

    /// Select a state by index; an out-of-range index clears the selection.
    pub fn select(&mut self, index: Option<usize>) {
        self.selected = index.filter(|i| *i < self.pack.state_machine.states.len());
    }

    /// Bind `clip` to state `index` — the clip→state drag-drop edit. Returns `false`
    /// (a no-op) if the state or the clip is unknown, so a bad drop can never write a
    /// dangling clip name into the pack.
    pub fn bind_clip(&mut self, index: usize, clip: &str) -> bool {
        if !self.model.clips.iter().any(|c| c.name == clip) {
            return false;
        }
        let Some(state) = self.pack.state_machine.states.get_mut(index) else {
            return false;
        };
        if state.clip == clip {
            return true; // legal drop, nothing changed
        }
        state.clip = clip.to_string();
        self.dirty = true;
        self.rebuild_preview();
        true
    }

    /// Append a state bound to `clip` (the canvas's Add State tool) and select it.
    /// Returns `None` if the clip isn't in the library.
    pub fn add_state(&mut self, clip: &str) -> Option<usize> {
        if !self.model.clips.iter().any(|c| c.name == clip) {
            return None;
        }
        let i = add_state_def(&mut self.pack.state_machine, clip);
        self.selected = Some(i);
        self.dirty = true;
        self.rebuild_preview();
        Some(i)
    }

    /// Remove a state and every edge pointing at it. Keeps the selection in range.
    pub fn remove_state(&mut self, index: usize) -> bool {
        if !remove_state_def(&mut self.pack.state_machine, index) {
            return false;
        }
        let len = self.pack.state_machine.states.len();
        self.selected = match self.selected {
            Some(s) if s == index => None,
            Some(s) if s > index => Some(s - 1),
            other => other.filter(|s| *s < len),
        };
        self.dirty = true;
        self.rebuild_preview();
        true
    }

    /// Weave a `from → to` transition (the canvas's Link tool).
    pub fn add_transition(&mut self, from: usize, to: usize, on: Trigger) -> bool {
        if !add_transition_def(&mut self.pack.state_machine, from, to, on) {
            return false;
        }
        self.dirty = true;
        self.rebuild_preview();
        true
    }

    /// Remove a transition (the canvas's Delete tool, applied to an edge).
    pub fn remove_transition(&mut self, e: EdgeRef) -> bool {
        self.edit(|def| remove_transition_def(def, e))
    }

    /// Step a transition's priority by `delta`, clamped to the authoring range.
    pub fn nudge_priority(&mut self, e: EdgeRef, delta: i32) -> bool {
        let Some(cur) = transition(self.def(), e).map(|t| t.priority) else {
            return false;
        };
        self.edit(|def| set_priority(def, e, cur.saturating_add(delta)))
    }

    /// Step a transition's blend override. Stepping below zero clears it back to `None`,
    /// so "inherit the machine default" stays reachable from the same control.
    pub fn nudge_blend(&mut self, e: EdgeRef, delta: i32) -> bool {
        let Some(cur) = transition(self.def(), e).map(|t| t.blend_ticks) else {
            return false;
        };
        let next = match (cur, delta) {
            (None, d) if d > 0 => Some(0),
            (None, _) => None,
            (Some(b), d) => {
                let n = b as i32 + d;
                (n >= 0).then_some(n as u32)
            }
        };
        self.edit(|def| set_blend_ticks(def, e, next))
    }

    /// Step an event's start tick by `delta` frames (saturating at zero).
    pub fn nudge_event_tick(&mut self, e: EventRef, delta: i32) -> bool {
        let Some(cur) = event(self.def(), e).map(|ev| ev.tick) else {
            return false;
        };
        let next = (cur as i32 + delta).max(0) as u32;
        self.edit(|def| set_event_tick(def, e, next))
    }

    /// Step an event's end tick. Stepping below the start drops the end entirely, turning
    /// the window back into a one-shot — so "make this a point event" is reachable from
    /// the same control that shortens it.
    pub fn nudge_event_end(&mut self, e: EventRef, delta: i32) -> bool {
        let Some(ev) = event(self.def(), e) else {
            return false;
        };
        let (tick, cur) = (ev.tick, ev.end);
        let next = match (cur, delta) {
            (None, d) if d > 0 => Some(tick + d as u32),
            (None, _) => None,
            (Some(end), d) => {
                let n = end as i32 + d;
                (n >= tick as i32).then_some(n as u32)
            }
        };
        self.edit(|def| set_event_end(def, e, next))
    }

    pub fn cycle_event_hit_type(&mut self, e: EventRef) -> bool {
        self.edit(|def| cycle_hit_type(def, e))
    }

    pub fn nudge_event_capsule(&mut self, e: EventRef, delta: f32) -> bool {
        self.edit(|def| nudge_capsule(def, e, delta))
    }

    pub fn toggle_event_response(&mut self, e: EventRef, r: Response) -> bool {
        self.edit(|def| toggle_response(def, e, r))
    }

    pub fn nudge_event_parry_scale(&mut self, e: EventRef, delta: f32) -> bool {
        self.edit(|def| nudge_parry_scale(def, e, delta))
    }

    /// Cycle a transition onto the next trigger.
    pub fn cycle_trigger(&mut self, e: EdgeRef) -> bool {
        let Some(on) = transition(self.def(), e).map(|t| next_trigger(t.on)) else {
            return false;
        };
        self.edit(|def| set_trigger(def, e, on))
    }

    /// Apply a graph edit, marking the document dirty and refreshing the preview only if
    /// the edit actually changed something — so a rejected edit can never dirty a save.
    fn edit(&mut self, f: impl FnOnce(&mut StateMachineDef) -> bool) -> bool {
        if !f(&mut self.pack.state_machine) {
            return false;
        }
        self.dirty = true;
        self.rebuild_preview();
        true
    }

    /// Rebuild the derived preview machine from the authored def. Cheap enough to run
    /// on every edit (it resolves names to indices and sorts transitions).
    pub fn rebuild_preview(&mut self) {
        // Scope the clip borrow so the assignments below can take `&mut self`.
        let built = {
            let clips: Vec<ClipRef> = self
                .model
                .clips
                .iter()
                .map(|c| ClipRef {
                    name: c.name.as_str(),
                    duration_ticks: c.duration_ticks,
                })
                .collect();
            StateMachine::build(&self.pack.state_machine, &clips)
        };
        match built {
            Ok(sm) => {
                self.warnings = sm.warnings().to_vec();
                self.preview = Some(sm);
            }
            Err(e) => {
                self.warnings = vec![format!("preview unavailable: {e}")];
                self.preview = None;
            }
        }
    }

    /// Write the document back to its `.pack.json`, round-tripping the `_note` header.
    pub fn save(&mut self) -> Result<()> {
        state::write_pack(&self.path, &self.pack)?;
        self.dirty = false;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A three-state graph: Idle ⇄ Walk, plus an any-state edge into Hit and a
    /// `next` chain Walk → Hit — i.e. every kind of reference a delete must clean up.
    fn def() -> StateMachineDef {
        let st = |name: &str, clip: &str, transitions: Vec<TransitionDef>| StateDef {
            name: name.into(),
            clip: clip.into(),
            looping: true,
            next: None,
            root_motion: false,
            stamina_cost: 0.0,
            blocking: false,
            guard_angle: None,
            transitions,
            events: Vec::new(),
        };
        let tr = |to: &str, on: Trigger| TransitionDef {
            to: to.into(),
            on,
            window: None,
            priority: 0,
            blend_ticks: None,
            on_incoming: None,
        };
        let mut walk = st("Walk", "walk", vec![tr("Idle", Trigger::MoveStop)]);
        walk.next = Some("Hit".into());
        StateMachineDef {
            initial: "Idle".into(),
            default_blend_ticks: 0,
            tick_rate_hz: 60,
            any: vec![tr("Hit", Trigger::Hit)],
            states: vec![
                st("Idle", "idle", vec![tr("Walk", Trigger::Move)]),
                walk,
                st("Hit", "hit", vec![]),
            ],
        }
    }

    /// A def whose first state carries one window event, for the event-edit tests.
    fn def_with_event() -> StateMachineDef {
        let mut d = def();
        d.states[0].events = vec![EventDef {
            tick: 10,
            end: Some(20),
            kind: state::EventKind::HitboxActive,
            label: "Weapon_R".into(),
            combat: None,
        }];
        d
    }

    #[test]
    fn moving_a_window_start_never_inverts_it() {
        let mut d = def_with_event();
        let e = EventRef { state: 0, index: 0 };
        // Dragging the start past the end clamps to the end, not through it.
        assert!(set_event_tick(&mut d, e, 25));
        let ev = event(&d, e).unwrap();
        assert_eq!(ev.tick, 20, "start clamps at the end");
        assert_eq!(ev.end, Some(20), "end is untouched");
    }

    #[test]
    fn end_below_start_turns_a_window_into_a_point() {
        let mut d = def_with_event();
        let e = EventRef { state: 0, index: 0 };
        // tick=10, end=20; step end down past the start → None (point event).
        for _ in 0..12 {
            let next = event(&d, e).unwrap().end.map(|v| v.saturating_sub(1));
            set_event_end(&mut d, e, next);
        }
        // Reaching the start keeps a zero-length window; going below is where the doc
        // wrapper (nudge_event_end) converts it to None. Here we assert the clamp floor.
        assert_eq!(event(&d, e).unwrap().end, Some(10));
    }

    #[test]
    fn cycling_hit_type_creates_combat_metadata_on_first_touch() {
        let mut d = def_with_event();
        let e = EventRef { state: 0, index: 0 };
        assert!(event(&d, e).unwrap().combat.is_none());
        assert!(cycle_hit_type(&mut d, e));
        let hit = event(&d, e).unwrap().combat.as_ref().unwrap().hit_type;
        assert_eq!(
            hit,
            Some(HitType::Slash),
            "first cycle lands on the first type"
        );
        // The full taxonomy wraps: Slash→Thrust→Strike→Sweep→Grab→Slash.
        let seen: Vec<HitType> = (0..4)
            .map(|_| {
                cycle_hit_type(&mut d, e);
                event(&d, e)
                    .unwrap()
                    .combat
                    .as_ref()
                    .unwrap()
                    .hit_type
                    .unwrap()
            })
            .collect();
        assert_eq!(
            seen,
            vec![
                HitType::Thrust,
                HitType::Strike,
                HitType::Sweep,
                HitType::Grab
            ],
            "every hit shape must be reachable from the inspector"
        );
        cycle_hit_type(&mut d, e);
        assert_eq!(
            event(&d, e).unwrap().combat.as_ref().unwrap().hit_type,
            Some(HitType::Slash)
        );
    }

    #[test]
    fn capsule_radius_stays_in_range_and_snaps_to_cm() {
        let mut d = def_with_event();
        let e = EventRef { state: 0, index: 0 };
        // Step down hard — must floor, not go negative.
        for _ in 0..50 {
            nudge_capsule(&mut d, e, -0.05);
        }
        let r = event(&d, e)
            .unwrap()
            .combat
            .as_ref()
            .unwrap()
            .capsule_radius
            .unwrap();
        assert!((MIN_CAPSULE_M..=MAX_CAPSULE_M).contains(&r));
        assert_eq!(
            (r * 100.0).round(),
            r * 100.0,
            "snapped to whole centimetres"
        );
    }

    /// Narrowing the mask to one answer is how an attack becomes perilous — but the LAST
    /// answer can never be cleared, because a window nothing can answer is unbeatable
    /// rather than hard.
    #[test]
    fn response_mask_narrows_to_perilous_but_never_to_nothing() {
        let mut d = def_with_event();
        let e = EventRef { state: 0, index: 0 };
        // Starts permissive (the default), so an un-annotated attack is unchanged.
        let mask = |d: &StateMachineDef| {
            event(d, e)
                .unwrap()
                .combat
                .as_ref()
                .map(|c| c.response_mask)
                .unwrap_or_default()
        };
        assert!(mask(&d).is_all());

        // Turn it into a sweep: only Jump remains.
        for r in [
            Response::Block,
            Response::Parry,
            Response::Dodge,
            Response::Counter,
        ] {
            assert!(toggle_response(&mut d, e, r), "clearing {r:?} should apply");
        }
        assert!(mask(&d).is_perilous());
        assert!(mask(&d).allows(Response::Jump));

        // Clearing the last one is REFUSED — and leaves the mask untouched.
        assert!(
            !toggle_response(&mut d, e, Response::Jump),
            "cannot clear the last answer"
        );
        assert!(
            mask(&d).allows(Response::Jump),
            "mask survived the refused edit"
        );
    }

    /// `1.0` means "unchanged", so it must clear the field rather than persist a value that
    /// says nothing — otherwise every touched event would bake a no-op scale into the file.
    #[test]
    fn parry_scale_clears_at_one_and_clamps_at_the_floor() {
        let mut d = def_with_event();
        let e = EventRef { state: 0, index: 0 };
        let scale = |d: &StateMachineDef| {
            event(d, e)
                .unwrap()
                .combat
                .as_ref()
                .and_then(|c| c.parry_window_scale)
        };

        assert!(nudge_parry_scale(&mut d, e, -0.05));
        assert_eq!(scale(&d), Some(0.95), "tightening records a real value");
        // Back up to 1.0 → the field goes away again.
        assert!(nudge_parry_scale(&mut d, e, 0.05));
        assert_eq!(scale(&d), None, "1.0 means unchanged, so it is not written");
        // Cannot be tightened past the floor.
        for _ in 0..50 {
            nudge_parry_scale(&mut d, e, -0.05);
        }
        assert!(scale(&d).unwrap() >= MIN_PARRY_SCALE);
    }

    #[test]
    fn editing_an_event_out_of_range_is_a_no_op() {
        let mut d = def_with_event();
        let miss = EventRef { state: 9, index: 9 };
        assert!(!set_event_tick(&mut d, miss, 5));
        assert!(!cycle_hit_type(&mut d, miss));
        assert!(!nudge_capsule(&mut d, miss, 0.05));
    }

    #[test]
    fn add_state_appends_with_a_unique_name() {
        let mut d = def();
        let i = add_state_def(&mut d, "idle");
        assert_eq!(i, 3);
        assert_eq!(d.states[3].name, "State 4");
        assert_eq!(d.states[3].clip, "idle");
        // Adding again must not collide, even if a "State 5" already exists.
        d.states[3].name = "State 5".into();
        let j = add_state_def(&mut d, "idle");
        assert_ne!(d.states[j].name, "State 5", "names stay unique");
        assert_eq!(
            d.states
                .iter()
                .filter(|s| s.name == d.states[j].name)
                .count(),
            1
        );
    }

    #[test]
    fn remove_state_purges_every_reference_to_it() {
        let mut d = def();
        // Remove "Hit" — referenced by an any-edge AND by Walk's `next`.
        assert!(remove_state_def(&mut d, 2));
        assert_eq!(d.states.len(), 2);
        assert!(
            d.any.is_empty(),
            "any-state edge into the deleted state is dropped"
        );
        assert!(
            d.states.iter().all(|s| s.next.is_none()),
            "dangling `next` cleared"
        );
        assert!(
            d.states
                .iter()
                .flat_map(|s| &s.transitions)
                .all(|t| t.to != "Hit"),
            "no transition may point at a deleted state"
        );
    }

    #[test]
    fn removing_the_initial_state_repoints_initial() {
        let mut d = def();
        assert_eq!(d.initial, "Idle");
        assert!(remove_state_def(&mut d, 0));
        assert_eq!(d.initial, "Walk", "initial moves to the first survivor");
        // And deleting everything leaves a valid (if empty) def rather than a dangling name.
        assert!(remove_state_def(&mut d, 0));
        assert!(remove_state_def(&mut d, 0));
        assert!(d.states.is_empty());
        assert_eq!(d.initial, "");
        assert!(
            !remove_state_def(&mut d, 0),
            "out-of-range delete is a no-op"
        );
    }

    #[test]
    fn add_transition_rejects_duplicates_and_bad_indices() {
        let mut d = def();
        // Idle already has Idle→Walk on `move`; the same edge again is refused.
        assert!(
            !add_transition_def(&mut d, 0, 1, Trigger::Move),
            "exact duplicate refused"
        );
        // Same target, different trigger is a legitimately different edge.
        assert!(add_transition_def(&mut d, 0, 1, Trigger::Run));
        assert_eq!(d.states[0].transitions.len(), 2);
        // Unknown endpoints are no-ops.
        assert!(!add_transition_def(&mut d, 0, 99, Trigger::Move));
        assert!(!add_transition_def(&mut d, 99, 0, Trigger::Move));
    }

    /// Removing a transition unlinks two states and touches nothing else — the mirror of
    /// `remove_state_def`, which has to sweep every reference.
    #[test]
    fn remove_transition_unlinks_only_that_edge() {
        let mut d = def();
        let states_before = d.states.len();
        let e = EdgeRef { from: 0, index: 0 }; // Idle → Walk on `move`
        assert_eq!(transition(&d, e).map(|t| t.to.as_str()), Some("Walk"));
        assert!(remove_transition_def(&mut d, e));
        assert!(d.states[0].transitions.is_empty(), "the edge is gone");
        assert_eq!(d.states.len(), states_before, "both states survive");
        assert_eq!(d.any.len(), 1, "any-state edges are untouched");
        // Out-of-range removals are no-ops, not panics.
        assert!(!remove_transition_def(
            &mut d,
            EdgeRef { from: 0, index: 0 }
        ));
        assert!(!remove_transition_def(
            &mut d,
            EdgeRef { from: 99, index: 0 }
        ));
    }

    #[test]
    fn transition_dials_clamp_to_the_authoring_range() {
        let mut d = def();
        let e = EdgeRef { from: 0, index: 0 };
        assert!(set_priority(&mut d, e, 500));
        assert_eq!(transition(&d, e).unwrap().priority, PRIORITY_MAX);
        assert!(set_priority(&mut d, e, -500));
        assert_eq!(transition(&d, e).unwrap().priority, PRIORITY_MIN);
        assert!(set_blend_ticks(&mut d, e, Some(9999)));
        assert_eq!(transition(&d, e).unwrap().blend_ticks, Some(BLEND_MAX));
        // Clearing the override must produce a real absence, so a save omits the key and
        // the transition keeps inheriting the machine default.
        assert!(set_blend_ticks(&mut d, e, None));
        assert_eq!(transition(&d, e).unwrap().blend_ticks, None);
        // A stale edge is refused rather than silently editing a neighbour.
        assert!(!set_priority(&mut d, EdgeRef { from: 0, index: 7 }, 1));
        assert!(!set_trigger(
            &mut d,
            EdgeRef { from: 42, index: 0 },
            Trigger::Jump
        ));
    }

    /// Stepping the blend below zero returns it to "inherit", so the author can reach
    /// every state of the field — including absence — from the one stepper.
    #[test]
    fn blend_steps_down_into_inherit() {
        let model = format::Model {
            bones: Vec::new(),
            clips: Vec::new(),
            mesh: Default::default(),
            source: Default::default(),
            world: glam::Mat4::IDENTITY,
            orbit_radius: 1.0,
            retarget: false,
            attach: Default::default(),
            collision: Default::default(),
        };
        let pack = PackFile {
            format: "flicker.pack".into(),
            version: 1,
            note: String::new(),
            state_machine: def(),
        };
        let mut doc = EditorDoc::from_parts(PathBuf::from("/tmp/x.pack.json"), pack, model);
        let e = EdgeRef { from: 0, index: 0 };
        assert_eq!(doc.def().states[0].transitions[0].blend_ticks, None);
        assert!(!doc.dirty(), "loading is not an edit");

        doc.nudge_blend(e, 1);
        assert_eq!(
            doc.def().states[0].transitions[0].blend_ticks,
            Some(0),
            "leaves inherit at 0"
        );
        doc.nudge_blend(e, 1);
        assert_eq!(doc.def().states[0].transitions[0].blend_ticks, Some(1));
        doc.nudge_blend(e, -1);
        doc.nudge_blend(e, -1);
        assert_eq!(
            doc.def().states[0].transitions[0].blend_ticks,
            None,
            "stepping past zero returns to inheriting the machine default"
        );
        assert!(doc.dirty(), "the edits dirtied the document");

        // Priority steps and clamps through the document too.
        for _ in 0..20 {
            doc.nudge_priority(e, 1);
        }
        assert_eq!(doc.def().states[0].transitions[0].priority, PRIORITY_MAX);
    }

    #[test]
    fn trigger_cycle_covers_every_variant_and_wraps() {
        let mut t = TRIGGERS[0].0;
        let mut seen = Vec::new();
        for _ in 0..TRIGGERS.len() {
            seen.push(trigger_label(t));
            t = next_trigger(t);
        }
        assert_eq!(t, TRIGGERS[0].0, "cycling the full list wraps to the start");
        seen.sort_unstable();
        seen.dedup();
        assert_eq!(
            seen.len(),
            TRIGGERS.len(),
            "every trigger has a distinct wire name"
        );
    }
}

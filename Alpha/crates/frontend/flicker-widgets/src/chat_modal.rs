//! `ChatModal` — the in-world comms window as a SHARED COMPONENT (the ruling
//! AC8B84BD, called in by Aaron 2026-08-29): one Rust-owned modal that owns the
//! floating panel's whole life — rect, titlebar-drag move, corner resize, the
//! channel tabs, the scrolling log, the roster rail, the input row and the
//! keyboard-focus mirror — built over the [`chat_panel`] tree and run as its own
//! walker pass. The same shape as `flicker-globe`'s `WorldMap`: a component
//! STRUCT the scene hosts, not a bespoke block the scene re-implements.
//!
//! What stays OUTSIDE, by design:
//! - **The socket.** The clay-chat client (flicker-net) is scene-owned today;
//!   the modal exposes granular mutators that mirror the protocol's events, so
//!   the scene's drain loop is a forwarding match and this crate never grows a
//!   net dependency. (The cross-scene persistent connection service is its own
//!   sitting — AC8B84BD §3.)
//! - **The keyboard hand-off.** Entering and leaving text entry is the WALKER's
//!   (the `EnterText` switch, `SubmitText` / `CancelText` exits — Aaron 2026-09-03);
//!   the host's exclusive-owner layer only mirrors the session for the chain. The
//!   modal takes the derived truth (`focused`) and re-asserts walker focus from it.
//!
//! Localization: the modal's system lines resolve `$chat_*` stringtable tokens
//! (shared component, shared strings — never scene-prefixed English).

use flicker_input_core::InputState;
use flicker_render::Vec2;
use flicker_script::{HudCommand, UiNode, ValueMap};
use std::collections::HashMap;

use crate::chat_panel::{chat_panel, ChatLineKind, ChatLineView, ChatView, RosterEntry, CORNER};
use crate::{run_ui, strings, UiInput, UiState};

// Window hit regions + minimum size (device px). The grip is the titlebar strip
// that drags the window; the corner box resizes it. Hit rects only — the panel
// draws its own chrome.
const GRIP_H: f32 = 34.0;
const MIN_W: f32 = 420.0;
const MIN_H: f32 = 180.0;
/// The scrollback ring per channel.
const LOG_CAP: usize = 300;

/// An in-flight drag of the window (`Move` remembers the grab offset from the
/// window's top-left so the window tracks the cursor).
#[derive(Copy, Clone, PartialEq)]
enum Drag {
    None,
    Move { grab: Vec2 },
    Resize,
}

/// One frame's answers from [`ChatModal::run`].
#[derive(Default)]
pub struct ChatFrame {
    /// The pointer sits over the window — the walker consumed the click.
    pub hit: bool,
    /// The SEND button fired this frame.
    pub send: bool,
    /// The join (＋) button fired this frame.
    pub join: bool,
    /// The leave (✕) button fired this frame.
    pub part: bool,
}

/// The shared chat window. Host it, feed it the wire's events through the
/// mutators, call [`update_pointer`](Self::update_pointer) then
/// [`run`](Self::run) each frame, and layer [`commands`](Self::commands) over
/// the HUD.
pub struct ChatModal {
    /// Dotted style-block prefix (e.g. `"pocclusters.chat"`) — the host scene's
    /// palette; no colour lives here.
    style: String,
    rect: (f32, f32, f32, f32),
    drag: Drag,
    /// Retained walker state for the modal's own pass (keyboard focus).
    ui: UiState,
    commands: Vec<HudCommand>,
    /// The last pass's tree + model, for the host's walker layer (`walker_parts`).
    nav: Option<(UiNode, ValueMap)>,
    nick: String,
    active: String,
    channels: Vec<String>,
    logs: HashMap<String, Vec<ChatLineView>>,
    rosters: HashMap<String, Vec<RosterEntry>>,
    /// The input field's current text (mirrors the walker `chat_input` bind).
    input: String,
    scroll: f32,
}

impl ChatModal {
    pub fn new(style_prefix: &str) -> Self {
        Self {
            style: style_prefix.to_string(),
            rect: (0.0, 0.0, 0.0, 0.0),
            drag: Drag::None,
            ui: UiState::new(),
            commands: Vec::new(),
            nav: None,
            nick: String::new(),
            active: String::new(),
            channels: Vec::new(),
            logs: HashMap::new(),
            rosters: HashMap::new(),
            input: String::new(),
            scroll: f32::MAX,
        }
    }

    // ── hosting ──────────────────────────────────────────────────────────────

    /// Float the window bottom-centre, ~3/5 of the screen wide (wide, not
    /// docked) — the component's own default placement.
    pub fn place_default(&mut self, screen: Vec2) {
        let w = (screen.x * 0.6).clamp(MIN_W, (screen.x - 40.0).max(MIN_W));
        let h = (screen.y * 0.42).clamp(MIN_H, (screen.y - 40.0).max(MIN_H));
        let x = ((screen.x - w) * 0.5).max(0.0);
        let y = (screen.y - h - 24.0).max(0.0);
        self.rect = (x, y, w, h);
    }

    /// Seed the session identity: our nick and the opening channel.
    pub fn open_session(&mut self, nick: &str, channel: &str) {
        self.nick = nick.to_string();
        self.active = channel.to_string();
        self.channels = vec![channel.to_string()];
    }

    pub fn nick(&self) -> &str {
        &self.nick
    }
    pub fn active(&self) -> &str {
        &self.active
    }
    /// The input line as it stands (the join button reads a channel name here).
    pub fn input_text(&self) -> &str {
        &self.input
    }
    /// Drain the input line (a submit posted it, or a join consumed it).
    pub fn take_input(&mut self) -> String {
        std::mem::take(&mut self.input)
    }
    pub fn commands(&self) -> &[HudCommand] {
        &self.commands
    }
    /// The modal's retained walker state, for the HOST's router chain: the
    /// dispatch walker layer wraps this so the chain sees the chat field's
    /// keyboard focus (input arbitration stays the host's — 4B15929B).
    pub fn walker_state(&mut self) -> &mut UiState {
        &mut self.ui
    }

    /// The walker state AND the tree/model of the last pass, split-borrowed so the
    /// host's walker layer can be built navigable over the chat tree (its
    /// `text_field` is what `EnterText` targets by default).
    pub fn walker_parts(&mut self) -> (&mut UiState, Option<(&UiNode, &ValueMap)>) {
        (&mut self.ui, self.nav.as_ref().map(|(t, m)| (t, m)))
    }

    /// Is the chat line's text-entry session open — does chat own the keyboard?
    pub fn text_entry(&self) -> bool {
        self.ui.text_entry()
    }

    // ── the per-frame pass ───────────────────────────────────────────────────

    /// The POINTER phase — window move/resize plus click-to-enter detection.
    /// Runs BEFORE the host's keyboard hand-off (`CommandHandler::drive` wants
    /// this frame's `click_focus`), which in turn runs before [`run`].
    /// Returns whether a left-press landed in the window this frame.
    pub fn update_pointer(&mut self, input: &InputState, screen: Vec2) -> bool {
        let (mut cx, mut cy, mut cw, mut ch) = self.rect;
        let m = input.mouse_position;
        let mut click_focus = false;
        let over =
            |x: f32, y: f32, w: f32, h: f32| m.x >= x && m.x < x + w && m.y >= y && m.y < y + h;
        if input.mouse_left_pressed {
            if over(cx + cw - CORNER, cy + ch - CORNER, CORNER, CORNER) {
                self.drag = Drag::Resize;
                click_focus = true;
            } else if over(cx, cy, cw, GRIP_H) {
                self.drag = Drag::Move {
                    grab: Vec2::new(m.x - cx, m.y - cy),
                };
                click_focus = true;
            } else if over(cx, cy, cw, ch) {
                click_focus = true;
            }
        }
        if input.mouse_left {
            match self.drag {
                Drag::Move { grab } => {
                    cx = (m.x - grab.x).clamp(0.0, (screen.x - cw).max(0.0));
                    cy = (m.y - grab.y).clamp(0.0, (screen.y - ch).max(0.0));
                }
                Drag::Resize => {
                    cw = (m.x - cx).clamp(MIN_W, (screen.x - cx).max(MIN_W));
                    ch = (m.y - cy).clamp(MIN_H, (screen.y - cy).max(MIN_H));
                }
                Drag::None => {}
            }
        } else {
            self.drag = Drag::None;
        }
        self.rect = (cx, cy, cw, ch);
        click_focus
    }

    /// Build and walk the window for one frame. `focused` is the session truth
    /// ([`text_entry`](Self::text_entry), read by the host before the pass); `styles`
    /// is the host's folded style tree (the modal's dotted prefix resolves into it);
    /// `sections` is the host's declared-surface publish, carried so the tree's
    /// `visible_bind` gate stays S9 data.
    pub fn run(
        &mut self,
        input: &InputState,
        screen: Vec2,
        focused: bool,
        styles: &serde_json::Value,
        sections: &ValueMap,
    ) -> ChatFrame {
        // Re-assert the field's focus each frame (run_ui clears focus on any
        // clicked frame) from the context-derived truth.
        if focused {
            self.ui.request_focus("chat_input");
        } else {
            self.ui.clear_focus();
        }

        let (cx, cy, cw, ch) = self.rect;
        let empty_lines: Vec<ChatLineView> = Vec::new();
        let empty_roster: Vec<RosterEntry> = Vec::new();
        let lines = self.logs.get(&self.active).unwrap_or(&empty_lines);
        let roster = self.rosters.get(&self.active).unwrap_or(&empty_roster);
        let you_label = strings::resolve("$chat_you");
        let mut tree = chat_panel(
            cx,
            cy,
            cw,
            ch,
            &ChatView {
                style: &self.style,
                active: &self.active,
                channels: &self.channels,
                lines,
                roster,
                nick: &self.nick,
                you_label: &you_label,
            },
        );
        // The window is a DECLARED surface of the host screen: its root rides
        // the `chat` gate, so hiding it is a helper call, not a code path (S9).
        tree.visible_bind = Some("chat".into());

        let mut model = ValueMap::new();
        model.extend(sections.clone());
        // The tab strip selects by INDEX (an index is a number, everywhere).
        let active_idx = self
            .channels
            .iter()
            .position(|c| c == &self.active)
            .unwrap_or(0);
        model.set("chat_tab", active_idx as f64);
        model.set("chat_scroll", f64::from(self.scroll));
        model.set("chat_input", self.input.as_str());

        let uin = UiInput {
            mouse: input.mouse_position,
            // Suppress the walker click while a title/corner drag is in flight,
            // so a drag never also toggles a tab or button under the cursor.
            clicked: input.mouse_left_pressed && self.drag == Drag::None,
            down: input.mouse_left,
            right_down: input.mouse_right,
            screen,
            wheel: input.mouse_wheel_delta,
            exclusive: false,
            motion: Default::default(),
        };
        let frame = run_ui(&tree, &model, styles, &uin, &mut self.ui);
        self.commands = frame.commands;
        self.nav = Some((tree, model));

        if let Some(t) = frame.results.text("chat_input") {
            self.input = t.to_string();
        }
        if let Some(s) = frame.results.number("chat_scroll") {
            self.scroll = s as f32;
        }
        if let Some(sel) = frame.results.number("chat_tab") {
            if let Some(channel) = self.channels.get(sel as usize) {
                if channel != &self.active {
                    self.active = channel.clone();
                    self.scroll = f32::MAX;
                }
            }
        }

        ChatFrame {
            hit: frame.results.is_on("hud_hit"),
            send: frame.results.is_on("chat_send"),
            join: frame.results.is_on("chat_join"),
            part: frame.results.is_on("chat_part"),
        }
    }

    // ── the wire's events, as view mutations ─────────────────────────────────
    // One mutator per protocol event; the host's drain loop is a forwarding
    // match, so this crate never learns the socket's types.

    pub fn connected(&mut self) {
        let active = self.active.clone();
        self.push_line(
            &active,
            ChatLineKind::Joined,
            format!("· {}", strings::resolve("$chat_connected")),
        );
    }

    pub fn disconnected(&mut self, reason: Option<&str>) {
        let active = self.active.clone();
        let msg = match reason {
            Some(r) => format!("· {} — {r}", strings::resolve("$chat_disconnected")),
            None => format!("· {}", strings::resolve("$chat_disconnected")),
        };
        self.push_line(&active, ChatLineKind::Left, msg);
    }

    /// A chat line arrived. `/me` renders as an emote; our own nick as You.
    pub fn message(&mut self, channel: &str, from: &str, text: &str) {
        let (kind, line) = if let Some(rest) = text.strip_prefix("/me ") {
            (ChatLineKind::Emote, format!("✦ {from} {rest}"))
        } else if from == self.nick {
            (ChatLineKind::You, format!("{from}   {text}"))
        } else {
            (ChatLineKind::Say, format!("{from}   {text}"))
        };
        self.push_line(channel, kind, line);
    }

    /// Someone joined. Returns `true` when it was US joining a channel the tab
    /// strip did not know yet — the host should ask the wire for NAMES.
    pub fn joined(&mut self, nick: &str, channel: &str) -> bool {
        self.roster_add(channel, nick);
        self.push_line(
            channel,
            ChatLineKind::Joined,
            format!("◈ {nick} {}", strings::resolve("$chat_joined")),
        );
        if nick == self.nick {
            let new = !self.channels.iter().any(|c| c == channel);
            if new {
                self.channels.push(channel.to_string());
            }
            self.active = channel.to_string();
            self.scroll = f32::MAX;
            new
        } else {
            false
        }
    }

    pub fn parted(&mut self, nick: &str, channel: &str) {
        self.roster_remove(channel, nick);
        self.push_line(
            channel,
            ChatLineKind::Left,
            format!("◌ {nick} {}", strings::resolve("$chat_left")),
        );
        if nick == self.nick {
            self.channels.retain(|c| c != channel);
            self.logs.remove(channel);
            self.rosters.remove(channel);
            if self.active == channel {
                self.active = self.channels.first().cloned().unwrap_or_default();
                self.scroll = f32::MAX;
            }
        }
    }

    pub fn renamed(&mut self, old: &str, new: &str) {
        if old == self.nick {
            self.nick = new.to_string();
        }
        let you = new == self.nick;
        let mut touched: Vec<String> = Vec::new();
        for (channel, roster) in self.rosters.iter_mut() {
            let mut hit = false;
            for member in roster.iter_mut() {
                if member.label == old {
                    member.label = new.to_string();
                    member.you = you;
                    hit = true;
                }
            }
            if hit {
                touched.push(channel.clone());
            }
        }
        for channel in touched {
            self.push_line(
                &channel,
                ChatLineKind::Renamed,
                format!("ᛥ {old} {} {new}", strings::resolve("$chat_is_now")),
            );
        }
    }

    pub fn names(&mut self, channel: &str, names: Vec<String>) {
        let roster = names
            .into_iter()
            .map(|label| RosterEntry {
                you: label == self.nick,
                op: false,
                label,
            })
            .collect();
        self.rosters.insert(channel.to_string(), roster);
    }

    pub fn nick_ack(&mut self, nick: &str) {
        self.nick = nick.to_string();
        let active = self.active.clone();
        self.push_line(
            &active,
            ChatLineKind::Joined,
            format!("· {} '{nick}'", strings::resolve("$chat_you_are_now")),
        );
    }

    pub fn notice(&mut self, text: &str) {
        let active = self.active.clone();
        self.push_line(&active, ChatLineKind::Left, format!("· {text}"));
    }

    pub fn error(&mut self, text: &str) {
        let active = self.active.clone();
        self.push_line(&active, ChatLineKind::Op, format!("⚠ {text}"));
    }

    // ── internals ────────────────────────────────────────────────────────────

    /// Append a line to a channel's scrollback (ring-capped), auto-following
    /// the active channel to newest.
    fn push_line(&mut self, channel: &str, kind: ChatLineKind, text: String) {
        let log = self.logs.entry(channel.to_string()).or_default();
        log.push(ChatLineView { kind, text });
        if log.len() > LOG_CAP {
            let drop = log.len() - LOG_CAP;
            log.drain(0..drop);
        }
        if channel == self.active {
            self.scroll = f32::MAX;
        }
    }

    fn roster_add(&mut self, channel: &str, nick: &str) {
        let you = nick == self.nick;
        let roster = self.rosters.entry(channel.to_string()).or_default();
        if !roster.iter().any(|m| m.label == nick) {
            roster.push(RosterEntry {
                label: nick.to_string(),
                op: false,
                you,
            });
        }
    }

    fn roster_remove(&mut self, channel: &str, nick: &str) {
        if let Some(roster) = self.rosters.get_mut(channel) {
            roster.retain(|m| m.label != nick);
        }
    }
}

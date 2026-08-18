//! `chat_panel` — the in-world comms window, built as a plain Rust `UiNode` tree.
//!
//! Unlike a registered `template`, this is a bare builder the SCENE calls every
//! frame with a live rect + the current channels/log/roster, then runs through a
//! second `run_ui` pass layered over the HUD. That per-frame rebuild is what lets
//! the window MOVE and RESIZE (walker geometry is otherwise static and the tree is
//! cached), and it sidesteps the "dynamic list on a scalar model" problem — the
//! log lines are baked straight into `text` children each frame.
//!
//! Every colour rides a dotted `style`/`color` path into the scene's `chat` style
//! block (prefix e.g. `pocclusters.chat`); no colour crosses here, so the single
//! palette stays the one source. Mirrors the DesignSync `ChatPanel.d.ts` contract
//! (MCP `36F7946B`) against the clay-chat v0.1 protocol (MCP `D9FE6CE0`).

use flicker_script::{UiAnchor, UiNode, Value};

// ── layout constants (device px) ──
const PAD: f32 = 6.0;
const GAP: f32 = 6.0;
const TITLE_H: f32 = 30.0;
const TAB_H: f32 = 30.0;
const INPUT_H: f32 = 40.0;
const ROSTER_W: f32 = 136.0;
const ROSTER_ROW_H: f32 = 18.0;
const LINE_H: f32 = 19.0;
const LINE_SIZE: f64 = 14.0;
const BTN: f32 = 34.0;
const SEND_W: f32 = 62.0;
/// The corner resize-handle box size. The scene hit-tests a rect this size at the
/// window's bottom-right corner; the builder only DRAWS the handle glyph.
pub const CORNER: f32 = 18.0;

/// The seven visually-distinct log line kinds (DesignSync `ChatLineKind`). The
/// scene assigns the kind (e.g. `You` vs `Say` by comparing the sender to the
/// local nick) — the wire never carries it.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum ChatLineKind {
    Say,
    You,
    Op,
    Emote,
    Joined,
    Left,
    Renamed,
}

impl ChatLineKind {
    fn style_leaf(self) -> &'static str {
        match self {
            ChatLineKind::Say => "line_say",
            ChatLineKind::You => "line_you",
            ChatLineKind::Op => "line_op",
            ChatLineKind::Emote => "line_emote",
            ChatLineKind::Joined => "line_joined",
            ChatLineKind::Left => "line_left",
            ChatLineKind::Renamed => "line_renamed",
        }
    }
    fn italic(self) -> bool {
        matches!(self, ChatLineKind::Emote)
    }
}

/// One rendered log line — `text` is the fully-formatted display string (the
/// scene decides "from  text" vs a system phrasing); `kind` picks the colour.
pub struct ChatLineView {
    pub kind: ChatLineKind,
    pub text: String,
}

/// One PRESENT-roster row.
pub struct RosterEntry {
    pub label: String,
    pub op: bool,
    pub you: bool,
}

/// Everything the builder needs for one frame, borrowed from scene-owned state.
pub struct ChatView<'a> {
    /// Dotted style-block PREFIX, e.g. `"pocclusters.chat"`.
    pub style: &'a str,
    /// The active channel (wire form, e.g. `"#general"`) — titlebar caption + tab selection.
    /// The `chat_tab` bind carries its INDEX in [`channels`](Self::channels), not its name.
    pub active: &'a str,
    /// Joined channels (wire form); one tab each, and the tab's `value` is its index here.
    pub channels: &'a [String],
    /// The visible log tail, oldest-first.
    pub lines: &'a [ChatLineView],
    /// PRESENT roster for the active channel.
    pub roster: &'a [RosterEntry],
    /// The local player's nick (speaker chip).
    pub nick: &'a str,
}

/// Build the floating chat window at `(x, y)` sized `w`×`h`. Returns a
/// screen-filling root `stack` so the frame floats at `(x, y)`; the scene runs
/// this through its own `run_ui` pass and lays the returned commands over the HUD.
pub fn chat_panel(x: f32, y: f32, w: f32, h: f32, view: &ChatView) -> UiNode {
    let sty = |leaf: &str| format!("{}.{}", view.style, leaf);

    // ── title bar (also the drag-to-move grip) ──
    let mut title = text_node(
        &format!("⬥  {}", display_channel(view.active)),
        &sty("title_color"),
        16.0,
    );
    set_txt(&mut title, "font", "display");
    title.grow = Some(1.0);
    let mut titlebar = styled("row", &sty("titlebar"));
    titlebar.height = Some(TITLE_H);
    titlebar.pad_x = Some(PAD + 4.0);
    titlebar.pad_y = Some(4.0);
    titlebar.children = vec![title];

    // ── tab strip: channel tabs (grow) · join(＋) · leave(✕) ──
    let mut tabs = styled("tabs", &sty("tabstrip"));
    tabs.bind = Some("chat_tab".to_string());
    set_txt(&mut tabs, "tab_active", &sty("tab_active"));
    set_txt(&mut tabs, "tab_idle", &sty("tab_idle"));
    tabs.grow = Some(1.0);
    tabs.children = view
        .channels
        .iter()
        .enumerate()
        .map(|(i, c)| tab_child(i, &display_channel(c)))
        .collect();
    let mut tabrow = elem("row");
    tabrow.height = Some(TAB_H);
    tabrow.gap = 4.0;
    tabrow.children = vec![
        tabs,
        button(&sty("btn"), "chat_join", "＋", BTN),
        button(&sty("btn_danger"), "chat_part", "✕", BTN),
    ];

    // ── body: scrolling log (grow) | roster rail (fixed) ──
    let mut scroll = elem("list");
    scroll.bind = Some("chat_scroll".to_string());
    scroll.grow = Some(1.0);
    scroll.gap = 2.0;
    scroll.children = view
        .lines
        .iter()
        .map(|l| line_node(l, &sty(l.kind.style_leaf())))
        .collect();
    let mut logwell = styled("cell", &sty("log"));
    logwell.grow = Some(1.0);
    logwell.pad = 8.0;
    logwell.children = vec![scroll];

    let mut roster = styled("cell", &sty("roster"));
    roster.size = Some(ROSTER_W); // fixed width as a child of the body row
    roster.pad = 8.0;
    roster.gap = 2.0;
    roster.children = {
        let mut rows = Vec::with_capacity(view.roster.len() + 1);
        let mut head = text_node(
            &format!("PRESENT · {}", view.roster.len()),
            &sty("roster_head"),
            11.0,
        );
        set_txt(&mut head, "font", "label");
        head.height = Some(ROSTER_ROW_H);
        rows.push(head);
        rows.extend(view.roster.iter().map(|m| roster_node(m, view.style)));
        rows
    };

    let mut body = elem("row");
    body.grow = Some(1.0);
    body.gap = GAP;
    body.children = vec![logwell, roster];

    // ── input row: speaker chip · text field (grow) · send ──
    let mut speaker = text_node(&format!("You · {}", view.nick), &sty("speaker_color"), 13.0);
    speaker.size = Some(speaker_w(view.nick));
    let mut field = styled("text_field", &sty("input"));
    field.id = "chat_input".to_string();
    field.bind = Some("chat_input".to_string());
    set_txt(
        &mut field,
        "placeholder",
        &format!("Speak in ⬥ {}…", display_channel(view.active)),
    );
    field.grow = Some(1.0);
    let mut inputrow = styled("row", &sty("input_bar"));
    inputrow.height = Some(INPUT_H);
    inputrow.pad_x = Some(PAD);
    inputrow.pad_y = Some(4.0);
    inputrow.gap = GAP;
    inputrow.children = vec![
        speaker,
        field,
        button(&sty("btn"), "chat_send", "Send", SEND_W),
    ];

    // ── chrome column (fills the frame) ──
    let mut col = elem("cell");
    col.anchor = Some(UiAnchor::TopLeft);
    set_num(&mut col, "width_frac", 1.0);
    set_num(&mut col, "height_frac", 1.0);
    col.pad = PAD;
    col.gap = GAP;
    col.children = vec![titlebar, tabrow, body, inputrow];

    // ── corner resize handle (bottom-right; scene hit-tests the rect) ──
    let mut handle = text_node("◢", &sty("corner"), 14.0);
    handle.anchor = Some(UiAnchor::BottomRight);
    handle.width = Some(CORNER);
    handle.height = Some(CORNER);
    set_txt(&mut handle, "align", "right");

    // ── window frame: styled stack overlaying the column + the handle ──
    let mut frame = styled("stack", &sty("window"));
    frame.anchor = Some(UiAnchor::TopLeft);
    frame.offset = [x, y];
    frame.width = Some(w);
    frame.height = Some(h);
    frame.children = vec![col, handle];

    // ── screen-filling root so the frame floats at (x, y) ──
    let mut root = elem("stack");
    root.anchor = Some(UiAnchor::TopLeft);
    set_num(&mut root, "width_frac", 1.0);
    set_num(&mut root, "height_frac", 1.0);
    root.children = vec![frame];
    root
}

// ── builder helpers ──────────────────────────────────────────────────────────

fn elem(kind: &str) -> UiNode {
    UiNode {
        component: kind.to_string(),
        ..Default::default()
    }
}

fn styled(kind: &str, style: &str) -> UiNode {
    let mut n = elem(kind);
    set_txt(&mut n, "style", style);
    n
}

fn text_node(text: &str, color_path: &str, size: f64) -> UiNode {
    let mut n = elem("text");
    set_txt(&mut n, "text", text);
    set_txt(&mut n, "color", color_path);
    set_num(&mut n, "text_size", size);
    n
}

fn set_txt(n: &mut UiNode, key: &str, v: &str) {
    n.props.insert(key.to_string(), Value::Text(v.to_string()));
}

fn set_num(n: &mut UiNode, key: &str, v: f64) {
    n.props.insert(key.to_string(), Value::Number(v));
}

fn button(style: &str, action: &str, label: &str, w: f32) -> UiNode {
    let mut b = styled("button", style);
    set_txt(&mut b, "label", label);
    b.action = Some(action.to_string());
    b.size = Some(w); // main-axis (width) in the row
    b
}

/// One channel tab: its `value` is the channel's INDEX in `ChatView::channels` (an
/// index is a number, everywhere), and the channel NAME is what the tab shows.
fn tab_child(i: usize, label: &str) -> UiNode {
    let mut n = elem("cell");
    set_num(&mut n, "value", i as f64);
    set_txt(&mut n, "label", label);
    n
}

fn line_node(l: &ChatLineView, color_path: &str) -> UiNode {
    let mut n = text_node(&l.text, color_path, LINE_SIZE);
    if l.kind.italic() {
        n.props.insert("italic".to_string(), Value::Bool(true));
    }
    n.height = Some(LINE_H); // so scroll_content_h sums the log correctly
    n
}

fn roster_node(m: &RosterEntry, prefix: &str) -> UiNode {
    let leaf = if m.you {
        "roster_you"
    } else if m.op {
        "roster_op"
    } else {
        "roster_here"
    };
    let label = if m.op {
        format!("• {} ᛟ", m.label)
    } else {
        format!("• {}", m.label)
    };
    let mut n = text_node(&label, &format!("{prefix}.{leaf}"), 13.0);
    n.height = Some(ROSTER_ROW_H);
    n
}

/// Strip the wire `#` for display (`"#general"` → `"general"`).
fn display_channel(ch: &str) -> String {
    ch.strip_prefix('#').unwrap_or(ch).to_string()
}

/// Rough fixed width for the speaker chip — the walker has no glyph metrics, so
/// estimate from `"You · <nick>"` length and clamp.
fn speaker_w(nick: &str) -> f32 {
    let chars = 6 + nick.chars().count();
    (chars as f32 * 8.0).clamp(90.0, 200.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn find(n: &UiNode, pred: impl Fn(&UiNode) -> bool + Copy) -> Option<&UiNode> {
        if pred(n) {
            return Some(n);
        }
        n.children.iter().find_map(|c| find(c, pred))
    }

    fn collect(n: &UiNode, pred: impl Fn(&UiNode) -> bool + Copy) -> Vec<&UiNode> {
        let mut out = Vec::new();
        if pred(n) {
            out.push(n);
        }
        for c in &n.children {
            out.extend(collect(c, pred));
        }
        out
    }

    #[test]
    fn builds_expected_tree_shape() {
        let channels = vec!["#general".to_string(), "#trade".to_string()];
        let lines = vec![
            ChatLineView {
                kind: ChatLineKind::Joined,
                text: "◈ ann joined".into(),
            },
            ChatLineView {
                kind: ChatLineKind::You,
                text: "me  hi there".into(),
            },
            ChatLineView {
                kind: ChatLineKind::Emote,
                text: "✦ ann waves".into(),
            },
        ];
        let roster = vec![
            RosterEntry {
                label: "ann".into(),
                op: true,
                you: false,
            },
            RosterEntry {
                label: "me".into(),
                op: false,
                you: true,
            },
        ];
        let view = ChatView {
            style: "pocclusters.chat",
            active: "#general",
            channels: &channels,
            lines: &lines,
            roster: &roster,
            nick: "me",
        };
        let root = chat_panel(40.0, 500.0, 900.0, 300.0, &view);

        assert_eq!(root.component, "stack");
        let frame = &root.children[0];
        assert_eq!(frame.component, "stack");
        assert_eq!(frame.offset, [40.0, 500.0]);
        assert_eq!(frame.width, Some(900.0));
        assert_eq!(frame.height, Some(300.0));
        // frame overlays the chrome column + the corner handle.
        assert_eq!(frame.children.len(), 2);
        let col = &frame.children[0];
        assert_eq!(col.component, "cell");
        assert_eq!(col.children.len(), 4); // titlebar, tabrow, body, input

        // The text field is focusable by a stable id and two-way bound.
        let field = find(&root, |n| n.component == "text_field").expect("a text_field");
        assert_eq!(field.id, "chat_input");
        assert_eq!(field.bind.as_deref(), Some("chat_input"));

        // One tab per channel; the strip reports through `chat_tab`. A tab's `value`
        // is the channel's INDEX — a number, the one representation of a selection.
        let tabs = find(&root, |n| n.component == "tabs").expect("a tabs strip");
        assert_eq!(tabs.children.len(), channels.len());
        assert_eq!(tabs.bind.as_deref(), Some("chat_tab"));
        for (i, tab) in tabs.children.iter().enumerate() {
            assert_eq!(
                tab.props.get("value"),
                Some(&Value::Number(i as f64)),
                "tab {i} carries its numeric index"
            );
        }

        // The list holds one line per view line, and follows `chat_scroll`.
        let scroll = find(&root, |n| n.component == "list").expect("a list");
        assert_eq!(scroll.children.len(), lines.len());
        assert_eq!(scroll.bind.as_deref(), Some("chat_scroll"));
        // The emote line renders italic.
        assert!(scroll
            .children
            .iter()
            .any(|n| matches!(n.props.get("italic"), Some(Value::Bool(true)))));

        // The three command affordances carry their actions.
        let actions: Vec<&str> = collect(&root, |n| n.component == "button")
            .iter()
            .filter_map(|b| b.action.as_deref())
            .collect();
        assert!(actions.contains(&"chat_send"));
        assert!(actions.contains(&"chat_join"));
        assert!(actions.contains(&"chat_part"));
    }

    #[test]
    fn runs_through_the_walker_without_panicking() {
        use crate::{run_ui, UiInput, UiState};
        use flicker_render::Vec2;
        use flicker_script::ValueMap;

        let channels = vec!["#general".to_string(), "#trade".to_string()];
        let lines = vec![
            ChatLineView {
                kind: ChatLineKind::Joined,
                text: "◈ ann joined".into(),
            },
            ChatLineView {
                kind: ChatLineKind::Say,
                text: "ann   hello".into(),
            },
            ChatLineView {
                kind: ChatLineKind::Emote,
                text: "✦ ann waves".into(),
            },
        ];
        let roster = vec![RosterEntry {
            label: "ann".into(),
            op: true,
            you: false,
        }];
        let view = ChatView {
            style: "pocclusters.chat",
            active: "#general",
            channels: &channels,
            lines: &lines,
            roster: &roster,
            nick: "me",
        };
        let tree = chat_panel(20.0, 400.0, 820.0, 260.0, &view);

        // Empty styles: every colour falls back to a default, but layout + draw still
        // run — so this exercises the full resolve/hit/draw path over the chat tree
        // (the floating frame, the scroll clip, tabs, the text_field) and would panic
        // on a geometry/clip bug a shape-only test can't reach.
        let styles = serde_json::json!({});
        let model = ValueMap::new();
        let input = UiInput {
            mouse: Vec2::new(0.0, 0.0),
            clicked: false,
            down: false,
            screen: Vec2::new(1280.0, 720.0),
            typed: String::new(),
            backspace: false,
            wheel: 0.0,
        };
        let mut state = UiState::new();
        let frame = run_ui(&tree, &model, &styles, &input, &mut state);
        assert!(
            !frame.commands.is_empty(),
            "the chat panel should emit draw commands"
        );
    }

    /// **A channel tab reports its INDEX.** The strip's selection channel is a number
    /// end to end — the tab's `value`, the `chat_tab` bind, and the scene's read of it
    /// (`pocclusters` maps the index back to the channel name).
    #[test]
    fn a_tab_click_reports_the_channel_index() {
        use crate::{run_ui, UiInput, UiState};
        use flicker_render::Vec2;
        use flicker_script::ValueMap;

        let channels = vec!["#general".to_string(), "#trade".to_string()];
        let view = ChatView {
            style: "pocclusters.chat",
            active: "#general",
            channels: &channels,
            lines: &[],
            roster: &[],
            nick: "me",
        };
        // Frame at (20,400) 820×260, column pad 6 / gap 6: titlebar 406..436, then the
        // tab row at 442..472. The strip grows to 808 − (34 + 34 + 4 + 4) = 732 wide
        // from x 26, so its two cells split at x 392 — click the second one.
        let tree = chat_panel(20.0, 400.0, 820.0, 260.0, &view);
        let click = UiInput {
            mouse: Vec2::new(500.0, 457.0),
            clicked: true,
            down: true,
            screen: Vec2::new(1280.0, 720.0),
            typed: String::new(),
            backspace: false,
            wheel: 0.0,
        };
        let model = ValueMap::new().with("chat_tab", 0.0);
        let f = run_ui(
            &tree,
            &model,
            &serde_json::json!({}),
            &click,
            &mut UiState::new(),
        );
        assert_eq!(
            f.results.number("chat_tab"),
            Some(1.0),
            "the second tab selects index 1"
        );
        assert_eq!(
            f.results.text("chat_tab"),
            None,
            "a tab selection is never text"
        );
    }
}

//! Transport types for the clay-chat client.
//!
//! These mirror the DesignSync `ChatPanel.d.ts` `ClayCommand` and the inbound
//! server lines of the clay-chat v0 wire protocol (MCP `D9FE6CE0`). They are
//! deliberately *transport only* — the UI render types (line kind, roster
//! member) live with the renderer in `flicker-widgets`, because whether a line
//! is "you" vs "say" and who is an operator is a client/scene decision, not
//! something the wire carries.

/// A command the client sends to the server — one wire line each (see `codec`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ChatCommand {
    /// Join (or auto-create) a channel — `JOIN <channel>`.
    Join(String),
    /// Leave a channel — `PART <channel>`.
    Part(String),
    /// Set/change your nick — `NICK <name>`.
    Nick(String),
    /// Send a message to a channel — `MSG <channel> <text>`.
    Msg { channel: String, text: String },
    /// Request a channel roster — `NAMES <channel>`.
    Names(String),
    /// Disconnect — `QUIT`.
    Quit,
}

/// An event the client surfaces to the game loop. Most are decoded from one
/// inbound server line; [`Connected`](Self::Connected)/
/// [`Disconnected`](Self::Disconnected) are synthesized by the client for the
/// net-level transitions the wire protocol has no line for.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ChatEvent {
    /// The TCP connection was established (client-synthesized).
    Connected,
    /// The connection closed (client-synthesized); optional reason.
    Disconnected(Option<String>),
    /// A chat message — wire `#chan <from> text`.
    Chat {
        channel: String,
        from: String,
        text: String,
    },
    /// Keepalive reply — wire `PONG` / `PONG <token>`.
    Pong(Option<String>),
    /// An error notice — wire `! <text>`.
    Error(String),
    /// A member joined — wire `* <nick> joined <#chan>`.
    Joined { nick: String, channel: String },
    /// A member left — wire `* <nick> left <#chan>`.
    Parted { nick: String, channel: String },
    /// A member renamed — wire `* <old> is now known as <new>` (the clay-chatd
    /// broadcast patch; the local renamer also gets [`NickAck`](Self::NickAck)).
    Renamed { old: String, new: String },
    /// A channel roster — wire `* <#chan> names: <n1 n2 …>`.
    Names { channel: String, names: Vec<String> },
    /// The channel list — wire `* channels: <#a(2) #b(3) …>` (counts dropped).
    Channels(Vec<String>),
    /// Your own nick was accepted — wire `* you are now '<n>'`.
    NickAck(String),
    /// Any other server notice (welcome / commands / bye / help / …).
    Notice(String),
}

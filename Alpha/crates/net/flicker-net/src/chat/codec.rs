//! Pure encode/decode for the clay-chat v0 line protocol (MCP `D9FE6CE0`).
//!
//! No tokio, no I/O — just `&str` ⇆ [`ChatCommand`]/[`ChatEvent`], so the wire
//! contract is exhaustively unit-testable. Keep this isolated: when the binary
//! CLAY/1 protocol replaces the POC line protocol, only this module changes.
//!
//! Decoding matches **most-specific-first**, because every system line shares
//! the `* ` prefix; ambiguous `* …` lines are disambiguated by anchoring on the
//! trailing `#channel` + keyword and falling through to [`ChatEvent::Notice`].

use super::types::{ChatCommand, ChatEvent};

/// A client command → one wire line (terminated `\r\n`).
pub fn encode(cmd: &ChatCommand) -> String {
    match cmd {
        ChatCommand::Join(ch) => format!("JOIN {}\r\n", norm_channel(ch)),
        ChatCommand::Part(ch) => format!("PART {}\r\n", norm_channel(ch)),
        ChatCommand::Nick(name) => format!("NICK {}\r\n", sanitize_nick(name)),
        ChatCommand::Msg { channel, text } => {
            format!("MSG {} {}\r\n", norm_channel(channel), text)
        }
        ChatCommand::Names(ch) => format!("NAMES {}\r\n", norm_channel(ch)),
        ChatCommand::Quit => "QUIT\r\n".to_string(),
    }
}

/// A single inbound wire line → a [`ChatEvent`]. The line may or may not still
/// carry its `\r\n`; both are stripped.
pub fn decode(line: &str) -> ChatEvent {
    let line = line.trim_end_matches(['\r', '\n']);

    // 1) Chat — the only line that starts with `#`: `#chan <from> text`.
    if let Some(ev) = parse_chat(line) {
        return ev;
    }
    // 2) Pong.
    if line == "PONG" {
        return ChatEvent::Pong(None);
    }
    if let Some(tok) = line.strip_prefix("PONG ") {
        return ChatEvent::Pong(Some(tok.to_string()));
    }
    // 3) Error.
    if let Some(rest) = line.strip_prefix("! ") {
        return ChatEvent::Error(rest.to_string());
    }
    // 4) System notices — all share the `* ` prefix; sub-match most-specific first.
    if let Some(rest) = line.strip_prefix("* ") {
        return parse_star(rest);
    }
    // Anything else: a bare, unprefixed line — treat as a notice.
    ChatEvent::Notice(line.to_string())
}

// ── decode helpers ──────────────────────────────────────────────────────────

/// `#chan <from> text` → [`ChatEvent::Chat`].
fn parse_chat(line: &str) -> Option<ChatEvent> {
    if !line.starts_with('#') {
        return None;
    }
    let (channel, rest) = line.split_once(' ')?;
    let rest = rest.strip_prefix('<')?;
    let (from, rest) = rest.split_once('>')?;
    let text = rest.strip_prefix(' ').unwrap_or(rest);
    if from.is_empty() {
        return None;
    }
    Some(ChatEvent::Chat {
        channel: channel.to_string(),
        from: from.to_string(),
        text: text.to_string(),
    })
}

/// The remainder after the `* ` prefix.
fn parse_star(rest: &str) -> ChatEvent {
    // `<nick> joined <#chan>` / `<nick> left <#chan>`.
    if let Some((nick, chan)) = rest.split_once(" joined ") {
        if !nick.is_empty() && is_channel(chan) {
            return ChatEvent::Joined {
                nick: nick.to_string(),
                channel: chan.to_string(),
            };
        }
    }
    if let Some((nick, chan)) = rest.split_once(" left ") {
        if !nick.is_empty() && is_channel(chan) {
            return ChatEvent::Parted {
                nick: nick.to_string(),
                channel: chan.to_string(),
            };
        }
    }
    // `<old> is now known as <new>` (nicks carry no spaces).
    if let Some((old, new)) = rest.split_once(" is now known as ") {
        if !old.is_empty() && !new.is_empty() && !old.contains(' ') && !new.contains(' ') {
            return ChatEvent::Renamed {
                old: old.to_string(),
                new: new.to_string(),
            };
        }
    }
    // `<#chan> names: <n1 n2 …>`.
    if let Some((chan, names)) = rest.split_once(" names: ") {
        if is_channel(chan) {
            return ChatEvent::Names {
                channel: chan.to_string(),
                names: names.split_whitespace().map(str::to_string).collect(),
            };
        }
    }
    // `channels: <#a(2) #b(3) …>` (empty list = `channels: ` → no tokens).
    if let Some(list) = rest.strip_prefix("channels: ") {
        return ChatEvent::Channels(parse_channel_list(list));
    }
    // `you are now '<n>'`.
    if let Some(inner) = rest.strip_prefix("you are now '") {
        if let Some(name) = inner.strip_suffix('\'') {
            return ChatEvent::NickAck(name.to_string());
        }
    }
    // Welcome / commands / bye / help / anything else.
    ChatEvent::Notice(rest.to_string())
}

/// `#a(2) #b(3)` → `["#a", "#b"]` (drop the `(count)` suffix).
fn parse_channel_list(list: &str) -> Vec<String> {
    list.split_whitespace()
        .map(|tok| {
            tok.split_once('(')
                .map_or(tok, |(name, _)| name)
                .to_string()
        })
        .collect()
}

fn is_channel(s: &str) -> bool {
    s.starts_with('#') && s.len() > 1 && !s.contains(char::is_whitespace)
}

// ── encode helpers ──────────────────────────────────────────────────────────

/// Ensure a leading `#` (the server would add one, but keeping our own state
/// canonical avoids `general` vs `#general` mismatches).
fn norm_channel(ch: &str) -> String {
    let ch = ch.trim();
    if ch.starts_with('#') {
        ch.to_string()
    } else {
        format!("#{ch}")
    }
}

/// Mirror the server's nick sanitize: strip all whitespace and `#`, truncate to
/// 24 chars. (The server also does this; the authoritative nick comes back as a
/// `NickAck`, but sanitizing here keeps the optimistic send predictable.)
fn sanitize_nick(name: &str) -> String {
    name.chars()
        .filter(|c| !c.is_whitespace() && *c != '#')
        .take(24)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn encode_exact_wire_strings() {
        assert_eq!(
            encode(&ChatCommand::Msg {
                channel: "#dev".into(),
                text: "hello there".into()
            }),
            "MSG #dev hello there\r\n"
        );
        assert_eq!(encode(&ChatCommand::Join("dev".into())), "JOIN #dev\r\n");
        assert_eq!(encode(&ChatCommand::Join("#dev".into())), "JOIN #dev\r\n");
        assert_eq!(encode(&ChatCommand::Part("#dev".into())), "PART #dev\r\n");
        assert_eq!(encode(&ChatCommand::Names("#dev".into())), "NAMES #dev\r\n");
        assert_eq!(encode(&ChatCommand::Quit), "QUIT\r\n");
    }

    #[test]
    fn encode_sanitizes_nick_like_the_server() {
        assert_eq!(encode(&ChatCommand::Nick("Foo#1".into())), "NICK Foo1\r\n");
        assert_eq!(encode(&ChatCommand::Nick("  a b ".into())), "NICK ab\r\n");
        let long = "a".repeat(40);
        assert_eq!(
            encode(&ChatCommand::Nick(long)),
            format!("NICK {}\r\n", "a".repeat(24))
        );
    }

    #[test]
    fn decode_chat_line() {
        assert_eq!(
            decode("#dev <ann> hi there\r\n"),
            ChatEvent::Chat {
                channel: "#dev".into(),
                from: "ann".into(),
                text: "hi there".into()
            }
        );
        // A message body may itself contain angle brackets — only the first
        // `<from>` is the speaker.
        assert_eq!(
            decode("#dev <ann> 1 < 2 > 0"),
            ChatEvent::Chat {
                channel: "#dev".into(),
                from: "ann".into(),
                text: "1 < 2 > 0".into()
            }
        );
    }

    #[test]
    fn decode_system_lines_most_specific_first() {
        assert_eq!(
            decode("* ann joined #dev"),
            ChatEvent::Joined {
                nick: "ann".into(),
                channel: "#dev".into()
            }
        );
        assert_eq!(
            decode("* ann left #dev"),
            ChatEvent::Parted {
                nick: "ann".into(),
                channel: "#dev".into()
            }
        );
        assert_eq!(
            decode("* ann is now known as bob"),
            ChatEvent::Renamed {
                old: "ann".into(),
                new: "bob".into()
            }
        );
        assert_eq!(
            decode("* #dev names: ann bob cid"),
            ChatEvent::Names {
                channel: "#dev".into(),
                names: vec!["ann".into(), "bob".into(), "cid".into()]
            }
        );
        assert_eq!(
            decode("* channels: #dev(2) #general(5)"),
            ChatEvent::Channels(vec!["#dev".into(), "#general".into()])
        );
        assert_eq!(decode("* channels: "), ChatEvent::Channels(vec![]));
        assert_eq!(
            decode("* you are now 'bob'"),
            ChatEvent::NickAck("bob".into())
        );
        assert_eq!(decode("PONG"), ChatEvent::Pong(None));
        assert_eq!(decode("PONG 42"), ChatEvent::Pong(Some("42".into())));
        assert_eq!(
            decode("! you are not in #dev"),
            ChatEvent::Error("you are not in #dev".into())
        );
    }

    #[test]
    fn decode_ambiguous_notices_fall_through() {
        // Contains "joined"/"left"/"is now known as" but no trailing #channel or
        // a multi-word tail → stays a Notice, never a false Joined/Parted/Renamed.
        assert_eq!(
            decode("* welcome — many have joined the fold"),
            ChatEvent::Notice("welcome — many have joined the fold".into())
        );
        assert_eq!(
            decode("* the door is now known as broken"),
            ChatEvent::Notice("the door is now known as broken".into())
        );
        assert_eq!(
            decode("* commands: NICK · JOIN #chan · MSG #chan <text>"),
            ChatEvent::Notice("commands: NICK · JOIN #chan · MSG #chan <text>".into())
        );
    }

    #[test]
    fn decode_round_trips_a_sent_message_echo() {
        // What we send as MSG comes back as a Chat echo from the server.
        let sent = encode(&ChatCommand::Msg {
            channel: "#dev".into(),
            text: "hi".into(),
        });
        assert_eq!(sent, "MSG #dev hi\r\n");
        assert_eq!(
            decode("#dev <me> hi"),
            ChatEvent::Chat {
                channel: "#dev".into(),
                from: "me".into(),
                text: "hi".into()
            }
        );
    }
}

//! clay-chat client — connects flicker to the clay-engine `clay-chatd` server
//! over the v0 plain-TCP line protocol (MCP `D9FE6CE0`).
//!
//! Tokio is confined to this module: [`ChatClient::connect`] spawns ONE OS
//! thread that owns a current-thread runtime and runs a reader + writer task.
//! The game loop stays fully synchronous and talks to it over channels —
//! outbound via a tokio unbounded channel (whose `send` is non-async, so the
//! sync caller needs no runtime), inbound via a `std::sync::mpsc` the scene
//! drains with [`try_recv`](ChatClient::try_recv) each frame (the same shape as
//! the scene's existing background-mesh channel).

mod codec;
pub mod types;

pub use types::{ChatCommand, ChatEvent};

use std::sync::mpsc::{self, Receiver};
use std::thread::JoinHandle;

use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::TcpStream;
use tokio::sync::mpsc::{unbounded_channel, UnboundedReceiver, UnboundedSender};

/// The clay-chatd address. Hard-coded for the v0 raw-protocol test — host
/// discovery / auth / registration arrive later from the web side.
const SERVER_ADDR: &str = "127.0.0.1:6667";

/// A live (or connecting) clay-chat session. Own one per scene; drop it to
/// disconnect. All methods are non-blocking and safe to call from the game loop.
pub struct ChatClient {
    /// Outbound commands → the writer task. `send` is non-async.
    tx: UnboundedSender<ChatCommand>,
    /// Inbound events ← the reader task. Drain with [`try_recv`](Self::try_recv).
    rx: Receiver<ChatEvent>,
    /// The runtime thread. Held (not joined) so it lives as long as the client;
    /// dropping the client drops `tx`, which lets the thread wind down on its own.
    _thread: JoinHandle<()>,
}

impl ChatClient {
    /// Connect to clay-chatd and set `nick`. Returns immediately — the socket
    /// opens on the background thread; watch for [`ChatEvent::Connected`] /
    /// [`ChatEvent::Disconnected`] via [`try_recv`](Self::try_recv).
    pub fn connect(nick: String) -> ChatClient {
        let (out_tx, out_rx) = unbounded_channel::<ChatCommand>();
        let (in_tx, in_rx) = mpsc::channel::<ChatEvent>();

        let thread = std::thread::Builder::new()
            .name("clay-chat".into())
            .spawn(move || {
                let rt = match tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build()
                {
                    Ok(rt) => rt,
                    Err(e) => {
                        let _ = in_tx.send(ChatEvent::Disconnected(Some(format!("runtime: {e}"))));
                        return;
                    }
                };
                rt.block_on(driver(nick, out_rx, in_tx));
            })
            .expect("spawn clay-chat runtime thread");

        ChatClient {
            tx: out_tx,
            rx: in_rx,
            _thread: thread,
        }
    }

    /// Queue a command to the server. Non-blocking; a no-op (logged) if the
    /// connection has already gone.
    pub fn send(&self, cmd: ChatCommand) {
        if self.tx.send(cmd).is_err() {
            tracing::debug!("clay-chat: send on a closed channel (disconnected)");
        }
    }

    /// Pop the next inbound event, or `None` if none is pending. Call in a
    /// `while let Some(ev) = client.try_recv()` loop each frame.
    pub fn try_recv(&self) -> Option<ChatEvent> {
        self.rx.try_recv().ok()
    }
}

impl Drop for ChatClient {
    fn drop(&mut self) {
        // Best-effort graceful close: send QUIT, then (as the struct drops) `tx`
        // drops → the writer ends → the socket closes → the reader hits EOF →
        // the driver future returns → the runtime thread exits. We deliberately
        // do NOT join the thread — the game loop must never stall on the socket.
        let _ = self.tx.send(ChatCommand::Quit);
    }
}

/// The connection driver: connect, then run the reader and writer concurrently
/// on the current-thread runtime until either side ends.
async fn driver(
    nick: String,
    mut out_rx: UnboundedReceiver<ChatCommand>,
    in_tx: mpsc::Sender<ChatEvent>,
) {
    let stream = match TcpStream::connect(SERVER_ADDR).await {
        Ok(s) => s,
        Err(e) => {
            let _ = in_tx.send(ChatEvent::Disconnected(Some(format!("connect {SERVER_ADDR}: {e}"))));
            return;
        }
    };
    let _ = in_tx.send(ChatEvent::Connected);
    let (read_half, mut write_half) = stream.into_split();

    // Reader: wire lines → decoded events, until EOF/error or the UI drops the
    // receiver. `while let` exits on `Ok(None)` (server closed) and `Err(_)`
    // (socket error); the inner `break` handles the UI-gone case.
    let reader = async {
        let mut lines = BufReader::new(read_half).lines();
        while let Ok(Some(line)) = lines.next_line().await {
            if in_tx.send(codec::decode(&line)).is_err() {
                break; // the game/UI side dropped the receiver
            }
        }
        let _ = in_tx.send(ChatEvent::Disconnected(None));
    };

    // Writer: NICK handshake first, then drain queued commands until QUIT / close.
    let writer = async {
        if write_half
            .write_all(codec::encode(&ChatCommand::Nick(nick)).as_bytes())
            .await
            .is_err()
        {
            return;
        }
        let _ = write_half.flush().await;
        while let Some(cmd) = out_rx.recv().await {
            let quit = matches!(cmd, ChatCommand::Quit);
            if write_half
                .write_all(codec::encode(&cmd).as_bytes())
                .await
                .is_err()
            {
                break;
            }
            let _ = write_half.flush().await;
            if quit {
                break;
            }
        }
    };

    tokio::join!(reader, writer);
}

//! Reports what the game does: every event it raises, every chat line, every remote call.
//!
//! The loader hooks the engine's event broadcaster, its chat path and its RPC dispatcher;
//! this plugin subscribes to all three and writes what arrives to the loader console, to its
//! own `game-events.log`, and — for the handful worth interrupting someone for — to a toast.
//!
//! ```text
//! 1759600412.118  event  MPSessionStartEvent
//! 1759600413.902  chat   [0] <server> You have joined the server
//! 1759600421.344  rpc    kind 14001 from 0x24f1a2b0 to 0x0
//! 1759600488.010  event  RespawnEvent
//! ```
//!
//! ## What it can and cannot tell you
//!
//! Event *names* are read out of each event's own virtual table, so they are right for
//! classes nobody has looked at — see the loader's `game_events` module for how. Event
//! *contents* are not decoded: each class keeps its fields at its own offsets, and this
//! plugin logs the name and the object's address rather than inventing a layout. So
//! `MPConnectionCloseEvent` tells you that the connection closed and when, not yet why; the
//! address is there so that a future version, or a session with the `read` console command,
//! can go and look.
//!
//! Chat is the exception, because the loader catches it where the engine still has the
//! channel and the three strings as separate arguments. Those fields are real.
//!
//! ## Why everything is a pattern list
//!
//! The engine raises render and input events through the same broadcaster as the session
//! ones, so "log every event" means a line per frame. Every sink therefore takes a list of
//! name patterns rather than a boolean, and the defaults exclude the known-noisy families
//! instead of naming the interesting ones. The `filter` module holds the syntax, and the
//! `events` setting's own description repeats the short version in the settings editor.

mod filter;
mod sink;

use std::sync::atomic::{AtomicU64, Ordering};

use dayz_plugin_sdk::api::{StopReason, UiLevel};
use dayz_plugin_sdk::{
    export_plugin, Chat, ChatVerdict, Event, Host, Notice, Plugin, PluginError, Rpc, Setting,
    Streams,
};

use filter::Filter;
use sink::LogFile;

/// Events written to the console and the file by default: everything except the families the
/// engine raises per frame, per mouse movement or on a timer.
pub(crate) const DEFAULT_EVENTS: &str = "*, !AfterRender*, !BeforeReset*, !AfterReset*, \
     !Mouse*, !Key*, !Raw*, !Char*, !Gamepad*, !Joystick*, !LastInputDeviceChange*, \
     !*StatsUpdated*, !NetworkInputBuffer*, !Progress*, !LoadWorldProgress*, !Entity*, \
     !WorldEvent*, !WidgetEvent*, !ReloadShaders*, !WindowsResize*";

/// Events worth a toast: the ones that mean the session changed under you.
pub(crate) const DEFAULT_TOASTS: &str = "Connecting*, MPSession*, MPConnection*, Client*, \
     Respawn*, PlayerDeath*, Login*, Logout*, DLC*, Startup*, Shutdown*, LeaveGameplaySession*";

/// Events whose toast should be a warning rather than information.
const ALARMING: &str =
    "MPConnectionClose*, MPConnectionLost*, MPSessionFail*, ConnectingAbort*, DLC*, \
     PlayerDeath*, Logout*";

/// How many characters of a chat line reach a toast before it is cut.
const CHAT_TOAST_LIMIT: usize = 120;

struct GameLog {
    file: LogFile,
    events: AtomicU64,
    chat: AtomicU64,
    rpc: AtomicU64,
}

impl Plugin for GameLog {
    const NAME: &'static str = "gamelog";
    const VERSION: &'static str = env!("CARGO_PKG_VERSION");
    const DESCRIPTION: &'static str =
        "Reports the game's own events, chat and remote calls to the console, a log file and toasts.";

    fn start(host: Host) -> Result<Self, PluginError> {
        register_settings(&host)?;
        host.command("where", "Print where the event log is written.", "")?;
        host.command("stats", "Print how much has been seen and written.", "")?;
        host.command("flush", "Write the event log out now.", "")?;
        // Everything, filtered here rather than there: the loader's mask is per stream and
        // the interesting filtering is per event name, which only this plugin knows about.
        host.listen_game(Streams::ALL)?;
        let file = LogFile::open(&host.config_dir());
        log::info!("watching the game's events, chat and remote calls");
        Ok(GameLog {
            file,
            events: AtomicU64::new(0),
            chat: AtomicU64::new(0),
            rpc: AtomicU64::new(0),
        })
    }

    fn on_game_event(&self, host: &Host, event: &Event<'_>) {
        self.events.fetch_add(1, Ordering::Relaxed);
        if !list(host, "events").covers(event.name) {
            return;
        }
        self.emit(
            host,
            "event",
            &format!("{} at {:p}", event.name, event.object),
        );
        let toasts = list(host, "toasts");
        if toasts.covers(event.name) {
            let level = if Filter::parse(ALARMING).covers(event.name) {
                UiLevel::Warning
            } else {
                UiLevel::Info
            };
            let _ = host.show(&Notice::toast(event.name).title("Game event").level(level));
        }
    }

    fn on_chat(&self, host: &Host, chat: &Chat<'_>) -> ChatVerdict {
        self.chat.fetch_add(1, Ordering::Relaxed);
        if host.get_as::<bool>("chat").unwrap_or(true) {
            let from = if chat.from.is_empty() {
                String::new()
            } else {
                format!("<{}> ", chat.from)
            };
            self.emit(
                host,
                "chat",
                &format!("[{}] {from}{}", chat.channel, chat.text),
            );
            if host.get_as::<bool>("chat_toasts").unwrap_or(false) {
                let text: String = chat.text.chars().take(CHAT_TOAST_LIMIT).collect();
                let title = if chat.from.is_empty() {
                    "Chat"
                } else {
                    chat.from
                };
                let _ = host.show(&Notice::toast(&text).title(title));
            }
        }
        // This plugin reports; it never takes a line off the screen. Swallowing is in the ABI
        // for a plugin whose job is to filter chat, and a logger that could silently eat a
        // message would be the wrong thing to have running all the time.
        ChatVerdict::PASS
    }

    fn on_rpc(&self, host: &Host, call: &Rpc) {
        self.rpc.fetch_add(1, Ordering::Relaxed);
        if !host.get_as::<bool>("rpc").unwrap_or(true) {
            return;
        }
        self.emit(
            host,
            "rpc",
            &format!(
                "kind {} from {:p} to {:p} params {:p}",
                call.kind, call.sender, call.target, call.params
            ),
        );
    }

    fn on_command(&self, host: &Host, name: &str, _args: &str) -> Result<(), PluginError> {
        match name {
            "where" => {
                host.console_print(&format!("{}", self.file.path().display()));
                host.console_print(&format!(
                    "{} lines written this session",
                    self.file.written()
                ));
                Ok(())
            }
            "stats" => {
                host.console_print(&format!(
                    "seen: {} events, {} chat lines, {} remote calls",
                    self.events.load(Ordering::Relaxed),
                    self.chat.load(Ordering::Relaxed),
                    self.rpc.load(Ordering::Relaxed),
                ));
                host.console_print(&format!(
                    "written: {} lines to {}",
                    self.file.written(),
                    self.file.path().display()
                ));
                Ok(())
            }
            "flush" => {
                self.file.flush();
                host.console_print("event log flushed");
                Ok(())
            }
            other => Err(PluginError::Message(format!("no such command: {other}"))),
        }
    }

    fn stop(&self, _host: &Host, _reason: StopReason) {
        self.file.flush();
    }
}

/// One of the pattern list settings, parsed. Read per event rather than cached because the
/// settings editor and the console can change it at any moment, and a logger that needed
/// restarting to widen its filter would be the wrong tool.
fn list(host: &Host, key: &str) -> Filter {
    let default = match key {
        "toasts" => DEFAULT_TOASTS,
        _ => DEFAULT_EVENTS,
    };
    Filter::parse(&host.get(key).unwrap_or_else(|_| default.to_owned()))
}

impl GameLog {
    /// Write one line to whichever sinks are on.
    fn emit(&self, host: &Host, kind: &str, what: &str) {
        let line = format!("{}  {kind:<5}  {what}", sink::stamp());
        if host.get_as::<bool>("to_file").unwrap_or(true) {
            self.file.write_line(&line);
        }
        if host.get_as::<bool>("to_console").unwrap_or(true) {
            host.console_print(&format!("{kind:<5} {what}"));
        }
        if host.get_as::<bool>("to_log").unwrap_or(false) {
            log::info!("{kind} {what}");
        }
    }
}

/// Every setting, in the order the settings editor shows them.
fn register_settings(host: &Host) -> Result<(), PluginError> {
    host.setting(
        &Setting::text("events", "Events to report", DEFAULT_EVENTS).with_description(
            "Comma separated name patterns; `*` is a wildcard and `!` excludes. The last \
             matching rule wins, so `*, !Mouse*` is everything but mouse events.",
        ),
    )?;
    host.setting(
        &Setting::text("toasts", "Events worth a toast", DEFAULT_TOASTS).with_description(
            "Same syntax as `events`, applied after it: an event has to pass both. Empty \
             means no toasts.",
        ),
    )?;
    host.setting(&Setting::bool("chat", "Report chat lines", true))?;
    host.setting(
        &Setting::bool("chat_toasts", "Toast every chat line", false).with_description(
            "Off by default: the game already draws chat, and a toast per line would cover \
             the screen on a busy server.",
        ),
    )?;
    host.setting(&Setting::bool("rpc", "Report remote calls", true))?;
    host.setting(
        &Setting::bool("to_console", "Print to the loader console", true)
            .with_description("The in-game console and the external one, if it is open."),
    )?;
    host.setting(
        &Setting::bool("to_file", "Write to game-events.log", true)
            .with_description("One line per event, beside the loader's own log."),
    )?;
    host.setting(
        &Setting::bool("to_log", "Also write to the loader log", false)
            .advanced()
            .with_description(
                "Off by default: the event log is the place for this, and mixing a line per \
                 event into the loader's own log makes that one hard to read.",
            ),
    )?;
    Ok(())
}

export_plugin!(GameLog);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_alarming_pattern_is_also_a_pattern_that_gets_a_toast() {
        // An alarming event nobody is told about would be a filter that cannot fire.
        let toasts = Filter::parse(DEFAULT_TOASTS);
        for name in [
            "MPConnectionCloseEvent",
            "MPConnectionLostEvent",
            "MPSessionFailEvent",
            "ConnectingAbortEvent",
            "DLCOwnerShipFailedEvent",
            "PlayerDeathEvent",
            "LogoutEvent",
        ] {
            assert!(Filter::parse(ALARMING).covers(name), "{name} is alarming");
            assert!(toasts.covers(name), "{name} should reach a toast");
        }
    }

    #[test]
    fn the_quiet_events_are_logged_without_a_toast() {
        let events = Filter::parse(DEFAULT_EVENTS);
        let toasts = Filter::parse(DEFAULT_TOASTS);
        for name in ["VONStateEvent", "SetFreeCameraEvent", "DialogQueuedEvent"] {
            assert!(events.covers(name), "{name} should be logged");
            assert!(!toasts.covers(name), "{name} should not interrupt anyone");
        }
    }
}

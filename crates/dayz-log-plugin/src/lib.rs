//! Reports what the game does: every event it raises, every chat line, every remote call.
//!
//! The `dayz` adapter hooks the engine's event broadcaster, its chat path and its RPC
//! dispatcher; this plugin subscribes to all three and sends what arrives wherever it has been
//! told to — a log file, the loader console, a toast, a dialog, or this client's own chat.
//! Toasts, dialogs and the Events panel need the overlay plugin; without it those
//! destinations stay silent and everything else works.
//!
//! ```text
//! 1759600412.118  event  MPSessionStartEvent
//! 1759600413.902  chat   [0] <server> You have joined the server
//! 1759600421.344  event  ChatMessageEvent  channel=0, from=server, text=You have joined…
//! 1759600488.010  event  RespawnEvent  respawntime=30
//! 1759600502.771  event  MPConnectionCloseEvent  eclientkicked=3, additionalinfo=
//! ```
//!
//! ## What it can and cannot tell you
//!
//! Event *names* are read out of each event's own virtual table, so they are right for
//! classes nobody has looked at. Event *contents* come from the loader's symbol database,
//! which has a layout for some of the classes and not the others — 17 of 77 on 1.29.163709.
//! An event with no fields is one nobody has established the layout for yet, and it is still
//! reported by name.
//!
//! The two differ because they come from different places, and that is worth keeping straight
//! when reading a line: the name is a fact about the object in front of you, the fields are a
//! fact somebody wrote down about this build. A field the loader could not read says
//! `<unreadable>` rather than guessing, so a wrong offset in the database looks wrong.
//!
//! ## Where a line goes
//!
//! Five destinations, set per event in the **Events** panel or in the `sinks` setting; see
//! the `sinks` module for the table and why the pattern lists still sit underneath it. The `to_file`,
//! `to_console` and `to_log` settings are the master switches over the top, and they are what
//! chat lines and remote calls follow, since those have no event class to look up.

mod filter;
mod panel;
mod sink;
mod sinks;

use std::collections::BTreeSet;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, RwLock};

use dayz_game::{Chat, ChatVerdict, Class, Dayz, Event, GameListener, Rpc, Streams};
use uniloader_overlay::{Overlay, UiDialog, UiLevel};
use uniloader_sdk::{export_plugin, Dependency, Host, Plugin, PluginError, Setting, StopReason};

use filter::Filter;
use sink::LogFile;
use sinks::{Rules, Sinks};

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

/// Colour class a line put into the game's own chat is drawn in.
const CHAT_COLOUR: &str = "ColorImportant";

/// The settings that decide where a line goes, mirrored out of the loader.
///
/// Mirrored rather than read per line because `on_game_event` runs on the game's own thread
/// for every event the engine raises, and three settings lookups per event there is three
/// string parses per event in the loader. These are written when the setting changes, which
/// is approximately never.
#[derive(Debug, Default)]
struct Switches {
    to_file: AtomicBool,
    to_console: AtomicBool,
    to_log: AtomicBool,
    chat: AtomicBool,
    chat_toasts: AtomicBool,
    rpc: AtomicBool,
}

impl Switches {
    /// Read every one out of the loader.
    fn refresh(&self, host: &Host) {
        let get = |key: &str, fallback: bool| host.get_as::<bool>(key).unwrap_or(fallback);
        self.to_file.store(get("to_file", true), Ordering::Relaxed);
        self.to_console
            .store(get("to_console", true), Ordering::Relaxed);
        self.to_log.store(get("to_log", false), Ordering::Relaxed);
        self.chat.store(get("chat", true), Ordering::Relaxed);
        self.chat_toasts
            .store(get("chat_toasts", false), Ordering::Relaxed);
        self.rpc.store(get("rpc", true), Ordering::Relaxed);
    }
}

/// How much has come through, for the `stats` command.
#[derive(Debug, Default)]
struct Counts {
    events: AtomicU64,
    chat: AtomicU64,
    rpc: AtomicU64,
}

/// The plugin. Everything lives in [`Inner`], shared with the adapter's listener and the
/// overlay's panel, which both call back without a `&self`.
struct GameLog {
    inner: Arc<Inner>,
}

/// What the plugin knows and keeps.
pub(crate) struct Inner {
    host: Host,
    dayz: Dayz,
    /// `None` without the overlay plugin: no toasts, dialogs or panel then.
    overlay: Option<Overlay>,
    file: LogFile,
    /// Where each event's line goes. A read lock per event; written only from the panel.
    rules: RwLock<Rules>,
    switches: Switches,
    counts: Counts,
    /// Every class the loader knows about, asked for once. The panel's rows.
    catalogue: Vec<Class>,
    /// The catalogue's names as a set. `on_game_event` asks "is this one new?" for every
    /// event the engine raises, and that question should not be a scan of seventy-seven
    /// strings on the game's own thread.
    known: std::collections::HashSet<&'static str>,
    /// Classes that turned up without being in the catalogue, so they can be configured too.
    extra: RwLock<BTreeSet<Box<str>>>,
}

impl Plugin for GameLog {
    const NAME: &'static str = "gamelog";
    const TITLE: &'static str = "Game log";
    const VERSION: &'static str = env!("CARGO_PKG_VERSION");
    const AUTHORS: &'static str = "Bluscream";
    const DESCRIPTION: &'static str =
        "Reports the game's own events, chat and remote calls to the console, a log file and toasts.";
    const DEPENDENCIES: &'static [Dependency] = &[
        Dependency::plugin(dayz_game::PLUGIN, ">=0.1"),
        Dependency::plugin(uniloader_overlay::PLUGIN, ">=0.1").optional(),
    ];

    fn start(host: Host) -> Result<Self, PluginError> {
        let dayz = Dayz::connect(&host)
            .ok_or_else(|| PluginError::from("the dayz adapter is not running".to_owned()))?;
        register_settings(&host)?;
        host.command("where", "Print where the event log is written.", "")?;
        host.command("stats", "Print how much has been seen and written.", "")?;
        host.command("flush", "Write the event log out now.", "")?;
        // Not `events`: that is the name of a setting, and `gamelog.events` on the console
        // resolves to the setting rather than the command. Found the obvious way.
        host.command(
            "classes",
            "List every event class, and where each one's line goes.",
            "",
        )?;
        let catalogue = dayz.catalogue().unwrap_or_default();
        let known = catalogue.iter().map(|class| class.name).collect();
        let inner = Arc::new(Inner {
            host,
            dayz,
            overlay: Overlay::connect(&host),
            file: LogFile::open(&host.config_dir()),
            rules: RwLock::new(Rules::default()),
            switches: Switches::default(),
            counts: Counts::default(),
            catalogue,
            known,
            extra: RwLock::new(BTreeSet::new()),
        });
        inner.reload();
        if let Some(overlay) = &inner.overlay {
            let panel = Arc::clone(&inner);
            overlay.panel("events", "Events", false, move |ui| panel::draw(&panel, ui))?;
        }
        // Everything, filtered here rather than there: the adapter's mask is per stream and
        // the interesting filtering is per event name, which only this plugin knows about.
        dayz.listen(Streams::ALL, Arc::clone(&inner))?;
        log::info!(
            "watching the game's events, chat and remote calls; {} classes known",
            inner.catalogue.len()
        );
        Ok(GameLog { inner })
    }

    fn on_setting_changed(&self, _host: &Host, key: &str, _value: &str) {
        // `search` is the panel's own filter and changes while somebody types in it; rebuilding
        // the rules on every keystroke would be work for nothing.
        if key != "search" {
            self.inner.reload();
        }
    }

    fn on_command(&self, host: &Host, name: &str, _args: &str) -> Result<(), PluginError> {
        self.inner.command(host, name)
    }

    fn stop(&self, _host: &Host, _reason: StopReason) {
        self.inner.file.flush();
    }
}

impl GameListener for Inner {
    fn on_game_event(&self, event: &Event<'_>) {
        self.counts.events.fetch_add(1, Ordering::Relaxed);
        let sinks = self.rules().for_event(event.name);
        if sinks.is_empty() {
            // Remembered even so: an event nobody wants to see today is still one they may
            // want to tick a box for tomorrow, and the panel can only offer names it has.
            self.remember(event.name);
            return;
        }
        self.remember(event.name);
        let fields = event.fields.iter().map(|field| (field.name, field.text));
        let what = describe(event.name, event.object, fields);
        self.emit("event", &what, sinks, alarming(event.name));
    }

    fn on_chat(&self, chat: &Chat<'_>) -> ChatVerdict {
        self.counts.chat.fetch_add(1, Ordering::Relaxed);
        if self.switches.chat.load(Ordering::Relaxed) {
            let from = if chat.from.is_empty() {
                String::new()
            } else {
                format!("<{}> ", chat.from)
            };
            let what = format!("[{}] {from}{}", chat.channel, chat.text);
            // A chat line has no event class to look up, so it follows the master switches
            // and its own two settings. Never to chat: that is where it came from.
            let mut sinks = Sinks::FILE | Sinks::CONSOLE;
            if self.switches.chat_toasts.load(Ordering::Relaxed) {
                sinks = sinks | Sinks::TOAST;
            }
            self.emit("chat", &what, sinks, false);
        }
        // This plugin reports; it never takes a line off the screen. Swallowing is in the ABI
        // for a plugin whose job is to filter chat, and a logger that could silently eat a
        // message would be the wrong thing to have running all the time.
        ChatVerdict::PASS
    }

    fn on_rpc(&self, call: &Rpc) {
        self.counts.rpc.fetch_add(1, Ordering::Relaxed);
        if !self.switches.rpc.load(Ordering::Relaxed) {
            return;
        }
        let what = format!(
            "kind {} from {:p} to {:p} params {:p}",
            call.kind, call.sender, call.target, call.params
        );
        self.emit("rpc", &what, Sinks::FILE | Sinks::CONSOLE, false);
    }
}

impl Inner {
    /// The plugin's console commands.
    fn command(&self, host: &Host, name: &str) -> Result<(), PluginError> {
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
                    self.counts.events.load(Ordering::Relaxed),
                    self.counts.chat.load(Ordering::Relaxed),
                    self.counts.rpc.load(Ordering::Relaxed),
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
            "classes" => {
                let rules = self.rules();
                for (name, fields) in self.rows() {
                    let how = if rules.is_explicit(&name) {
                        "set here"
                    } else {
                        "by pattern"
                    };
                    let sinks = rules.for_event(&name).to_string();
                    let plural = if fields == 1 { "field " } else { "fields" };
                    host.console_print(&format!(
                        "{name:<40} {fields:>2} {plural}  {sinks:<6} ({how})"
                    ));
                }
                Ok(())
            }
            other => Err(PluginError::Message(format!("no such command: {other}"))),
        }
    }

    /// Borrow the rules for reading.
    ///
    /// A poisoned lock is treated as readable: the rules are a plain value, a panic while
    /// holding them cannot have left them half-written in a way that matters, and refusing to
    /// report anything for the rest of the session would be the worse failure.
    pub(crate) fn rules(&self) -> std::sync::RwLockReadGuard<'_, Rules> {
        self.rules
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    /// Re-read every setting this plugin's behaviour depends on.
    fn reload(&self) {
        let host = &self.host;
        self.switches.refresh(host);
        let rules = Rules::new(
            &host.get("sinks").unwrap_or_default(),
            &host
                .get("events")
                .unwrap_or_else(|_| DEFAULT_EVENTS.to_owned()),
            &host
                .get("toasts")
                .unwrap_or_else(|_| DEFAULT_TOASTS.to_owned()),
        );
        *self
            .rules
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = rules;
    }

    /// Take the panel's changes, write them back to the setting, and keep them.
    pub(crate) fn reassign(&self, changes: &[(String, Sinks)]) {
        let text = {
            let mut rules = self
                .rules
                .write()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            panel::apply(&mut rules, changes);
            rules.to_setting()
        };
        // Written through the loader so it is validated, persisted and announced like any
        // other setting change — which also means `on_setting_changed` rebuilds the rules
        // from the text, so what is kept is what was saved.
        if let Err(e) = self.host.set("sinks", &text) {
            log::warn!("could not save the event table: {e}");
        }
    }

    /// Put every event back on the pattern lists.
    pub(crate) fn clear_overrides(&self) {
        self.rules
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clear();
        if let Err(e) = self.host.set("sinks", "") {
            log::warn!("could not clear the event table: {e}");
        }
    }

    /// Every class the panel and the `events` command list: the catalogue, then anything seen
    /// that was not in it.
    /// Owned rather than borrowed, because the extras are behind a lock and a panel frame
    /// is not a hot path: it is drawn only while somebody has the window open.
    pub(crate) fn rows(&self) -> Vec<(String, usize)> {
        let mut rows: Vec<(String, usize)> = self
            .catalogue
            .iter()
            .map(|class| (class.name.to_owned(), class.field_count))
            .collect();
        let extra = self
            .extra
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        rows.extend(extra.iter().map(|name| (name.to_string(), 0)));
        rows
    }

    /// Note a class the catalogue did not have, so the panel can offer it.
    fn remember(&self, name: &str) {
        {
            let extra = self
                .extra
                .read()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            if extra.contains(name) {
                return;
            }
        }
        if self.known.contains(name) {
            return;
        }
        self.extra
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .insert(name.to_owned().into_boxed_str());
    }

    /// Send one line to each of its destinations.
    fn emit(&self, kind: &str, what: &str, sinks: Sinks, alarming: bool) {
        if sinks.has(Sinks::FILE) && self.switches.to_file.load(Ordering::Relaxed) {
            self.file
                .write_line(&format!("{}  {kind:<5}  {what}", sink::stamp()));
        }
        if sinks.has(Sinks::CONSOLE) && self.switches.to_console.load(Ordering::Relaxed) {
            self.host.console_print(&format!("{kind:<5} {what}"));
        }
        if self.switches.to_log.load(Ordering::Relaxed) {
            log::info!("{kind} {what}");
        }
        let level = if alarming {
            UiLevel::WARNING
        } else {
            UiLevel::INFO
        };
        // Both need the overlay; without it these two destinations are silent by design.
        if let Some(overlay) = &self.overlay {
            if sinks.has(Sinks::TOAST) {
                let text: String = what.chars().take(CHAT_TOAST_LIMIT).collect();
                let _ = overlay.toast(level, kind, &text);
            }
            if sinks.has(Sinks::BOX) {
                let _ = overlay.dialog(UiDialog::MESSAGE, level, kind, what, "", |_, _| {});
            }
        }
        if sinks.has(Sinks::CHAT) {
            // Ignored on purpose: the game object is not known until the engine has dispatched
            // a remote call, so this fails in the main menu. Logging a warning per event for
            // something that fixes itself on joining a server would be the louder mistake.
            let _ = self
                .dayz
                .chat_local(&format!("{kind}: {what}"), CHAT_COLOUR);
        }
    }
}

/// Whether an event's toast should be a warning.
fn alarming(name: &str) -> bool {
    Filter::parse(ALARMING).covers(name)
}

/// One event as a line: its class, then its fields if the loader could read any.
///
/// Takes the parts rather than the [`Event`], so that what a line looks like is pinned down
/// by a test instead of only by a session with the game running.
fn describe<'a>(
    name: &str,
    object: *mut core::ffi::c_void,
    fields: impl Iterator<Item = (&'a str, &'a str)>,
) -> String {
    let fields = fields
        .map(|(field, value)| format!("{field}={value}"))
        .collect::<Vec<_>>();
    if fields.is_empty() {
        // The address is what is left to say about a class nobody has a layout for, and it is
        // what the loader's `read` command needs to go and look.
        return format!("{name} at {object:p}");
    }
    format!("{name}  {}", fields.join(", "))
}

/// Every setting, in the order the settings editor shows them.
fn register_settings(host: &Host) -> Result<(), PluginError> {
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
        &Setting::text("search", "Find an event", "")
            .transient()
            .with_description("Narrows the Events panel to classes whose name contains this."),
    )?;
    host.setting(
        &Setting::text("sinks", "Where each event goes", "")
            .advanced()
            .with_description(
                "The Events panel writes this. `Name=flags` entries, comma separated, where \
                 the flags are f(ile), c(onsole), t(oast), m(essage box) and (c)h(at); `-` \
                 means nowhere. An event not named here follows the two pattern settings.",
            ),
    )?;
    host.setting(
        &Setting::text("events", "Events to report", DEFAULT_EVENTS)
            .advanced()
            .with_description(
                "The fallback for events the panel has not been used on. Comma separated \
                 name patterns; `*` is a wildcard and `!` excludes. The last matching rule \
                 wins, so `*, !Mouse*` is everything but mouse events.",
            ),
    )?;
    host.setting(
        &Setting::text("toasts", "Events worth a toast", DEFAULT_TOASTS)
            .advanced()
            .with_description(
                "Same syntax and the same fallback role as `events`, for toasts. Empty means \
                 no toasts except the ones the panel asks for.",
            ),
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
            assert!(alarming(name), "{name} is alarming");
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

    /// The defaults have to come out of `Rules` the same way they used to come out of the two
    /// filters, or installing this version would quietly change what everybody sees.
    #[test]
    fn the_default_rules_match_what_the_pattern_lists_say() {
        let rules = Rules::new("", DEFAULT_EVENTS, DEFAULT_TOASTS);
        assert_eq!(
            rules.for_event("MPSessionStartEvent"),
            Sinks::FILE | Sinks::CONSOLE | Sinks::TOAST
        );
        assert_eq!(
            rules.for_event("VONStateEvent"),
            Sinks::FILE | Sinks::CONSOLE
        );
        assert!(rules.for_event("AfterRenderEvent").is_empty());
        assert_eq!(rules.explicit_count(), 0, "nothing is set by hand yet");
    }

    #[test]
    fn an_event_with_no_known_layout_is_described_by_name_and_address() {
        let line = describe("SomeNewEvent", core::ptr::null_mut(), core::iter::empty());
        assert!(line.starts_with("SomeNewEvent at "), "{line}");
    }

    #[test]
    fn a_decoded_event_is_described_by_its_fields() {
        let fields = [("channel", "0"), ("from", "server"), ("text", "hello")];
        let line = describe(
            "ChatMessageEvent",
            core::ptr::null_mut(),
            fields.into_iter(),
        );
        assert_eq!(
            line, "ChatMessageEvent  channel=0, from=server, text=hello",
            "the fields stay in the order the engine's own parameters list them"
        );
        assert!(
            !line.contains("0x"),
            "an event with fields does not need its address as well"
        );
    }

    /// An empty string field has to be visible as one. The engine stores an empty string as a
    /// null pointer, so `from=` for a system message is the correct and informative rendering
    /// — dropping the field would make a system line look like a malformed one.
    #[test]
    fn an_empty_field_still_appears() {
        let line = describe("Ev", core::ptr::null_mut(), [("from", "")].into_iter());
        assert_eq!(line, "Ev  from=");
    }
}

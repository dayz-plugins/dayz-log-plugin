//! Where one event's line goes: the five destinations, and the rules that pick them.
//!
//! A destination is a bit, and an event's destinations are the five bits together — which is
//! the whole reason this is a bitmask and not five booleans. The settings panel draws a row
//! of five checkboxes per event and stores one value per row; the `sinks` setting is that
//! table written out as `Name=flags`; and a line's destinations are decided with one lookup.
//!
//! ```text
//! sinks = "ChatMessageEvent=fc, MPConnectionCloseEvent=fctm, RespawnEvent=fh"
//!                             │                         │                 └ chat
//!                             │                         └ messagebox
//!                             └ file + console
//! ```
//!
//! ## Why there is still a pattern list underneath
//!
//! An event not named in `sinks` falls back to the `events` and `toasts` pattern lists, and
//! those default to "everything, except these noisy families". That is deliberate and the two
//! are not redundant. A table can only mention classes somebody has already seen, so a table
//! alone would silently stop reporting the first new event a game update introduces — and the
//! person who would notice is the person who installed this to see events. The patterns cover
//! what nobody has decided about yet; the table overrides what they have.

use std::collections::BTreeMap;

use crate::filter::Filter;

/// The five destinations one line can go to.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(crate) struct Sinks(pub u8);

impl Sinks {
    /// Nowhere.
    pub(crate) const NONE: Sinks = Sinks(0);
    /// The plugin's own `game-events.log`.
    pub(crate) const FILE: Sinks = Sinks(1 << 0);
    /// The loader console.
    pub(crate) const CONSOLE: Sinks = Sinks(1 << 1);
    /// A toast, which goes away by itself.
    pub(crate) const TOAST: Sinks = Sinks(1 << 2);
    /// A modal dialog, which does not.
    pub(crate) const BOX: Sinks = Sinks(1 << 3);
    /// This client's own chat, which nobody else sees.
    pub(crate) const CHAT: Sinks = Sinks(1 << 4);

    /// Every destination, with the letter it is written as and the column it is drawn in.
    ///
    /// One table rather than three, so the `sinks` text, the panel's columns and the bit
    /// values cannot drift apart: the order here *is* the column order.
    pub(crate) const ALL: [(Sinks, char, &'static str); 5] = [
        (Sinks::FILE, 'f', "file"),
        (Sinks::CONSOLE, 'c', "console"),
        (Sinks::TOAST, 't', "toast"),
        (Sinks::BOX, 'm', "box"),
        (Sinks::CHAT, 'h', "chat"),
    ];

    /// Whether `other`'s bits are all set here.
    pub(crate) const fn has(self, other: Sinks) -> bool {
        self.0 & other.0 == other.0
    }

    /// Nowhere at all.
    pub(crate) const fn is_empty(self) -> bool {
        self.0 == 0
    }

    /// Read a letter set like `fct`. Unknown letters are ignored rather than refused: this
    /// parses a settings file a person edits by hand, and losing the whole row over one typo
    /// would be worse than losing the typo.
    pub(crate) fn parse(text: &str) -> Sinks {
        let mut sinks = Sinks::NONE;
        for letter in text.chars() {
            let lower = letter.to_ascii_lowercase();
            if let Some((bit, _, _)) = Sinks::ALL.iter().find(|(_, c, _)| *c == lower) {
                sinks = Sinks(sinks.0 | bit.0);
            }
        }
        sinks
    }
}

impl core::fmt::Display for Sinks {
    /// The letters, in column order. Empty prints as `-`, so a row that is deliberately off
    /// is visible in the settings file rather than looking like a missing value.
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        if self.is_empty() {
            return f.write_str("-");
        }
        for (bit, letter, _) in Sinks::ALL {
            if self.has(bit) {
                f.write_fmt(format_args!("{letter}"))?;
            }
        }
        Ok(())
    }
}

impl core::ops::BitOr for Sinks {
    type Output = Sinks;

    fn bitor(self, other: Sinks) -> Sinks {
        Sinks(self.0 | other.0)
    }
}

/// How an event's destinations are decided, parsed once and kept.
///
/// Parsed once because the alternative is parsing three settings strings per event on the
/// game's own thread, and the engine raises events every frame. Rebuilt whenever one of those
/// settings changes, which is what [`Rules::new`] is for.
#[derive(Debug, Default)]
pub(crate) struct Rules {
    /// Per event name, from the `sinks` setting. Wins outright when present.
    table: BTreeMap<Box<str>, Sinks>,
    /// Fallback: names matching this go to the file and the console.
    report: Filter,
    /// Fallback: names matching this also get a toast.
    toast: Filter,
}

impl Rules {
    /// Build from the three settings strings.
    pub(crate) fn new(sinks: &str, events: &str, toasts: &str) -> Rules {
        Rules {
            table: parse_table(sinks),
            report: Filter::parse(events),
            toast: Filter::parse(toasts),
        }
    }

    /// Where one event's line should go.
    pub(crate) fn for_event(&self, name: &str) -> Sinks {
        if let Some(explicit) = self.table.get(name) {
            return *explicit;
        }
        let mut sinks = Sinks::NONE;
        if self.report.covers(name) {
            sinks = sinks | Sinks::FILE | Sinks::CONSOLE;
        }
        if self.toast.covers(name) {
            sinks = sinks | Sinks::TOAST;
        }
        sinks
    }

    /// Whether this event is in the table rather than following the patterns.
    pub(crate) fn is_explicit(&self, name: &str) -> bool {
        self.table.contains_key(name)
    }

    /// Put one event in the table, or take it out again when `sinks` matches what the
    /// patterns would have said anyway.
    ///
    /// Taking it out matters more than it looks: a table entry stops following the patterns
    /// for ever, so an event ticked and then un-ticked back to its default should go back to
    /// being covered by `*` and keep working after a game update renames things around it.
    pub(crate) fn assign(&mut self, name: &str, sinks: Sinks) {
        self.table.remove(name);
        if self.for_event(name) != sinks {
            self.table.insert(name.to_owned().into_boxed_str(), sinks);
        }
    }

    /// Forget every override, so everything follows the patterns again.
    pub(crate) fn clear(&mut self) {
        self.table.clear();
    }

    /// The table as the `sinks` setting stores it.
    pub(crate) fn to_setting(&self) -> String {
        self.table
            .iter()
            .map(|(name, sinks)| format!("{name}={sinks}"))
            .collect::<Vec<_>>()
            .join(", ")
    }

    /// How many events have an explicit entry.
    pub(crate) fn explicit_count(&self) -> usize {
        self.table.len()
    }
}

/// Read `Name=flags` pairs. A pair without an `=` sets every destination, so that a bare list
/// of event names is a usable thing to type.
fn parse_table(text: &str) -> BTreeMap<Box<str>, Sinks> {
    let mut table = BTreeMap::new();
    for entry in text.split(',') {
        let entry = entry.trim();
        if entry.is_empty() {
            continue;
        }
        let (name, flags) = match entry.split_once('=') {
            Some((name, flags)) => (name.trim(), Sinks::parse(flags.trim())),
            None => (entry, Sinks::FILE | Sinks::CONSOLE | Sinks::TOAST),
        };
        if !name.is_empty() {
            table.insert(name.to_owned().into_boxed_str(), flags);
        }
    }
    table
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_destination_has_its_own_bit_and_its_own_letter() {
        let mut bits = 0u8;
        let mut letters = Vec::new();
        for (bit, letter, name) in Sinks::ALL {
            assert_eq!(bit.0.count_ones(), 1, "{name} is one bit");
            assert_eq!(bits & bit.0, 0, "{name}'s bit is its own");
            bits |= bit.0;
            assert!(!letters.contains(&letter), "{letter} is used twice");
            letters.push(letter);
        }
    }

    #[test]
    fn letters_round_trip_through_text() {
        for raw in 0..32u8 {
            let sinks = Sinks(raw);
            assert_eq!(Sinks::parse(&sinks.to_string()), sinks, "{raw:#07b}");
        }
        assert_eq!(Sinks::NONE.to_string(), "-");
        assert_eq!(Sinks::parse("-"), Sinks::NONE);
    }

    #[test]
    fn an_unknown_letter_is_dropped_and_the_rest_of_the_row_survives() {
        assert_eq!(Sinks::parse("fzc"), Sinks::FILE | Sinks::CONSOLE);
        assert_eq!(Sinks::parse("FC"), Sinks::FILE | Sinks::CONSOLE);
        assert_eq!(Sinks::parse("zzz"), Sinks::NONE);
    }

    #[test]
    fn the_table_wins_over_the_patterns() {
        let rules = Rules::new("RespawnEvent=h", "*", "Respawn*");
        assert_eq!(rules.for_event("RespawnEvent"), Sinks::CHAT);
        // And an event the table says nothing about still follows them.
        assert_eq!(
            rules.for_event("MPSessionStartEvent"),
            Sinks::FILE | Sinks::CONSOLE
        );
    }

    #[test]
    fn a_bare_name_in_the_table_means_the_usual_three() {
        let rules = Rules::new("RespawnEvent", "", "");
        assert_eq!(
            rules.for_event("RespawnEvent"),
            Sinks::FILE | Sinks::CONSOLE | Sinks::TOAST
        );
    }

    #[test]
    fn assigning_a_default_takes_the_entry_back_out() {
        let mut rules = Rules::new("", "*", "");
        let default = rules.for_event("RespawnEvent");
        rules.assign("RespawnEvent", Sinks::CHAT);
        assert!(rules.is_explicit("RespawnEvent"));
        rules.assign("RespawnEvent", default);
        assert!(
            !rules.is_explicit("RespawnEvent"),
            "back to its default means back to following the patterns"
        );
        assert_eq!(rules.for_event("RespawnEvent"), default);
    }

    #[test]
    fn the_table_round_trips_through_the_setting() {
        let mut rules = Rules::new("", "", "");
        rules.assign("ChatMessageEvent", Sinks::FILE | Sinks::CONSOLE);
        rules.assign("RespawnEvent", Sinks::CHAT | Sinks::BOX);
        rules.assign("LogoutEvent", Sinks::NONE);
        let text = rules.to_setting();
        let again = Rules::new(&text, "", "");
        for name in ["ChatMessageEvent", "RespawnEvent", "LogoutEvent"] {
            assert_eq!(again.for_event(name), rules.for_event(name), "{name}");
        }
        // Two, not three: with no patterns at all, "nowhere" is already LogoutEvent's
        // default, so `assign` stored nothing for it. That is the point of `assign` — an
        // entry is only kept when it says something the patterns do not.
        assert_eq!(again.explicit_count(), 2);
        assert!(!again.is_explicit("LogoutEvent"));
    }

    #[test]
    fn whitespace_and_empty_entries_are_tolerated() {
        let rules = Rules::new("  RespawnEvent = f , , LogoutEvent=c  ", "", "");
        assert_eq!(rules.for_event("RespawnEvent"), Sinks::FILE);
        assert_eq!(rules.for_event("LogoutEvent"), Sinks::CONSOLE);
        assert_eq!(rules.explicit_count(), 2);
    }

    #[test]
    fn an_event_nothing_mentions_goes_nowhere() {
        let rules = Rules::new("", "", "");
        assert!(rules.for_event("AfterRenderEvent").is_empty());
    }
}

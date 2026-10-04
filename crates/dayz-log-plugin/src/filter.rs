//! Deciding which event names a setting covers.
//!
//! The engine raises far more events than anyone wants to read. `AfterRenderEvent` goes out
//! once a frame, the mouse and keyboard events go through the same broadcaster as the session
//! ones, and two stats events tick away the whole time a server is connected. So every sink
//! in this plugin takes a pattern list rather than a boolean, and the default lists say
//! "everything except the noise" instead of naming the seventy events that are interesting.
//!
//! A list is comma separated and read left to right, with the **last matching rule winning**:
//!
//! ```text
//! *                                      everything
//! Respawn*, PlayerDeath*                 only those two families
//! *, !Mouse*, !Key*                      everything except mouse and keyboard events
//! *, !Mouse*, MouseWheelEvent            everything except mouse events, but keep the wheel
//! ```
//!
//! A rule starting with `!` excludes. `*` stands for any run of characters and may appear
//! anywhere; everything else matches literally, ignoring case, because an event name is a C++
//! class name and nobody wants to be told their rule was right but for the capital letter.
//! An empty list matches nothing, which is how a sink is turned off.

/// One rule: a pattern and whether matching it means yes or no.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Rule {
    /// The pattern, lowercased, split on `*`.
    parts: Vec<String>,
    /// Whether the pattern started and ended with `*`, which the matcher needs to know
    /// because the first and last part are anchored unless it did.
    open_start: bool,
    open_end: bool,
    /// `false` for a rule written with a leading `!`.
    include: bool,
}

/// A parsed pattern list.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Filter {
    rules: Vec<Rule>,
}

impl Filter {
    /// Parse a comma separated list. Unparseable is not a thing here: an empty rule is
    /// dropped, and anything else is a pattern.
    #[must_use]
    pub fn parse(list: &str) -> Filter {
        let rules = list
            .split(',')
            .map(str::trim)
            .filter(|rule| !rule.is_empty())
            .map(Rule::parse)
            .collect();
        Filter { rules }
    }

    /// Whether this name is covered. The last matching rule decides; no match is no.
    #[must_use]
    pub fn covers(&self, name: &str) -> bool {
        let lowered = name.to_lowercase();
        self.rules
            .iter()
            .rev()
            .find(|rule| rule.matches(&lowered))
            .is_some_and(|rule| rule.include)
    }
}

impl Rule {
    fn parse(rule: &str) -> Rule {
        let (include, pattern) = match rule.strip_prefix('!') {
            Some(rest) => (false, rest.trim()),
            None => (true, rule),
        };
        let lowered = pattern.to_lowercase();
        Rule {
            open_start: lowered.starts_with('*'),
            open_end: lowered.ends_with('*'),
            parts: lowered
                .split('*')
                .filter(|part| !part.is_empty())
                .map(ToOwned::to_owned)
                .collect(),
            include,
        }
    }

    /// Whether the pattern matches an already lowercased name.
    fn matches(&self, name: &str) -> bool {
        // A pattern of nothing but stars, which `parse` leaves with no parts at all.
        let Some((first, rest)) = self.parts.split_first() else {
            return true;
        };
        let mut at = if self.open_start {
            match name.find(first.as_str()) {
                Some(found) => found + first.len(),
                None => return false,
            }
        } else if name.starts_with(first.as_str()) {
            first.len()
        } else {
            return false;
        };
        let Some((last, middle)) = rest.split_last() else {
            // One part only: it was the first, so all that is left is the tail anchor.
            return self.open_end || at == name.len();
        };
        for part in middle {
            match name[at..].find(part.as_str()) {
                Some(found) => at += found + part.len(),
                None => return false,
            }
        }
        if self.open_end {
            name[at..].contains(last.as_str())
        } else {
            name[at..].ends_with(last.as_str()) && name.len() - at >= last.len()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_star_covers_everything_and_an_empty_list_covers_nothing() {
        assert!(Filter::parse("*").covers("RespawnEvent"));
        assert!(Filter::parse("*").covers(""));
        assert!(!Filter::parse("").covers("RespawnEvent"));
        assert!(!Filter::parse("  ,  , ").covers("anything"));
    }

    #[test]
    fn a_prefix_pattern_matches_a_family() {
        let filter = Filter::parse("Client*");
        assert!(filter.covers("ClientPrepareEvent"));
        assert!(filter.covers("ClientReadyEvent"));
        assert!(!filter.covers("MPSessionStartEvent"));
    }

    #[test]
    fn a_literal_pattern_matches_only_itself() {
        let filter = Filter::parse("RespawnEvent");
        assert!(filter.covers("RespawnEvent"));
        assert!(!filter.covers("RespawnEventThing"));
        assert!(!filter.covers("PreRespawnEvent"));
    }

    #[test]
    fn case_is_ignored_both_ways() {
        assert!(Filter::parse("respawn*").covers("RespawnEvent"));
        assert!(Filter::parse("RESPAWN*").covers("respawnevent"));
    }

    #[test]
    fn an_exclusion_removes_what_an_earlier_rule_added() {
        let filter = Filter::parse("*, !Mouse*, !Key*");
        assert!(filter.covers("RespawnEvent"));
        assert!(!filter.covers("MouseMoveEvent"));
        assert!(!filter.covers("KeyPressEvent"));
    }

    #[test]
    fn the_last_matching_rule_wins_so_an_exclusion_can_be_undone() {
        let filter = Filter::parse("*, !Mouse*, MouseWheelEvent");
        assert!(!filter.covers("MouseMoveEvent"));
        assert!(filter.covers("MouseWheelEvent"));
    }

    #[test]
    fn a_star_in_the_middle_matches_across_it() {
        let filter = Filter::parse("*StatsUpdated*");
        assert!(filter.covers("ConnectivityStatsUpdatedEvent"));
        assert!(filter.covers("ServerFpsStatsUpdatedEvent"));
        assert!(!filter.covers("MPSessionStartEvent"));
    }

    #[test]
    fn an_unanchored_pattern_needs_every_part_in_order() {
        let filter = Filter::parse("MP*Close*");
        assert!(filter.covers("MPConnectionCloseEvent"));
        assert!(!filter.covers("CloseMPConnectionEvent"));
    }

    #[test]
    fn the_default_event_list_keeps_the_session_events_and_drops_the_noise() {
        let filter = Filter::parse(crate::DEFAULT_EVENTS);
        for wanted in [
            "ConnectingStartEvent",
            "MPConnectionCloseEvent",
            "MPSessionStartEvent",
            "ClientPrepareEvent",
            "RespawnEvent",
            "PlayerDeathEvent",
            "LoginStatusEvent",
            "StartupEvent",
            "ChatMessageEvent",
            "WorldCleaupEvent",
        ] {
            assert!(
                filter.covers(wanted),
                "{wanted} should be logged by default"
            );
        }
        for unwanted in [
            "AfterRenderEvent",
            "BeforeResetEvent",
            "AfterResetEvent",
            "MouseMoveEvent",
            "MousePositionEvent",
            "KeyPressEvent",
            "RawKeyEvent",
            "CharEvent",
            "GamepadEvent",
            "JoystickEvent",
            "ConnectivityStatsUpdatedEvent",
            "ServerFpsStatsUpdatedEvent",
            "NetworkInputBufferEvent",
            "LoadWorldProgressEvent",
            "ProgressEvent",
        ] {
            assert!(
                !filter.covers(unwanted),
                "{unwanted} is noise and should not be logged by default"
            );
        }
    }

    #[test]
    fn the_default_toast_list_is_narrower_than_the_default_event_list() {
        let events = Filter::parse(crate::DEFAULT_EVENTS);
        let toasts = Filter::parse(crate::DEFAULT_TOASTS);
        assert!(toasts.covers("MPConnectionCloseEvent"));
        assert!(toasts.covers("RespawnEvent"));
        // Logged, but not worth interrupting anyone for.
        assert!(events.covers("VONStateEvent"));
        assert!(!toasts.covers("VONStateEvent"));
        assert!(!toasts.covers("ChatMessageEvent"));
    }
}

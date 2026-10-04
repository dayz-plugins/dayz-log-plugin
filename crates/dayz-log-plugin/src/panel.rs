//! The event table: one row per event class, one checkbox per destination.
//!
//! The rows come from the loader's event catalogue, which is every class it knows the shape
//! of — *before* any of them has been raised. That matters more than it sounds: a table built
//! from events as they arrive would fill up as the session went on, and a player cannot tick
//! a box for an event they have not seen yet. Classes that do turn up without being in the
//! catalogue are added to the end of the list, so an event from a game update can still be
//! configured.
//!
//! ```text
//! Events                                     file  console  toast  box  chat
//! ChatMessageEvent          4 fields           x      x
//! ClientConnectedEvent      2 fields           x      x      x
//! MPConnectionCloseEvent    2 fields           x      x      x     x
//! RespawnEvent              1 field            x                        x
//! ```
//!
//! Every tick is written straight back to the `sinks` setting, because a panel with an unsaved
//! state is a panel that loses it the first time the game closes unexpectedly — which, for a
//! plugin whose job is to tell you what the game just did, is exactly when it matters.

use dayz_plugin_sdk::{Host, Ui};

use crate::sinks::{Rules, Sinks};
use crate::GameLog;

/// Points reserved for the name column, so the checkboxes line up down the table.
const NAME_WIDTH: f32 = 300.0;

/// Draw the table. This is the body of `on_ui` for the `events` panel.
pub(crate) fn draw(plugin: &GameLog, host: &Host, ui: &Ui) {
    ui.label(&format!(
        "{} event classes, {} with decoded fields",
        plugin.catalogue.len(),
        plugin
            .catalogue
            .iter()
            .filter(|class| class.field_count > 0)
            .count()
    ));
    // The loader draws, validates and persists it; a failure here means the setting is not
    // registered, which is a bug in `start` rather than something to report per frame.
    let _ = ui.setting("search");
    ui.separator();
    columns(ui);

    let search = host.get("search").unwrap_or_default().to_lowercase();
    let mut changed = Vec::new();
    {
        let rules = plugin.rules();
        for (name, fields) in plugin.rows() {
            if !search.is_empty() && !name.to_lowercase().contains(&search) {
                continue;
            }
            let mut bits = i64::from(rules.for_event(&name).0);
            let label = match fields {
                0 => name.clone(),
                1 => format!("{name}   (1 field)"),
                n => format!("{name}   ({n} fields)"),
            };
            #[allow(clippy::cast_possible_truncation)]
            let columns = Sinks::ALL.len() as u32;
            if ui.toggles(&label, &mut bits, columns, NAME_WIDTH) {
                // Collected rather than written here: writing needs the rules for writing and
                // this loop is holding them for reading.
                changed.push((name, from_bits(bits)));
            }
        }
    }
    if !changed.is_empty() {
        plugin.reassign(host, &changed);
    }

    ui.separator();
    footer(plugin, host, ui);
}

/// A legend for the columns, rather than a header row.
///
/// A header made of labels cannot line up with the checkboxes under it — a label and a
/// checkbox are not the same width, and the font is proportional — so this does not pretend
/// to. It names the columns in order and leaves the reader to count, which is honest about
/// five boxes in a way that a misaligned header would not be.
fn columns(ui: &Ui) {
    let names: Vec<&str> = Sinks::ALL.iter().map(|(_, _, name)| *name).collect();
    ui.label(&format!("boxes, left to right: {}", names.join("  ·  ")));
}

/// The count and the reset button under the table.
fn footer(plugin: &GameLog, host: &Host, ui: &Ui) {
    let explicit = plugin.rules().explicit_count();
    ui.label(&format!(
        "{explicit} of them set here; the rest follow the `events` and `toasts` patterns"
    ));
    if ui.button("Back to the patterns for every event") {
        plugin.clear_overrides(host);
    }
}

/// A toggle row's value back as a destination set.
fn from_bits(bits: i64) -> Sinks {
    // The widget cannot set a bit the row does not have, but it is the loader's number and
    // this is the one place it is believed, so it is masked rather than cast.
    let mask = Sinks::ALL
        .iter()
        .fold(0u8, |mask, (bit, _, _)| mask | bit.0);
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    Sinks((bits as u64 as u8) & mask)
}

/// Apply the table's own idea of the defaults to a fresh [`Rules`].
pub(crate) fn apply(rules: &mut Rules, changes: &[(String, Sinks)]) {
    for (name, sinks) in changes {
        rules.assign(name, *sinks);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_rows_bits_survive_the_trip_through_the_widget() {
        for raw in 0..32u8 {
            let sinks = Sinks(raw);
            assert_eq!(from_bits(i64::from(sinks.0)), sinks, "{raw:#07b}");
        }
    }

    #[test]
    fn a_number_the_row_could_not_have_produced_is_masked_rather_than_trusted() {
        assert_eq!(from_bits(-1), Sinks(0b0001_1111));
        assert_eq!(from_bits(i64::MAX), Sinks(0b0001_1111));
        assert_eq!(from_bits(0b1110_0000), Sinks::NONE);
    }

    #[test]
    fn applying_changes_puts_them_all_in() {
        let mut rules = Rules::new("", "", "");
        apply(
            &mut rules,
            &[
                ("RespawnEvent".to_owned(), Sinks::CHAT),
                ("LogoutEvent".to_owned(), Sinks::FILE),
            ],
        );
        assert_eq!(rules.for_event("RespawnEvent"), Sinks::CHAT);
        assert_eq!(rules.for_event("LogoutEvent"), Sinks::FILE);
    }
}

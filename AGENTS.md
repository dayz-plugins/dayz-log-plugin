# Working on dayz-log-plugin

Read `../skills-public/coding-dev.md` and `../skills-public/code-languages/rust.md` first;
this file only covers what is specific to this repository.

## The rule that shapes everything here

**This plugin reports; it does not interpret.** It knows three things the loader hands it — an
event's name, a chat line's four fields, a remote call's four values — and it decides where
each one goes. It must not grow a table of event names mapped to meanings, a struct mirroring
`MPConnectionCloseEvent`'s fields, or a guess at what an RPC number stands for.

The reason is that none of those are this repository's to know. An event's layout is a fact
about the game binary, and a fact about the game binary belongs in
[dayz-data](https://github.com/dayz-plugins/dayz-data) with the reasoning in
[dayz-plugins.github.io](https://github.com/dayz-plugins/dayz-plugins.github.io) — where the
loader can resolve it per build, and where it stays right when a game update moves it. A copy
in here would be a second source of truth that nothing checks.

So when an event's contents do become readable, the decoding goes in the **loader**, reaches
this plugin as more fields on `Event`, and this plugin's change is one more `format!`
argument. If a change here needs an offset, it is in the wrong repository.

## Layout

- `crates/dayz-log-plugin` — `cdylib` named `gamelog`, built for `x86_64-pc-windows-msvc`.
  - `lib.rs` — the `Plugin` impl, the settings, the three callbacks. The only file that knows
    what a line looks like.
  - `filter.rs` — the pattern matcher. No loader, no game: it tests on any host, and it is the
    piece most likely to be wrong in a way nobody notices, so it carries the most tests.
  - `sink.rs` — the log file. Also host-testable.

The SDK comes from a sibling `dayz-plugin-loader` checkout as a path dependency, not from a
registry. `scripts/build.sh` checks for it and says so; `DAYZ_LOADER_DIR` moves it.

## The defaults are the feature

`DEFAULT_EVENTS` is a deny list — "everything, except these noisy families" — and not an allow
list, on purpose. An allow list would be out of date the first time a game update adds an
event class, and the person who would notice is the person who no longer sees the thing they
installed this for. Keep it that way: a new exclusion is fine, replacing the `*` is not.

When adding an exclusion, add it to the test in `filter.rs` that asserts the defaults keep the
session events and drop the noise. That test is the specification for these two constants.

## Callbacks run inside the engine

`on_game_event` runs with the engine's own broadcast on the stack, for every event the game
raises, and `on_chat` runs before the game draws a line. Neither may block, allocate without
bound, or do anything that might re-enter the engine. The log file is opened once and kept
open for exactly this reason — a file system call per event would be on the game's thread.

`on_chat` returns `ChatVerdict::PASS`, always. The ABI allows swallowing and this plugin
deliberately does not use it: a logger that could silently eat a message is the wrong thing to
leave running. A chat filter is a different plugin.

## The gate

`scripts/build.sh` is the only accepted way to call something finished: format, clippy on the
host, tests, docs, clippy for the Windows target, then the release DLL. It filters nothing and
stops on the first failure. `--fast` skips the host gate when iterating; the full gate has to
pass before anything is committed.

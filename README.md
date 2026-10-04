# dayz-log-plugin

A plugin for the [DayZ plugin loader](https://github.com/dayz-plugins/dayz-plugin-loader) that
reports what the game does: every event the engine raises, every chat line, and every remote
call a mod sends. Lines go to the loader console, to `game-events.log`, and — for the handful
that mean the session changed under you — to a toast.

```text
1759600412.118  event  MPSessionStartEvent at 0x000000003f8a1c40
1759600413.902  chat   [0] <server> You have joined the server
1759600421.344  rpc    kind 14001 from 0x24f1a2b0 to 0x0 params 0x3f8a2110
1759600488.010  event  RespawnEvent at 0x000000003f8a1c40
```

## Installing

Build and deploy in one step, with a `dayz-plugin-loader` checkout beside this one:

```bash
scripts/build.sh --deploy
```

That installs `gamelog.dll` into `<game>/plugins/`. The loader picks it up on the next launch;
`plugin load gamelog` in the console loads it into a running game.

## What it can and cannot tell you

Event **names** are reliable. The loader reads each event's class name out of the event's own
virtual table, so a class nobody has looked at still names itself correctly —
`ConnectingStartEvent`, `MPConnectionCloseEvent`, `ClientPrepareEvent`, `RespawnEvent`,
`PlayerDeathEvent` and about seventy more.

Event **contents** are not decoded. Every class keeps its fields at its own offsets, and this
plugin logs the name and the object's address rather than inventing a layout for it. So
`MPConnectionCloseEvent` currently tells you that the connection closed and when, not why. The
address is in the line so you can go and look with the loader's `read` command.

Chat is the exception: the loader catches it where the engine still has the channel and the
three strings as separate arguments, so those fields are real.

## Settings

Every setting is live — change it in the settings editor or with `set gamelog.<key> <value>`
and the next event uses it. No restart, because a logger you have to restart to widen is the
wrong tool for watching something that already happened once.

| Setting | Default | What it does |
| :--- | :--- | :--- |
| `events` | everything but the noise | Name patterns for which events are reported. |
| `toasts` | the session events | Name patterns for which events also get a toast. |
| `chat` | `true` | Report chat lines. |
| `chat_toasts` | `false` | Toast every chat line. The game already draws chat. |
| `rpc` | `true` | Report remote calls. |
| `to_console` | `true` | Print to the loader console. |
| `to_file` | `true` | Write to `game-events.log`. |
| `to_log` | `false` | Also write into the loader's own log. Advanced. |

### Pattern lists

`events` and `toasts` take a comma separated list of name patterns, read left to right, with
**the last matching rule winning**. `*` is a wildcard anywhere in a pattern, `!` in front of a
rule excludes, and case is ignored.

```text
*                               every event
Respawn*, PlayerDeath*          only those two families
*, !Mouse*, !Key*               everything except mouse and keyboard events
*, !Mouse*, MouseWheelEvent     everything except mouse events, but keep the wheel
                                (empty) nothing
```

The default for `events` is the second-to-last form: the engine raises its render, mouse and
keyboard events through the same broadcaster as the session ones, so "log everything" means a
line per frame. The default excludes those families rather than listing the interesting ones,
so an event class added by a game update shows up without anyone editing a list.

## Commands

| Command | What it does |
| :--- | :--- |
| `gamelog.where` | Print the log file's path and how many lines it has. |
| `gamelog.stats` | Print how much has been seen on each stream, and how much written. |
| `gamelog.flush` | Write the log out now rather than at the next flush. |

`game` — a loader command, not this plugin's — shows the other half: which of the engine's
streams the loader managed to hook, how much has passed through them, and who is listening.

## How it works

Nothing in this repository touches the game. The loader installs three detours during its own
initialisation and offers them as three subscribable streams; this plugin asks for all three
and decides what to do with what arrives.

- `event.raise`, the engine's event broadcaster, which every event passes through.
- `chat.raise_message`, one level below the chat event, where the parts are still separate.
- `rpc.dispatch_to_script`, a mod's remote call on its way to script's `OnRPC`.

The addresses come from [dayz-data](https://github.com/dayz-plugins/dayz-data) and the
reasoning behind them is written up in
[research/events.md](https://dayz-plugins.github.io/research/events.html). On a build whose
addresses are not in the database the loader hooks nothing, `listen_game` fails, and this
plugin says so in the log instead of starting.

## Licence

Unlicense. See [LICENSE](LICENSE).

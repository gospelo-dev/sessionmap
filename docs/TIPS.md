# Tips

Findings from using sessionmap on real machines. Each one is backed by numbers
you can reproduce.

[日本語版](https://github.com/gospelo-dev/sessionmap/blob/main/docs/TIPS_ja.md)

## Run OpenCode as one server with several attached clients

Starting `opencode` normally gives you one process per session, and each one
pays the full startup cost. Splitting it into a single `opencode serve` plus one
`opencode attach` per session pays that cost once.

**It only wins from the second session on.** With a single session the split
costs you ~190 MB extra.

### Measured

macOS 26.6.2, opencode 1.18.29, physical footprint via `footprint -p`.

| Sessions | `opencode` × N | `serve` + `attach` × N | Difference |
|---:|---:|---:|---:|
| 1 | 606 MB | 798 MB | +192 MB |
| **2** | 1,212 MB | **985 MB** | **−227 MB** |
| **3** | 1,818 MB | **1,131 MB** | **−687 MB (38%)** |
| 4 | 2,424 MB | ~1,390 MB | ~−1,030 MB |

Rows 1 to 3 were measured directly; row 4 is a projection from the model below.
Every session in the comparison was a light one (about 16k of context) on both
sides, so this is a fair comparison of what it costs to *have* N sessions open.
Both sides grow once those sessions carry real work — see the client figures
under "What you give up".

The server does not grow with the number of clients:

```
0 clients   244 MB
1 client    575 MB   +331 MB  ← one-time initialisation, not a per-session cost
2 clients   553 MB
3 clients   493 MB            ← still not growing
```

That first jump is what makes a single session a bad deal, and it is also why
the arithmetic flips as soon as you add a second one. The server's own figure
drifts between roughly 490 and 590 MB as its heap is collected, so treat it as
a band rather than a fixed number.

Each attached client costs about 213 MB, so:

```
opencode × N        ≈ 606 N
serve + attach × N  ≈ 540 + 213 N        (N ≥ 1)

break-even:  540 + 213 N < 606 N  →  N ≥ 2
```

A standalone TUI is expensive because it carries the whole agent runtime as well
as the terminal: 606 MB against 244 MB for a bare server. Splitting them does not
divide that figure cleanly — a bare server (244 MB) plus a client (213 MB) comes
to 457 MB, not 606 MB — but the shape is what matters. You keep paying for a
terminal per session, and for the agent runtime only once.

The client is a view in the sense that it owns no session state: it holds no row
in the database, and when the server was killed the clients reattached to its
replacement and carried on. It is not a thin terminal, though. At 213 MB it is a
full runtime of its own, drawing the interface and holding its own scrollback.

### How to set it up

```sh
opencode serve                      # once, listens on 127.0.0.1:4096
opencode attach http://127.0.0.1:4096   # in each project, as many as you like
```

sessionmap shows the server as the session row and each client as a view:

```
● opencode  56753   553M   27m35s   2h24m!   16k  serve   working on the refactor
● opencode  58111   223M   26m09s      -      -   attach  (view of 127.0.0.1:4096 — session runs in pid 56753)
● opencode  88167   212M    1m02s      -      -   attach  (view of 127.0.0.1:4096 — session runs in pid 56753)
```

### What you give up

- **`x` on the server drops every session's memory at once.** Memory becomes
  all-or-nothing: you cannot free the one session you have finished with while
  the others keep working. Standalone processes let you do exactly that.
- **The 213 MB per client is a floor, not a rate.** That is what a client costs
  with nothing open. Point one at a real session and it carries the
  conversation for display:

  | Context in the session | Client |
  |---:|---:|
  | none (bare client) | 213–267 MB |
  | 16k | 237 MB |
  | 62k | 346 MB |

  Two points only, both measured seconds after the client started, so treat the
  slope as indicative. The server did not move correspondingly, but its own
  figure drifts by ±50 MB on its own, so this does not prove the server holds
  nothing. Budget for clients that grow with the work they are showing.

Session state itself is durable, which cuts both ways. Killing the server
outright did not lose anything: the clients that had been attached to it
reconnected to the replacement on their own and carried on, because sessions
live in the SQLite database and the server reloads them. Convenient, but it also
means nothing prunes itself when you walk away. (Whether an agent turn that is
mid-flight survives losing its view was not tested.)

## A long conversation is expensive to have open, not to have

What makes an OpenCode client heavy is the number of stored history items it is
showing, not the size of the context it is carrying. Those are different things,
and only the first predicts memory:

| Session | Stored history | Context | Client |
|---|---:|---:|---:|
| A | 14 parts | 16k | 245 MB |
| B | 33 parts | 51k | 259 MB |
| C | **5,237 parts** | 62k | **351 MB** |

Session B carries three times the context of A for 14 MB more. Session C carries
barely more context than B but 159 times the history, and costs 92 MB more.

Most of that is the replay buffer — the history drawn back onto the screen so
you can scroll up through it. Opening session C three ways:

| | Client | Peak |
|---|---:|---:|
| `--mini` (full replay) | 390 MB | 611 MB |
| `--mini --replay-limit 50` | 329 MB | 386 MB |
| `--mini --no-replay` | 280 MB | 334 MB |

Turning the replay off saves 110 MB, and 277 MB at the peak. Against a bare
client at about 245 MB, roughly three quarters of what a long session costs is
screen history.

**There is no way to limit this in the normal TUI.** `--replay-limit` and
`--no-replay` both fail with `Error: --replay-limit requires --mini`, so the only
way to get the saving is to switch interfaces. Until that changes, the practical
rule is simply: do not leave a long past session open in a view you are not
reading. The history is safe in the database either way — you are paying to keep
it on screen.

(Measured inside `--mini`, so the relative effect is the finding; how many MB it
would save in the normal TUI is unknown. One session, one run per arm.)

## Sort by idle time, not by memory

`i` in the TUI, or `--json | jq 'sort_by(.idle_secs)'`.

Memory tells you which session is large; idle time tells you which one is
finished. A 600 MB session you are actively using is not the problem — a 400 MB
session you last touched nine hours ago is. Sorting by idle time puts the ones
you have genuinely forgotten at the top, and those are the ones where the whole
figure is reclaimable.

## Reproducing these numbers

```sh
footprint -p <pid> | grep phys_footprint    # what sessionmap reports on macOS
vmmap --summary <pid> | grep "WebKit Malloc"  # how much of it is JS heap
leaks <pid> | tail -2                        # native leaks (usually ~0)
```

`ps` RSS is not comparable: it hides pages the kernel compressed and counts
shared executable pages that the footprint excludes. On the same machine one
idle session read 218 MB of RSS against 1.4 GB of footprint, while a freshly
started one read 918 MB of RSS against 606 MB of footprint — wrong in both
directions.

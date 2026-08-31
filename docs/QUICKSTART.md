# Quickstart

Get from zero to "I found the session that was eating 2 GB and freed it" in about two minutes.

[日本語版](https://github.com/gospelo-dev/sessionmap/blob/main/docs/QUICKSTART_ja.md)

## 1. Install

```sh
cargo install gospelo-sessionmap
```

This installs the `sessionmap` command. Requires a Rust toolchain (`rustup`); if you don't have one, get it from <https://rustup.rs>.

To build from a checkout instead:

```sh
git clone https://github.com/gospelo-dev/sessionmap.git
cd sessionmap
cargo install --path .
```

## 2. Take a snapshot

```sh
sessionmap --once
```

You get one row per running coding-agent session:

```
 sessionmap  4 running  1.2G total RSS (incl. children)  2 busy  2 idle >30m
  AGENT       PID    MEM        UP    IDLE    CTX VIA     PROJECT           TITLE
● claude    51952   545M    20m14s      8s    50k cli     gospelo-dev       naming discussion
  opencode  48120   210M     1h30m  1h12m!    38k cli     webapp-ui         monthly report
  copilot   47311   190M     2h40m  2h05m!     -  vscode  api-migration     skill handoff
● codex     52007   160M     3m02s     15s    12k vscode  sessionmap        add quickstart
```

How to read it:

| Column | What it tells you |
|---|---|
| AGENT | Which harness: `claude`, `opencode`, `copilot`, `codex` |
| MEM | Memory the session holds (process + children) |
| IDLE | Time since the session last did anything. `!` means it passed the idle threshold (30 min by default) |
| CTX | Context tokens the session is carrying |
| PROJECT / TITLE | Where it was working and what it was doing |

The rows marked `!` are the stranded ones — sessions you probably forgot about that are still holding memory.

## 3. Free the stranded sessions

Open the interactive view:

```sh
sessionmap
```

Then:

1. Press `i` to sort by idle time (longest idle at the top).
2. Move the cursor with `j` / `k` (or `↑` / `↓`) onto a session you no longer need.
3. Press `x`, then `y` to confirm. The session receives SIGTERM and its memory is released.
4. Press `q` to quit.

Other keys: `m` sort by memory, `p` sort by project, `a` toggle stale entries, `r` refresh.

## 4. Tune it to your habits

```sh
sessionmap --idle-warn 15         # flag sessions idle for 15+ minutes
sessionmap --interval 5           # refresh the TUI every 5 seconds
sessionmap --all                  # also show stale registries (process already gone)
sessionmap --json | jq '.[] | select(.idle_secs > 3600)'   # script it
watch -n 5 --color sessionmap --once --color               # poor man's dashboard
```

## Where the data comes from

sessionmap only reads files each harness already writes (Claude Code's session registry and transcripts, OpenCode's SQLite DB, Copilot's lock files, Codex's state DB) plus the process table. It never modifies them. Set `CLAUDE_CONFIG_DIR`, `OPENCODE_DB`, `COPILOT_HOME`, `CODEX_HOME`, or `VSCODE_WORKSPACE_STORAGE` if yours live somewhere non-standard.

Full details, including per-harness caveats, are in the [README](https://github.com/gospelo-dev/sessionmap/blob/main/README.md).

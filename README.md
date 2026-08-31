# gospelo-sessionmap

[![Crates.io](https://img.shields.io/crates/v/gospelo-sessionmap?style=flat&labelColor=8B95A5&color=FF3670)](https://crates.io/crates/gospelo-sessionmap) [![License: MIT](https://img.shields.io/badge/license-MIT-2EA8FF?style=flat&labelColor=8B95A5)](https://github.com/gospelo-dev/sessionmap/blob/main/LICENSE) [![Rust 2024](https://img.shields.io/badge/rust-2024_edition-000000?style=flat&logo=rust&logoColor=white&labelColor=000000)](https://doc.rust-lang.org/edition-guide/rust-2024/) [![GitHub release](https://img.shields.io/github/v/release/gospelo-dev/sessionmap?include_prereleases&style=flat&labelColor=8B95A5&color=34C759)](https://github.com/gospelo-dev/sessionmap/releases)

![SessionMap — find the coding-agent sessions stranded on your machine](https://raw.githubusercontent.com/gospelo-dev/sessionmap/main/assets/hero.jpg)

A lightweight Rust TUI that lists every running coding-agent session — Claude Code, OpenCode, GitHub Copilot CLI, Codex — in one table, so you can spot the ones that were left behind and are quietly hogging memory.

On top of what `ps` gives you (PID / RSS / uptime), it overlays **which project the session was working on and what it was doing (title)**, **how long it has been idle**, and **how many context tokens it is holding**.

[日本語版 README](https://github.com/gospelo-dev/sessionmap/blob/main/README_ja.md) · [Quickstart](https://github.com/gospelo-dev/sessionmap/blob/main/docs/QUICKSTART.md)

## Supported platforms

| OS | Status |
|---|---|
| macOS | Fully supported (primary development platform) |
| Linux | Supported. `lsof` is used as a cwd fallback if present, but not required |
| Windows | Supported natively (`%USERPROFILE%` / `%APPDATA%` are used for agent and VS Code paths; `.exe` process names are recognized). Process cwd is not available on Windows, so Codex CLI rows attach to the most recent open CLI thread and OpenCode rows may show no session. Also works under WSL, where it sees WSL-side sessions only |

CI builds and runs `sessionmap --once` on all three.

## Install

```sh
cargo install gospelo-sessionmap   # from crates.io
```

Or from source:

```sh
cargo install --path .
```

Either way the command is `sessionmap`.

## Usage

```sh
sessionmap            # TUI (refreshes every 2s)
sessionmap --once     # print the table once and exit (pipe-friendly)
sessionmap --json     # JSON output
sessionmap --all      # also show stale registries (process already gone)
sessionmap --idle-warn 15 --interval 5
sessionmap --once --color | less -R   # keep colors when piping (default: color only on a TTY; NO_COLOR / --no-color disables)
watch -n 5 --color sessionmap --once --color
```

Example `--once` output:

```
   PID     MEM    TREE      UP    IDLE    CTX PROJECT                VIA      TITLE
 22387    339M    406M   9m13s    11s     66k sessionmap             cli      Session monitor
  6306    191M    237M   1h12m  1h08m!    38k internal-dashboard     vscode   Check session ID
  8757    177M    197M   1h06m 18m02s    616k api-migration          vscode   Write skill handoff doc
```

| Column | Meaning |
|---|---|
| MEM | RSS of the agent process itself |
| TREE | Total RSS including child processes (MCP servers, hooks, …) |
| UP | Time since the process started |
| IDLE | Time since the session transcript was last written. Marked `!` / yellow once it exceeds `--idle-warn` (default 30 min) |
| CTX | Input tokens of the last assistant turn (input + cache read + cache creation) |
| VIA | Where it was launched from (cli / vscode) |
| TITLE | custom title > AI title > first prompt > registry name, in that order |

### TUI keys

| Key | Action |
|---|---|
| `j` / `k`, `↑` / `↓` | Move selection |
| `m` / `i` / `u` / `p` | Sort by memory / idle / uptime / project (`s` cycles) |
| `a` | Show/hide stale entries (registry present, process gone) |
| `x` | Send SIGTERM to the selected session (confirm with `y`) |
| `r` | Refresh now |
| `q` / `Esc` | Quit |

## How it works

1. Agent processes are found in the process table (sysinfo) — RSS / CPU / start time. PID reuse is detected by comparing start times
2. The state files each agent already writes (registries, SQLite databases, lock files, transcripts) are read **read-only** and joined onto the processes to add title, idle time and token counts

The data source differs per agent — see the sections below.

## Claude Code

Claude Code sessions appear as **AGENT = claude**.

- `~/.claude/sessions/<pid>.json` — the registry each session writes (pid, sessionId, cwd, start time, entrypoint). `CLAUDE_CONFIG_DIR` is honored if set
- `~/.claude/projects/*/<sessionId>.jsonl` — title, first prompt, token counts. Only the appended tail is read on each refresh, so large transcripts stay cheap
- `claude` processes that have no registry entry are still picked up as "unregistered"

## OpenCode

[OpenCode](https://github.com/sst/opencode) processes (`opencode` / `opencode serve` / `opencode run`) appear in the same table as **AGENT = opencode**.

- Session data is read read-only from `~/.local/share/opencode/opencode.db` (SQLite). Override with `OPENCODE_DB`; `XDG_DATA_HOME` is respected
- OpenCode has no PID registry, so the **latest session whose directory matches the process cwd** is attached. If that session ended before the process started it is shown with `(last in dir)`
- IDLE comes from the session's `time_updated`; CTX is input + cache tokens of the last assistant turn
- `x` in the TUI works on OpenCode processes too

## GitHub Copilot CLI

[GitHub Copilot CLI](https://github.com/github/copilot-cli) (`copilot`) sessions appear as **AGENT = copilot**.

- `~/.copilot/session-state/<id>/inuse.<pid>.lock` is used as the PID registry (only live sessions hold this file). Override the location with `COPILOT_HOME`
- Title is `name` from `workspace.yaml` (falling back to the first user message in `events.jsonl`); branch, cwd, and launcher (cli / vscode) come from the same file
- IDLE is the mtime of `events.jsonl`; model is taken from the last `assistant.message`
- `copilot` runs as a thin wrapper plus the real process, so MEM is their sum and PID is the wrapper (`x` terminates both)
- CTX is `-` because Copilot does not record per-turn input tokens
- A lock left behind by a dead process shows up as stale under `--all` / `a`

### Copilot Chat in VS Code

The VS Code Copilot Chat extension has no process of its own — it runs inside the extension host — so it is shown as **one row per window** (AGENT = copilot / VIA = vscode).

- Windows are detected via `~/.copilot/ide/<uuid>.lock` written by the extension (extension-host PID and workspace folder)
- Chats are read from `~/Library/Application Support/Code/User/workspaceStorage/<hash>/chatSessions/*.jsonl`; the **most recent non-empty chat** supplies the title (customTitle > first message) and model. If several chats were touched within 24 h, `[+N chats/24h]` is appended. Linux `~/.config/Code` and Insiders are also searched; override with `VSCODE_WORKSPACE_STORAGE`
- IDLE is the mtime of the latest chat file
- MEM is the whole extension-host process tree (tsserver and other extensions included). Sessions already counted on their own row — e.g. Claude Code launched from the same extension host — are **excluded**, so nothing is double-counted
- PID is the extension host, so `x` restarts every extension in that window

## Codex

OpenAI Codex (the `codex` CLI and the `codex app-server` launched by the VS Code extension) appears as **AGENT = codex**.

- Thread data comes from the `threads` table in `~/.codex/state_5.sqlite` (override with `CODEX_HOME`)
- "Open threads" are those with a `~/.codex/thread-writer-locks/<thread_id>.lock`. The lock has no PID, so `app-server` is matched to threads under the same VS Code window's folder and the CLI to threads under the process cwd. Multiple matches show `[+N threads]`
- Title is `name` > `title` > first user message; CTX is `tokens_used` (cumulative); model and branch also come from the DB
- VIA is `vscode` (app-server) / `cli` / `exec`
- An app-server with no open thread is shown as `(codex app-server, no open thread)`

## License

MIT

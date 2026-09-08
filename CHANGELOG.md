# Changelog

All notable changes to this project are documented here. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and this project
adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

## [0.3.0] — 2026-09-08

### Changed

- **MEM and TREE report the physical footprint on macOS instead of RSS**, which
  is what Activity Monitor shows. macOS compresses the pages of a process
  nobody is touching, so an idle session's RSS falls while it still holds the
  memory — the metric was smallest for exactly the sessions this tool exists to
  find. One session idle for seven hours measured 218 MB of RSS against 1.4 GB
  of footprint, and sorting by memory ranked it below sessions in active use.
  Read through `proc_pid_rusage`, falling back to RSS when it is unavailable.
  Linux and Windows keep the value `sysinfo` reports, and the `--json` field
  names `rss_self` and `rss_tree` are unchanged.
- The header now names the metric in use: `total footprint` on macOS,
  `total RSS` elsewhere.

### Added

- **`opencode attach` clients are shown as views rather than sessions.** The
  session runs in the `serve` process they point at, so matching them against
  the working directory was inventing a title, a context size and an idle time
  they did not own. Such a row now names the server it belongs to and leaves
  those fields blank, and `x` warns that it closes the view without ending the
  session.
- A client started with `--session <id>` (or `-s`) states which session it is
  showing, so that session's title, context and idle time are shown for real.
  The id is looked up directly, so it works for sessions older than the
  recent-session window, and it is reserved so no other row claims it by working
  directory. A `--session` flag with its value missing is ignored rather than
  treated as a match.
- [`docs/TIPS.md`](https://github.com/gospelo-dev/sessionmap/blob/main/docs/TIPS.md) — measured guidance on running OpenCode as one
  server with several attached clients, on what makes a client heavy, and on
  reproducing the memory figures.

## [0.2.3] — 2026-08-31

### Fixed

- Strip the Windows extended-length path prefix (`\\?\`, `\\?\UNC\`) from the
  working directory read out of the Codex state database, so paths display and
  match correctly.

## [0.2.2] — 2026-08-31

### Fixed

- The Claude Desktop application is no longer reported as a Claude Code session.
  It is excluded by executable path (`AnthropicClaude`, `Claude.app`), and
  process-name matching is now case-sensitive apart from the `.exe` suffix, so
  macOS's `Claude` is not confused with `claude`.

## [0.2.1] — 2026-08-31

### Fixed

- Recognise `.exe` process names on Windows. All four harnesses were compared
  against bare names, so nothing was detected there.
- Codex CLI sessions attach to the most recently opened CLI thread when the
  process working directory is unavailable, which is always the case on Windows.

## [0.2.0] — 2026-08-30

### Added

- Windows and Linux support: `%USERPROFILE%` and `%APPDATA%` are used for agent
  and VS Code paths, `lsof` is confined to Unix, and sessions are terminated
  with `taskkill /T /F` on Windows, where `sysinfo`'s kill reports failure.
- Continuous integration across ubuntu-latest, macos-latest and windows-latest.

## [0.1.0] — 2026-08-30

First public release, as `gospelo-sessionmap`.

### Added

- One table of every running coding-agent session — Claude Code, OpenCode,
  GitHub Copilot CLI (and Copilot Chat in VS Code), Codex — with memory,
  uptime, idle time, context size, project and title.
- Each agent's own state files are read read-only and joined onto the process
  table; nothing is written.
- TUI with sorting by memory, idle time, uptime and project, and `x` to
  terminate a session; `--once` for a snapshot and `--json` for scripting.

[Unreleased]: https://github.com/gospelo-dev/sessionmap/compare/v0.3.0...HEAD
[0.3.0]: https://github.com/gospelo-dev/sessionmap/compare/v0.2.3...v0.3.0
[0.2.3]: https://github.com/gospelo-dev/sessionmap/compare/v0.2.0...v0.2.3
[0.2.2]: https://github.com/gospelo-dev/sessionmap/compare/v0.2.0...v0.2.2
[0.2.1]: https://github.com/gospelo-dev/sessionmap/compare/v0.2.0...v0.2.1
[0.2.0]: https://github.com/gospelo-dev/sessionmap/compare/v0.1.0...v0.2.0
[0.1.0]: https://github.com/gospelo-dev/sessionmap/releases/tag/v0.1.0

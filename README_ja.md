# gospelo-sessionmap

[![Crates.io](https://img.shields.io/crates/v/gospelo-sessionmap?style=flat&labelColor=8B95A5&color=FF3670)](https://crates.io/crates/gospelo-sessionmap) [![License: MIT](https://img.shields.io/badge/license-MIT-2EA8FF?style=flat&labelColor=8B95A5)](https://github.com/gospelo-dev/sessionmap/blob/main/LICENSE) [![Rust 2024](https://img.shields.io/badge/rust-2024_edition-000000?style=flat&logo=rust&logoColor=white&labelColor=000000)](https://doc.rust-lang.org/edition-guide/rust-2024/) [![GitHub release](https://img.shields.io/github/v/release/gospelo-dev/sessionmap?include_prereleases&style=flat&labelColor=8B95A5&color=34C759)](https://github.com/gospelo-dev/sessionmap/releases)

![SessionMap — 放置されたコーディングエージェントのセッションを見つける](https://raw.githubusercontent.com/gospelo-dev/sessionmap/main/assets/hero.jpg)

[English README](https://github.com/gospelo-dev/sessionmap/blob/main/README.md) · [クイックスタート](https://github.com/gospelo-dev/sessionmap/blob/main/docs/QUICKSTART_ja.md)

Claude Code / OpenCode / GitHub Copilot CLI / Codex などコーディングエージェントのセッションを一覧し、放置されてメモリを無駄に占有していないかをひと目で確認するための、軽量な Rust 製 TUI モニターです。
`ps` の出力（PID / RSS / 経過時間）に、**どのプロジェクトで何をしていたセッションか（タイトル）**、**最後に動いてからの放置時間**、**コンテキストのトークン数** を重ねて表示します。

## インストール

```sh
cargo install gospelo-sessionmap   # crates.io から
```

ソースから:

```sh
cargo install --path .
```

どちらでもコマンド名は `sessionmap` です。

## 使い方

```sh
sessionmap            # TUI（2秒ごとに更新）
sessionmap --once     # 1回だけ表で出力して終了（パイプ向け）
sessionmap --json     # JSON で出力
sessionmap --all      # 死んでいるレジストリ（stale）も表示
sessionmap --idle-warn 15 --interval 5
sessionmap --once --color | less -R   # パイプでも色を付ける（既定は TTY のときだけ色付き、NO_COLOR / --no-color で無効）
watch -n 5 --color sessionmap --once --color
```

`--once` の出力例:

```
   PID     MEM    TREE      UP    IDLE    CTX PROJECT                VIA      TITLE
 22387    339M    406M   9m13s    11s     66k sessionmap             cli      Claude Code セッションモニター
  6306    191M    237M   1h12m  1h08m!    38k pj_leader-retention    vscode   セッションID確認
  8757    177M    197M   1h06m 18m02s    616k code-review            vscode   Skill handoff 資料作成
```

| 列 | 意味 |
|---|---|
| MEM | claude プロセス自身の RSS |
| TREE | 子プロセス（MCP サーバー、hook など）を含めた RSS 合計 |
| UP | プロセス起動からの経過時間 |
| IDLE | セッションの transcript が最後に書かれてからの時間。`--idle-warn`（既定 30 分）を超えると `!` / 黄色 |
| CTX | 最後のアシスタント応答時の入力トークン数（input + cache read + cache creation） |
| VIA | 起動元（cli / vscode） |
| TITLE | custom-title > AI title > 最初のプロンプト > レジストリ名 の優先順 |

### TUI のキー

| キー | 動作 |
|---|---|
| `j` / `k`, `↑` / `↓` | 選択移動 |
| `m` / `i` / `u` / `p` | メモリ / 放置時間 / 起動時間 / プロジェクト でソート（`s` で順に切替） |
| `a` | stale（プロセスが死んでいるレジストリ）を表示/非表示 |
| `x` | 選択セッションに SIGTERM（`y` で確認） |
| `r` | 手動更新 |
| `q` / `Esc` | 終了 |

## 仕組み

1. `~/.claude/sessions/<pid>.json` — 各セッションが書くレジストリ（pid, sessionId, cwd, 起動時刻, entrypoint）
2. プロセステーブル（sysinfo）— RSS / CPU / 起動時刻。PID の再利用は起動時刻で照合して弾きます
3. `~/.claude/projects/*/<sessionId>.jsonl` — タイトル・最初のプロンプト・トークン数。追記分だけを差分読みするので大きな transcript でも軽量です

レジストリに無い `claude` プロセスも「unregistered」として拾います。`CLAUDE_CONFIG_DIR` を設定している場合はそれに従います。

## OpenCode 対応

[OpenCode](https://github.com/sst/opencode) のプロセス（`opencode` / `opencode serve` / `opencode run`）も同じ表に **AGENT = opencode** として並びます。

- セッション情報は `~/.local/share/opencode/opencode.db`（SQLite）を読み取り専用で参照します（`OPENCODE_DB` で場所を上書き可、`XDG_DATA_HOME` にも追従）
- OpenCode には PID レジストリが無いので、**プロセスの cwd と一致する directory の最新セッション**を紐づけます。プロセス起動より前に終わっていたセッションは `(last in dir)` を付けて表示します
- IDLE はセッションの `time_updated`、CTX は最後のアシスタント応答の input + cache トークン
- TUI の `x` は OpenCode プロセスにも使えます

## GitHub Copilot CLI 対応

[GitHub Copilot CLI](https://github.com/github/copilot-cli)（`copilot` コマンド）のセッションも **AGENT = copilot** として並びます。

- `~/.copilot/session-state/<id>/inuse.<pid>.lock` を PID レジストリとして使います（起動中のセッションだけが持つファイル）。`COPILOT_HOME` で場所を上書き可
- タイトルは `workspace.yaml` の `name`（無ければ `events.jsonl` の最初のユーザーメッセージ）、ブランチ・cwd・起動元（cli / vscode）も同ファイルから
- IDLE は `events.jsonl` の更新時刻、モデルは最後の `assistant.message`
- `copilot` は薄いラッパープロセス + 本体の 2 プロセス構成なので、MEM はその合計、PID はラッパー側（`x` で kill すると両方終了します）
- CTX は Copilot がターンごとの入力トークンを記録しないため `-` になります
- プロセスが死んでいるのにロックが残っている場合は `--all` / `a` で stale として見えます

### VS Code の Copilot Chat

VS Code 拡張の Copilot Chat は独立したプロセスを持たず拡張ホスト内で動くため、**ウィンドウ単位**で 1 行（AGENT = copilot / VIA = vscode）として表示します。

- Copilot 拡張が書く `~/.copilot/ide/<uuid>.lock`（拡張ホストの PID とワークスペースフォルダ）でウィンドウを検出
- チャットは `~/Library/Application Support/Code/User/workspaceStorage/<hash>/chatSessions/*.jsonl` から読み、**最新の中身のあるチャット**のタイトル（customTitle > 最初のメッセージ）とモデルを表示。24 時間以内に触ったチャットが複数あれば `[+N chats/24h]` を付けます。Linux の `~/.config/Code` と Insiders も探索、`VSCODE_WORKSPACE_STORAGE` で上書き可
- IDLE は最新チャットファイルの更新時刻
- MEM は拡張ホストのプロセスツリー全体（tsserver 等ほかの拡張も含む）。ただし同じ拡張ホストから起動された Claude Code など**別行で数えているセッションは除外**しているので二重計上はありません
- PID は拡張ホストなので、`x` で kill するとそのウィンドウの拡張がすべて再起動します

## Codex 対応

OpenAI Codex（CLI の `codex`、VS Code 拡張が起動する `codex app-server`）は **AGENT = codex** として並びます。

- スレッド情報は `~/.codex/state_5.sqlite` の `threads` テーブル（`CODEX_HOME` で場所を上書き可）
- 「開いているスレッド」は `~/.codex/thread-writer-locks/<thread_id>.lock` で判定。ロックに PID は無いので、`app-server` には同じ VS Code ウィンドウのフォルダ配下のスレッド、CLI にはプロセス cwd 配下のスレッドを紐づけます。複数あれば `[+N threads]`
- タイトルは `name` > `title` > 最初のユーザーメッセージ、CTX は `tokens_used`（累計）、モデル・ブランチも DB から
- VIA は `vscode`（app-server）/ `cli` / `exec`
- 開いているスレッドが無い app-server は `(codex app-server, no open thread)` と表示します

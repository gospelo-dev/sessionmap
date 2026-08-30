# claude-monitor

Claude Code のセッションが放置されていないかをひと目で確認するための、軽量な Rust 製 TUI モニターです。
`ps | grep claude` の出力（PID / RSS / 経過時間）に、**どのプロジェクトで何をしていたセッションか（タイトル）**、**最後に動いてからの放置時間**、**コンテキストのトークン数** を重ねて表示します。

## インストール

```sh
cargo install --path .
```

## 使い方

```sh
claude-monitor            # TUI（2秒ごとに更新）
claude-monitor --once     # 1回だけ表で出力して終了（パイプ向け）
claude-monitor --json     # JSON で出力
claude-monitor --all      # 死んでいるレジストリ（stale）も表示
claude-monitor --idle-warn 15 --interval 5
claude-monitor --once --color | less -R   # パイプでも色を付ける（既定は TTY のときだけ色付き、NO_COLOR / --no-color で無効）
watch -n 5 --color claude-monitor --once --color
```

`--once` の出力例:

```
   PID     MEM    TREE      UP    IDLE    CTX PROJECT                VIA      TITLE
 22387    339M    406M   9m13s    11s     66k claude-monitor         cli      Claude Code セッションモニター
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

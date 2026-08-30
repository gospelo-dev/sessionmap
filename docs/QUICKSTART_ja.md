# クイックスタート

インストールから「2 GB 食っていた放置セッションを見つけて解放する」まで、2 分ほどで到達できます。

[English](https://github.com/gospelo-dev/sessionmap/blob/main/docs/QUICKSTART.md)

## 1. インストール

```sh
cargo install gospelo-sessionmap
```

これで `sessionmap` コマンドが入ります。Rust ツールチェーン(`rustup`)が必要です。未導入なら <https://rustup.rs> から入れてください。

チェックアウトからビルドする場合:

```sh
git clone https://github.com/gospelo-dev/sessionmap.git
cd sessionmap
cargo install --path .
```

## 2. まず 1 回だけ表示してみる

```sh
sessionmap --once
```

動いているコーディングエージェントのセッションが 1 行ずつ並びます:

```
 sessionmap  4 running  1.2G total RSS (incl. children)  2 busy  2 idle >30m
  AGENT       PID    MEM        UP    IDLE    CTX VIA     PROJECT           TITLE
● claude    51952   545M    20m14s      8s    50k cli     gospelo-dev       naming discussion
  opencode  48120   210M     1h30m  1h12m!    38k cli     pj_leader         retention report
  copilot   47311   190M     2h40m  2h05m!     -  vscode  code-review       skill handoff
● codex     52007   160M     3m02s     15s    12k vscode  sessionmap        add quickstart
```

見方:

| 列 | 分かること |
|---|---|
| AGENT | どのハーネスか: `claude` / `opencode` / `copilot` / `codex` |
| MEM | セッションが確保しているメモリ(プロセス + 子プロセス) |
| IDLE | 最後に動いてからの時間。`!` は放置しきい値(既定 30 分)を超えたもの |
| CTX | 抱えているコンテキストのトークン数 |
| PROJECT / TITLE | どこで何をしていたか |

`!` の付いた行が「遭難者」です。忘れられたまま、メモリを持ち続けているセッションです。

## 3. 放置セッションを解放する

対話画面を開きます:

```sh
sessionmap
```

そのあと:

1. `i` を押して放置時間順にソート(長いものが上に来ます)
2. `j` / `k`(または `↑` / `↓`)で、もう要らないセッションにカーソルを合わせる
3. `x` → `y` で確定。セッションに SIGTERM が送られ、メモリが解放されます
4. `q` で終了

その他のキー: `m` メモリ順、`p` プロジェクト順、`a` stale 表示切替、`r` 手動更新。

## 4. 自分の使い方に合わせる

```sh
sessionmap --idle-warn 15         # 15 分以上放置で ! を付ける
sessionmap --interval 5           # TUI を 5 秒ごとに更新
sessionmap --all                  # プロセスが死んでいるレジストリ(stale)も表示
sessionmap --json | jq '.[] | select(.idle_secs > 3600)'   # スクリプトから使う
watch -n 5 --color sessionmap --once --color               # 簡易ダッシュボード
```

## データの出どころ

sessionmap は各ハーネスが元々書いているファイル(Claude Code のセッションレジストリと transcript、OpenCode の SQLite、Copilot のロックファイル、Codex の state DB)とプロセステーブルを**読むだけ**で、書き換えは一切しません。標準外の場所に置いている場合は `CLAUDE_CONFIG_DIR` / `OPENCODE_DB` / `COPILOT_HOME` / `CODEX_HOME` / `VSCODE_WORKSPACE_STORAGE` で指定できます。

ハーネスごとの詳細や注意点は [README](https://github.com/gospelo-dev/sessionmap/blob/main/README_ja.md) を参照してください。

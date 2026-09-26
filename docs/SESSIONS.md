# hbui Sessions: 複数アプリケーションとstdio専用bridge

`docs/MCP_BRIDGE.md` の構成を、複数のhbuiアプリケーションを同時に扱えるよう一般化したもの。
両者が食い違う箇所はこの文書を優先する。

## 変更点の要約

- `hbui-mcp-bridge` は **stdio専用** のMCP serverとする。HTTP transportは持たない。
- bridgeはMCPクライアント（エージェント）が起動する子プロセスになる。エージェントごとに1つ。
- そのため **applicationがlistenし、bridgeがscanする** 向きに反転した。
  bridgeが共有socketをlistenする設計では、2つ目のエージェントのbridgeが起動できない。

```text
agent A ──MCP(stdio)── bridge A ──┐                     ┌── commander  (session "commander")
                                  ├── $HBUI_DIR/*.sock ─┤
agent B ──MCP(stdio)── bridge B ──┘                     └── rs-voxeler (session "voxeler")
```

## 3つの識別子

```text
session  = logical application（socket名。再起動しても変わらない）
instance = そのsessionの何回目の起動か（再起動ごとに+1）
pid      = 現在のプロセス（補助的なselector）
```

PIDはprimary IDにしない。再build・再起動で変わるため。

## Session directory

```text
$HBUI_DIR  （既定: <temp dir>/hbui-$USER, mode 0700）
  commander.sock    application がlistenするsocket (0600)
  commander.lock    sessionの占有ロック兼instanceカウンタ
  voxeler.sock
  voxeler.lock
```

- applicationは起動時に `<session>.lock` を `try_lock` で取得する。
  ロックはプロセス終了時（crashを含む）にカーネルが解放するので、
  「sessionが使用中か」は古いsocketファイルに左右されない。
- ロック取得後、ファイル内のinstance番号を+1して書き戻す。
  instanceはapplication側で決まるので、どのbridgeから見ても同じ番号になる。
- ロックを取れたら残っている `<session>.sock` は死んだプロセスのものなので削除してbindする。
- 正常終了時はsocketを削除する。`.lock` はinstanceカウンタのため残す。

session名:

- `--hbui-session NAME` で指定できる。使用中ならエラーで終了する（勝手に別名にしない）。
- 指定しなければapplication名、使用中なら `name-2`, `name-3`, ... を使う。
- ファイル名になるので `[A-Za-z0-9_.-]` のみ。

## Handshake（hbui-ipc v2）

applicationが先に送る:

```json
{"protocol":"hbui-ipc","version":2,"application":"commander",
 "session":"commander","instance":7,"pid":4242,"cwd":"/Users/me"}
```

bridgeが返す:

```json
{"accepted":true}
```

versionが違えば `{"accepted":false,"error":"protocol_mismatch","bridge_version":2}`。
その後の `get_view` / `dispatch` / `capture_view` と、applicationからの
`view_changed` pushは `MCP_BRIDGE.md` の通り。1つのapplicationに複数のbridgeが同時に接続でき、
それぞれに同じ応答とpushが届く。

## Bridge

- 起動時と各tool呼び出し時、および250msごとにdirectoryをscanし、未接続のsocketへ接続する。
  応答しないsocketは死んだプロセスのものとして無視する。
- 一度見たsessionは、切断後も `disconnected` として一覧に残し、最後にpushされたviewを保持する。
- UI stateのownerではない。

## MCP API

```text
list_sessions()
get_view(session, since?, instance?)
dispatch(session, ...action, expected_instance?, expected_revision?)
capture_view(session, width?, height?)
```

- `session` の代わりに `pid` も使える（補助selector）。
- **current sessionは持たない。** 毎回targetを明示する。
  「select → 推論 → dispatch」の間に暗黙状態が変わる危険を避けるため。
- sessionを省略すると `session_required`、存在しなければ `unknown_session`。
  どちらも存在するsession名の一覧を返す。

`list_sessions` の例:

```json
{"sessions":[
  {"session":"commander","application":"commander","pid":1234,"cwd":"/Users/me",
   "instance":7,"status":"ready","revision":12}
]}
```

`status` は `ready` / `disconnected` / `protocol_mismatch`。

## 今後

- human向けCLI（`hbui list`, `hbui inspect <session>`, `hbui capture <session>`）。
  bridgeと同じ `hbui_ipc::Bridge` を使えば実装できる。
- session metadata（例: 開いているdocument名）をapplicationから追加できるようにする。

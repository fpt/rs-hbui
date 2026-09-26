# hbui MCP Bridge and Development Lifecycle

## 目的

`hbui` の開発中に、MCPクライアントと開発対象アプリケーションのライフサイクルを分離する。

MCPサーバーを開発対象バイナリへ直接埋め込むと、アプリケーションをビルド・再起動するたびにMCP接続まで失われる可能性がある。

その結果、

- AIエージェントを再起動する
- MCPサーバーへ再接続する
- セッションを再初期化する
- 現在のUI状態を再取得する

といった作業が発生し、開発体験が悪くなる。

HTTPベースのMCP transportで再接続できる場合もあるが、再接続やセッション復旧の挙動をMCPクライアント実装へ依存させない。

そのため `hbui` では、

> MCP connection lifecycle と Application lifecycle を分離する

ことを基本方針とする。

---

# 基本構成

MCPサーバーは開発対象アプリケーションとは別プロセスとして動作させる。

```text
AI Agent / MCP Client
        │
        │ MCP
        ▼
┌────────────────────────┐
│ hbui-mcp-bridge        │
│ long-lived process     │
└───────────┬────────────┘
            │
            │ local IPC
            ▼
┌────────────────────────┐
│ hbui application       │
│ short-lived process    │
└────────────────────────┘
```

`hbui-mcp-bridge` は長寿命プロセスとする。

開発対象アプリケーションは、

```bash
cargo run
```

やwatcherによって何度でも再起動してよい。

AIエージェントから見るMCP endpointは変化しない。

---

# 設計原則

## MCPはAdapterとする

MCPそのものを `hbui-core` の中核に置かない。

構造は次のようにする。

```text
MCP Client
    │
    ▼
hbui-mcp-bridge
    │
    │ hbui internal protocol
    ▼
Application
    │
    ▼
hbui-core
```

MCP固有の、

- transport
- session
- initialization
- reconnect
- protocol version

などの概念は、可能な限り `hbui-mcp-bridge` で吸収する。

アプリケーションや `hbui-core` はMCPを意識しない。

---

# アプリケーションとの通信

`hbui-mcp-bridge` とアプリケーションの間は、ローカルIPCを使用する。

初期実装では複雑なprotocolを作らない。

候補は、

- Unix Domain Socket
- localhost TCP socket
- named pipe
- JSON Lines
- length-prefixed JSON

など。

Unix系OSを最初の対象にする場合はUnix Domain Socketが単純でよい。

例:

```text
/tmp/hbui-dev.sock
```

内部protocolはMCPである必要はない。

例えば、

```json
{"id":1,"method":"get_view"}
```

に対して、

```json
{
  "id":1,
  "result":{
    "instance":27,
    "revision":12,
    "view":{
      "...":"..."
    }
  }
}
```

を返す程度でよい。

---

# 最初のRPC

bridgeとapplicationの間で必要なRPCは、初期段階では非常に少なくてよい。

```text
get_view
dispatch
```

Graphics Pane対応後は、

```text
capture_view
```

などを追加する。

MCP側のtoolも基本的にはこれをそのまま公開する。

```text
MCP get_view
    ↓
bridge
    ↓
IPC get_view
    ↓
application
```

---

# Application Instance ID

UI revisionとは別に、アプリケーションプロセスの世代を識別するIDを持つ。

例えば、

```text
instance=26 revision=183
```

の状態でアプリケーションを再起動した場合、

```text
instance=27 revision=0
```

とする。

`revision` は同一application instance内でのみ単調増加する。

`instance` はアプリケーションの再起動ごとに変わる。

---

# なぜInstance IDが必要か

AIが古いアプリケーション状態を見た後、その間にアプリケーションが再起動する可能性がある。

例えばAIが、

```json
{
  "type":"activate",
  "target":"left.files",
  "expected_instance":26,
  "expected_revision":183
}
```

を送ったとする。

現在のアプリケーションが、

```text
instance=27
```

なら、その操作は拒否する。

```json
{
  "error":"stale_instance",
  "current_instance":27
}
```

これにより、

> 同じrevision番号だが別プロセス

という曖昧さも避けられる。

---

# Revisionとの関係

操作の事前条件は、

```text
instance
+
revision
```

の組で扱う。

例えば、

```json
{
  "type":"select",
  "target":"left.files",
  "item":"documents",
  "expected_instance":27,
  "expected_revision":12
}
```

とする。

結果:

```json
{
  "ok":true,
  "instance":27,
  "revision":13
}
```

HumanとAIが同時に操作する場合も、従来のrevision conflict detectionをそのまま利用できる。

---

# アプリケーション停止中の挙動

アプリケーションが停止していても、`hbui-mcp-bridge` は停止しない。

`get_view` を呼ばれた場合は、MCP接続自体を失敗させるのではなく、明示的な状態を返す。

例:

```json
{
  "status":"application_unavailable",
  "application":{
    "connected":false
  }
}
```

これによりAIは、

- MCP自体が壊れた
- アプリケーションだけが停止している

という2つの状態を区別できる。

---

# 再接続

新しいアプリケーションプロセスが起動したら、同じIPC endpointへ接続する。

```text
old application
      ↓ exit

hbui-mcp-bridge
      ↓ remains alive

new application
      ↓ connect

hbui-mcp-bridge
      ↓ ready
```

AIエージェント側ではMCP接続を張り直さない。

これはMCP transportの再接続機能に依存しない。

---

# 最後のViewをキャッシュする

bridgeには、オプションとして最後に取得したsemantic viewを保持してもよい。

```text
Application
    ↓
semantic view
    ↓
hbui-mcp-bridge cache
```

アプリケーション停止中に、

```text
get_view
```

された場合、

```json
{
  "status":"application_disconnected",
  "stale":true,
  "last_view":{
    "instance":26,
    "revision":183,
    "...":"..."
  }
}
```

のように返せる。

ただし、このViewは現在状態ではないため、

```json
"stale":true
```

を必須とする。

AIが古いViewを現在状態と誤認しないようにする。

---

# キャッシュの目的

これは通常操作のためではなく、主にデバッグのために使う。

例えば、

- アプリケーションがpanicした
- GUI/CUIがクラッシュした
- AIが変更したコードで起動できなくなった

といった場合に、

> クラッシュ直前のsemantic UI state

をAI自身が確認できる。

これは、AIとの観測チャネルをアプリケーション障害から独立させるという点でも有用である。

---

# BridgeはApplication Supervisorにしない

初期段階では、`hbui-mcp-bridge` 自身が、

- build
- restart
- process supervision
- cargo watch

まで担当しない。

bridgeの責務は、

```text
MCP
↕
hbui internal protocol
```

の変換に限定する。

開発初期は例えば、

```bash
cargo run -p hbui-mcp-bridge
```

と、

```bash
cargo run -p commander
```

を別terminalで起動するだけでよい。

---

# 将来的な開発用Launcher

必要になれば、別途、

```text
hbui-dev
```

のようなツールを作る。

例えば、

```bash
hbui-dev cargo run -p commander
```

で、

```text
hbui-mcp-bridge start
        │
        ├── application start
        ├── watch
        └── restart
```

までまとめてもよい。

ただしこれは `hbui-mcp-bridge` とは別責務とする。

構造としては、

```text
hbui-dev
 ├── hbui-mcp-bridge
 └── application supervisor
```

のように考える。

---

# Watch / Rebuildとの組み合わせ

開発時の理想的な構成は次のようになる。

```text
                       file watcher
                            │
                            ▼
AI ──MCP── hbui-mcp-bridge ──IPC── application
                │                     ▲
                │                     │
                │                  restart
                │
                └── last view cache
```

コードを変更すると、

```text
source change
   ↓
build
   ↓
application restart
   ↓
new instance connects
```

となる。

MCP connectionは維持される。

---

# Error Model

bridgeから見える主な状態を明示的に定義する。

例:

```text
ready
application_unavailable
application_disconnected
protocol_mismatch
stale_instance
stale_revision
internal_error
```

MCP transport errorとapplication errorを混同しない。

例えばアプリケーションが落ちているだけなら、MCP tool自体は正常に応答する。

```json
{
  "status":"application_unavailable"
}
```

これをnetwork errorとして扱わない。

---

# Protocol Version

bridgeとapplication間の内部protocolにはversionを持たせる。

例:

```json
{
  "protocol":"hbui-ipc",
  "version":1
}
```

接続時にhandshakeする。

bridgeと新しくbuildしたapplicationのversionが一致しない場合は、

```json
{
  "error":"protocol_mismatch",
  "bridge_version":1,
  "application_version":2
}
```

と明示する。

開発中はbridge自体を再起動すべきケースもあるため、それを曖昧にしない。

---

# Handshake

アプリケーション接続時には最低限、

```json
{
  "protocol":"hbui-ipc",
  "version":1,
  "application":"commander",
  "instance":27
}
```

のような情報を送る。

bridgeは必要なら、

```json
{
  "accepted":true
}
```

を返す。

この段階で、

- protocol version
- application identity
- instance ID

を確定する。

---

# Instance IDの生成

Instance IDは必ずしも永続化する必要はない。

重要なのは、bridgeが接続中のアプリケーション世代を一意に識別できることである。

実装例としては、

- bridgeが接続ごとにmonotonic counterを割り当てる
- applicationが起動時にUUIDを生成する

などが考えられる。

初期実装ではbridge側のmonotonic counterで十分。

```text
connection #1 -> instance 1
connection #2 -> instance 2
connection #3 -> instance 3
```

とする。

---

# MCP Transportに依存しない

MCP transportとして、

```text
stdio
Streamable HTTP
その他
```

のいずれを使う場合でも、application lifecycleをそれに依存させない。

重要なのは、

```text
MCP transport lifecycle
        ↓
hbui-mcp-bridge

Application lifecycle
        ↓
hbui application
```

と分離することである。

Streamable HTTPが透過的な再接続を提供する場合でも、それは最適化であって設計上の前提にしない。

---

# stdio MCPの場合

MCPクライアントがstdio transportを要求する場合でも、同じ構造を維持できる。

```text
AI Agent
   │
 stdio
   │
hbui-mcp-bridge
   │
  IPC
   │
application
```

つまりbridge自体はMCPクライアントの子プロセスとして起動されてもよい。

重要なのは、開発対象applicationがbridgeとは別プロセスであること。

---

# HTTP MCPの場合

HTTPを使用する場合は、

```text
AI Agent
    │
 HTTP
    ▼
hbui-mcp-bridge
 localhost
    │
   IPC
    ▼
application
```

とする。

この場合、bridgeを長時間常駐させやすい。

ただしapplication restart recoveryをMCPクライアントのHTTP reconnect機能へ依存させない。

---

# Security

初期段階ではdevelopment-only用途を想定する。

HTTP MCP endpointを使う場合は、デフォルトではlocalhostにのみbindする。

例:

```text
127.0.0.1
::1
```

外部interfaceへ自動的に公開しない。

IPC socketにも適切なpermissionを設定する。

---

# Graphics Paneへの拡張

将来 `rs-voxeler` などでGraphics Paneを扱う場合も、同じbridgeを利用する。

Semantic View:

```json
{
  "id":"viewport",
  "role":"graphics",
  "document_revision":128,
  "camera":{
    "...":"..."
  }
}
```

必要な場合だけ、

```text
capture_view("viewport")
```

を呼ぶ。

Application側で画像を生成し、bridge経由でMCP clientへ返す。

構造は、

```text
MCP capture_view
      ↓
hbui-mcp-bridge
      ↓
IPC
      ↓
application
      ↓
renderer / viewport capture
```

とする。

bridge自身はgraphics rendererを持たない。

---

# rs-voxelerでの想定構成

将来的には、

```text
rs-voxeler
 ├── document model
 ├── hbui-core
 ├── GUI renderer
 └── hbui IPC endpoint

hbui-mcp-bridge
 └── MCP tools
```

のようにできる。

rs-voxeler自体をMCP serverにしない。

この構成なら、rs-voxelerがクラッシュしてもMCP connectionは生きたままになる。

---

# Crate構成案

例えば、

```text
hbui-core
hbui-terminal
hbui-ipc
hbui-mcp-bridge
```

とする。

Applicationは、

```text
hbui-core
+
hbui-ipc
```

を利用する。

bridgeは、

```text
hbui-ipc
+
MCP implementation
```

を利用する。

依存関係として、

```text
hbui-core
    ↑
application

hbui-ipc
   ↑     ↑
app    bridge
```

とし、`hbui-core` がMCPへ依存しないようにする。

---

# 初期実装範囲

最初は以下だけでよい。

## hbui-ipc

- Unix Domain Socket
- simple request/response
- JSON serialization
- handshake
- instance ID
- get_view
- dispatch

## hbui-mcp-bridge

- MCP server
- get_view tool
- dispatch tool
- application connection state
- stale instance detection
- latest semantic view cache

## Example Application

- hbui IPC endpointへ接続
- semantic viewを返す
- dispatchを処理する
- restartしてもbridgeが生き続けることを確認する

---

# 最初に確認するシナリオ

以下をPoCの成功条件とする。

1. AI Agentが `hbui-mcp-bridge` に接続する
2. Applicationがbridgeへ接続する
3. AIが `get_view` を呼べる
4. AIが `dispatch` を呼べる
5. Applicationを終了する
6. MCP connectionは維持される
7. `get_view` が `application_unavailable` を返す
8. Applicationを再buildして起動する
9. Applicationが自動的にbridgeへ再接続する
10. Instance IDが変わる
11. AIは再び `get_view` と `dispatch` を利用できる
12. 古いinstanceに対する操作は `stale_instance` になる
13. AI AgentやMCP clientを再起動する必要がない

---

# 非目標

初期実装では以下を行わない。

- MCP client固有のreconnect behaviorへの最適化
- 分散システム向けRPC
- remote application control
- service discovery
- authentication infrastructure
- automatic build system
- process supervisor
- hot code reload
- application state migration
- application restart後のrevision継続
- bridgeによるUI state所有

BridgeはUI Stateのownerではない。

UI StateのSingle Source of Truthは常にApplication側にある。

---

# 最重要原則

`hbui-mcp-bridge` はUI applicationそのものではない。

役割は、

> AIとの長寿命な通信チャネルと、短寿命なApplicationプロセスの間を安定して橋渡しすること

である。

最終的な構造は、

```text
              AI
              │
             MCP
              │
       ┌──────▼──────┐
       │ MCP Bridge  │
       │ long-lived  │
       └──────┬──────┘
              │
         hbui IPC
              │
       ┌──────▼──────┐
       │ Application │
       │ short-lived │
       └──────┬──────┘
              │
          hbui-core
              │
           Renderer
              │
            Human
```

とする。

この分離により、

```text
AI session lifetime
≠
Application process lifetime
```

を実現する。

これを `hbui` の開発体験における基本設計とする。

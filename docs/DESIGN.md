# AIフレンドリーなCUI/MCPライブラリ設計方針

## 目的

Rustで、AIと人間が同一のUI状態を共有できるCUIライブラリを作る。

最初の対象はCUIとMCPのみとする。設計に問題がないことを確認できたらGUIへ拡張し、最終的には `fpt/rs-voxeler` などでも利用できる構成を目指す。

このライブラリの中心的な考え方は次の通り。

> Pixels are a rendering detail. The UI is a structured interactive document.

AIに画面をOCRやスクリーンショットとして理解させるのではなく、人間向けCUIとAI向けMCP Viewの両方を、同一のUI状態モデルから生成する。

---

## 背景

既存のCUI/GUIライブラリ、たとえばBubble TeaやeguiをAIに操作・実装させると、画面状態の一貫性が崩れやすい。

典型的には以下の問題が起きる。

- AIが「直した」と判断しているが、実際の画面では直っていない
- 内部状態と表示状態がずれる
- AIが現在のフォーカスや選択状態を誤認する
- 人間が画面状態を自然言語で説明しないとAIが問題を認識できない
- AIが座標やキー操作を推測する必要がある
- マルチウインドウやfloating UIでは、どのコンテキストを操作しているか不明瞭になる

これを避けるため、UIを「画面」ではなく「構造化されたinteractive document」として扱う。

---

# 基本アーキテクチャ

最初は以下の3層に分ける。

```text
Application Model
       │
       ▼
    ui-core
       │
       ├── ui-cui
       │      └── Terminal
       │
       └── ui-mcp
              └── AI
```

将来的には、

```text
ui-core
 ├── ui-cui
 ├── ui-mcp
 ├── ui-gui
 └── ui-web
```

のように拡張できる構造を目指す。

重要なのは、

```text
GUI/CUI
  ↓
AI向け画面解析
```

ではなく、

```text
              UI State
             /        \
            /          \
       CUI Renderer   MCP View
          Human         AI
```

とすることである。

---

# UI StateをSingle Source of Truthにする

画面状態はすべてserializableなUI Stateとして保持する。

例:

```rust
struct UiState {
    revision: u64,
    panes: Vec<Pane>,
    focus: FocusId,
    modal: Option<Modal>,
}
```

RendererやMCPはこの状態を読むだけとする。

Terminal上の描画結果やcursor位置を状態の正としない。

---

# PaneベースのUI

最初は自由度の高いGUIを扱わない。

UI構造は以下に限定する。

```text
Screen
 ├── Pane
 ├── Pane
 ├── ...
 └── Modal?
```

レイアウトは主に次のものだけ扱う。

```text
Horizontal Split
Vertical Split
Tabs
Modal
```

floating windowや任意座標配置は扱わない。

入力コンテキストの優先順位も単純化する。

```text
Modal
  >
Focused Pane
  >
Screen
```

これによって、人間とAIの双方が現在の操作対象を明確に認識できるようにする。

---

# 最初のWidget Set

最初から汎用GUI toolkitを作ろうとしない。

最低限以下だけを実装する。

```text
Text
List
Tree
Input
Button / Action
Pane
Split
Modal
```

必要になったら後から、

```text
Table
Viewer
Tabs
Graphics
```

などを追加する。

---

# Semantic UI Tree

Widgetは可能な限りsemanticな構造を持つ。

すべての主要Widgetに、少なくとも次の属性を持たせる。

```text
id
role
label
value
state
actions
```

例:

```json
{
  "id": "left.files",
  "role": "tree",
  "label": "Files",
  "state": {
    "selected": "documents"
  },
  "actions": {
    "select": {
      "parameters": {
        "item": "item_id"
      }
    }
  }
}
```

Stable IDを非常に重視する。

AIは座標ではなくIDを使って操作する。

悪い例:

```json
{
  "click": {
    "x": 32,
    "y": 14
  }
}
```

良い例:

```json
{
  "type": "select",
  "target": "left.files",
  "item": "documents"
}
```

---

# AI向けView

AIには画面のテキストダンプではなく、構造化されたUI状態を返す。

たとえばNorton Commander風のUIなら、

```json
{
  "revision": 42,
  "focus": "left.files",
  "panes": [
    {
      "id": "left",
      "role": "tree",
      "selected": "documents",
      "items": [
        {
          "id": "root",
          "label": "/root"
        },
        {
          "id": "documents",
          "label": "/root/Documents"
        },
        {
          "id": "download",
          "label": "/root/Download"
        }
      ]
    },
    {
      "id": "right",
      "role": "tree",
      "selected": "temp",
      "items": [
        {
          "id": "temp",
          "label": "/temp"
        }
      ]
    }
  ]
}
```

人間向けには同じ状態を、

```text
┌──────── Left ────────┬──────── Right ───────┐
│ > Documents          │ > /tmp               │
│   Downloads          │                       │
│   src                │                       │
│                      │                       │
└──────────────────────┴───────────────────────┘
```

のように描画する。

---

# MCP API

最初のMCP APIはできるだけ小さくする。

基本は次の2つでもよい。

```text
get_view()
dispatch(action)
```

`get_view()` は現在のsemantic UI stateを返す。

`dispatch()` はsemantic actionを実行する。

例:

```json
{
  "type": "select",
  "target": "left.files",
  "item": "documents",
  "revision": 42
}
```

返り値:

```json
{
  "ok": true,
  "revision": 43,
  "changes": [
    {
      "path": "/panes/left/selection",
      "value": "documents"
    }
  ]
}
```

---

# AIにはキー操作よりSemantic Actionを使わせる

AI向け操作APIは、

```text
press_key("ArrowDown")
type_text("abc")
```

を中心にしない。

基本的には次のようなsemantic actionを使う。

```text
focus
select
activate
set_text
invoke
open_modal
close_modal
```

キー入力は人間向けCUI backendの詳細とする。

必要であればdebug用として低レベルのキー操作APIを追加してもよいが、通常のAI操作では使わない。

---

# 人間とAIでは入力デバイスが異なってよい

人間はキーイベントを使う。

```text
Key('a')
Key('b')
Key('c')
Backspace
Left
```

AIはsemantic actionを使う。

```json
{
  "type": "set_text",
  "target": "rename.filename",
  "value": "abc.txt"
}
```

最終的な`TextInput` stateは共通とする。

人間とAIが同じ操作方法を使う必要はない。

重要なのは、同じstateを見て同じstateを更新することである。

---

# Revision管理

UI Stateには必ずrevisionを持たせる。

```rust
struct UiState {
    revision: u64,
    // ...
}
```

AIが操作するときには、可能なら観測したrevisionを送る。

```json
{
  "type": "select",
  "target": "left.files",
  "item": "documents",
  "expected_revision": 42
}
```

その間に人間がUIを操作してrevisionが変わっていた場合、

```json
{
  "error": "stale_view",
  "current_revision": 43
}
```

を返す。

Human + AIで同時にUIを操作した場合でも、古い状態に基づく操作を防止する。

---

# Diff

将来的には、毎回全UI stateを返す必要はない。

初回:

```json
{
  "revision": 120,
  "screen": {
    "...": "..."
  }
}
```

次回:

```text
get_view(since=120)
```

返り値:

```json
{
  "revision": 121,
  "changes": [
    {
      "op": "replace",
      "path": "/panes/left/selection",
      "value": "download"
    }
  ]
}
```

これによってAI自身が、

- 操作によって何が変化したか
- 操作が成功したか
- 期待した状態になったか

を確認できる。

操作後のdiffが空なら、AIはその操作が有効でなかった可能性を認識できる。

---

# CUI Backend

最初はCrosstermなど既存のterminal abstractionを薄く使う。

ただし、Crosstermの型やイベントを`ui-core`に露出させない。

Backend側で入力を正規化する。

例:

```rust
enum InputEvent {
    Key(Key),
    Text(String),
    Paste(String),
    Resize {
        cols: u16,
        rows: u16,
    },
}
```

将来backendを変更しても`ui-core`には影響しない構造にする。

---

# Terminalを賢く扱わない

このプロジェクトの目的は高度なterminal emulator対応ではない。

IME、Unicode、terminal固有bugなどに実装コストを吸われないようにする。

基本方針は、

> Terminalには描画と生入力だけを任せ、賢さはserializableなUI stateに置く。

とする。

---

# Cursor管理

Cursor位置はsemantic stateとして保持する。

例えばTextInputなら、

```rust
struct TextInput {
    id: WidgetId,
    text: String,
    cursor: TextCursor,
    selection: Option<TextRange>,
    focused: bool,
}
```

Terminal上の`(x, y)`はRendererが毎フレーム計算する。

UI Stateにはscreen coordinateを保持しない。

---

# IME対応

IMEを自前実装しない。

OSおよびterminal emulatorが提供するIMEをなるべく邪魔しない方針を採る。

特にText Inputにfocusがある場合は、

- terminal cursorを表示する
- 実際に文字が挿入されるセルにcursorを置く
- cursor位置を不用意に飛ばさない
- 入力中に過剰な全画面再描画をしない

ことを基本とする。

通常のListやTree操作中はterminal cursorをhideしてもよい。

---

# Terminal右端の安全マージン

特にmacOSのTerminal.appでは、IME入力中にcursorが画面右端まで到達すると不安定になるケースがある。

これに対して完全な回避策を実装することは、プロジェクトの主目的ではない。

そのため単純な安全策を採る。

例えば、

```rust
let safe_width = terminal_width.saturating_sub(1);
```

として、editable textのcursorを最終列に置かない。

TextInputでは右端に1〜2セル程度の安全マージンを持たせる。

```text
│ input text here█  │
                 ^^
              safety margin
```

「すべてのterminal edge caseを解決する」のではなく、

> 怪しい場所にcursorを置かない

ことで回避する。

---

# Unicode

Unicode処理は最低限必要だが、最初から完全対応を目指さない。

少なくとも以下を混同しない。

```text
UTF-8 byte index
Unicode scalar value
grapheme cluster
terminal cell width
```

Cursorはbyte offsetでは持たない。

例えば、

```rust
struct TextCursor {
    grapheme: usize,
}
```

のようにgrapheme単位で管理する。

表示幅の計算には`unicode-width`相当を使う。

grapheme分割には`unicode-segmentation`相当を使う。

ただし初期段階では、

- BiDi
- 複雑なcombining sequence
- 高度なUnicode shaping

などを完全に解決しようとしない。

---

# Surface

CUI Rendererには中間表現としてCell Surfaceを持たせる。

```rust
struct Cell {
    grapheme: String,
    style: Style,
}

struct Surface {
    width: u16,
    height: u16,
    cells: Vec<Cell>,
}
```

描画パイプラインは、

```text
UiState
  ↓
Layout
  ↓
Surface
  ↓
Diff with previous Surface
  ↓
Terminal
```

とする。

これによって描画結果をsnapshot testできる。

---

# Semantic SnapshotとVisual Snapshotを分離する

テストには2種類のsnapshotを用意する。

## Semantic Snapshot

```json
{
  "revision": 42,
  "focus": "left.files",
  "panes": [
    "..."
  ]
}
```

AIが見る状態と同じ。

## Visual Snapshot

```text
┌──────── Left ────────┬──────── Right ───────┐
│ > Documents          │ > /tmp               │
│   Downloads          │                       │
└──────────────────────┴───────────────────────┘
```

またはCell Surface。

これによって、

- 内部状態が正しいか
- 人間に正しく描画されているか

を独立に確認できる。

---

# Terminal Guard

Raw modeやalternate screenを使用するため、terminal cleanupは最優先で実装する。

例:

```rust
struct TerminalGuard;

impl Drop for TerminalGuard {
    fn drop(&mut self) {
        // show cursor
        // leave alternate screen
        // disable bracketed paste
        // disable raw mode
    }
}
```

panic hookも設定し、

```text
panic
 ↓
terminal restore
 ↓
error output
```

となるようにする。

ライブラリのbugによって利用者のshellが壊れたように見える状態を避ける。

---

# 初期crate構成

まずは以下のような構成を想定する。

```text
aiui-core
 ├── state
 ├── widget
 ├── layout
 ├── action
 └── snapshot

aiui-terminal
 ├── crossterm backend
 ├── cell surface
 ├── diff renderer
 ├── input normalization
 └── cursor placement

aiui-mcp
 ├── get_view
 └── dispatch

examples
 └── commander
```

名称は仮。

---

# 最初のサンプルアプリ

Norton Commander風の2-pane file browserを作る。

```text
┌──────── Left ────────┬──────── Right ───────┐
│ > Documents          │ > /tmp               │
│   Downloads          │                       │
│   src                │                       │
│                      │                       │
├──────────────────────┴───────────────────────┤
│ F5 Copy   F6 Move   F8 Delete               │
└──────────────────────────────────────────────┘
```

最初のPoCでは最低限以下を確認する。

1. Tabでpane focusを移動できる
2. 上下キーでList/Treeを選択できる
3. Enterでactivateできる
4. Modalを開閉できる
5. Rename Modalなどで日本語入力できる
6. MCPから現在のsemantic stateを取得できる
7. MCPからsemantic actionを実行できる
8. Human操作とAI操作を交互に行える
9. Revision conflictを検出できる
10. Semantic Snapshotをテストできる
11. Visual Snapshotをテストできる

---

# GUIへの拡張

CUI版が成立した場合、`ui-core`を変更せず、GUI Rendererを追加する方向とする。

例えば、

```text
aiui-core
aiui-terminal
aiui-mcp
aiui-egui
```

または、

```text
aiui-wgpu
```

など。

GUIでもsemantic UI StateをSingle Source of Truthとする。

---

# Graphics Pane

`rs-voxeler`のようなアプリでは3D viewportなどのGraphics Paneが必要になる。

Graphics Paneを通常のWidgetと同様にsemantic treeへ含める。

例:

```json
{
  "id": "viewport",
  "role": "graphics",
  "camera": {
    "...": "..."
  },
  "document_revision": 128,
  "actions": [
    "orbit",
    "pan",
    "zoom",
    "select"
  ]
}
```

ただし3D viewportの視覚情報はJSONだけでは不足する。

そのため別途、

```text
capture_view("viewport")
```

のようなMCP toolを用意し、画像を返せるようにする。

重要なのは、

> 画像をAI向けViewそのものにしない

ことである。

基本Viewは常にsemantic structureとし、画像は必要な場合のみ取得するvisual observationとする。

```

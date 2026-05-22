# fulgur-da3u: CounterPass 注入セレクタの specificity 再構成

## 背景

`fulgur-2ykw` で、複数アイテムの要素 `::before`/`::after` generated content
を `CounterPass` 経由でレンダリングするゲートを修正した。これにより
AssetBundle / `<link>` CSS と、タグ / クラスセレクタを使う inline `<style>`
は全アイテムが描画されるようになった。

しかし inline `<style>` ブロックには別の、先行して存在する根本原因が残る。

## 根本原因

`CounterPass` は解決済みの擬似要素 content を
`[data-fulgur-cid="N"]::before/::after{content:"..."}` という形で注入する。
属性セレクタの specificity は `(0,1,0)`、擬似要素を足して `(0,1,1)`。

- **AssetBundle / `<link>` CSS**: 元の author 宣言は DOM に文字列として
  入らない（`engine::render_pass` が `parse_gcpm().cleaned_css` を
  `InjectCssPass` で注入する）。カスケード競合は発生しない。
- **inline `<style>`**: `extract_gcpm_from_inline_styles` は mapping を
  **抽出するだけ**で、`<style>` テキストは DOM に残り Stylo がそのまま
  パースする。これは意図的で、`RunningElementPass`（`position:running()`）
  など他パスが元の inline `<style>` テキストの残存に依存しているため。

結果、生き残った author ルールが注入ルールとカスケード競合する。タグ
セレクタ `(0,0,1)` は注入 `(0,1,1)` に負ける（正常動作）が、ID セレクタ
`(1,0,0)` — もしくは specificity が `(0,1,0)` 以上の任意の author
セレクタ、あるいは `!important` — は注入に**勝ち**、Blitz が
items[0] 切り詰め版をレンダリングしてしまう。

### 検証済みエビデンス（fulgur-2ykw 調査より）

`{h2, #s} × {::before, ::after}` のマトリクスで
`content:"[" counter(c) "]"` を inline `<style>` に置く。タグセレクタは
`[1]`/`[2]` を完全描画。ID セレクタは先頭の `[` のみ描画。

### 不採用となった先行案

各 inline `<style>` を `parse_gcpm().cleaned_css` + Blitz
`upsert_stylesheet_for_node` で書き換える実装は、狙ったケースは直るが
`gcpm_snapshot` テスト（`gcpm_running_element_via_inline_style`,
`gcpm_element_policy_first/last`）を回帰させた。`cleaned_css` は
`position:running()` / `@page` も除去し、`RunningElementPass` は元の
inline `<style>` テキストの残存に依存するため。AssetBundle と
inline-`<style>` の経路は設計上非対称（DOM presence・パス順序が異なる）で、
一括書き換えは不可。

## 採用方針: セレクタ再構成

注入セレクタに**対象要素自身の compound を前置**して specificity を
引き上げる。`!important` も CSS テキスト編集も使わない。

```css
/* 現状 */   [data-fulgur-cid="3"]::after{content:"[1][2]"}
/* 変更後 */ h2#s[data-fulgur-cid="3"]::after{content:"[1][2]"}
```

### なぜマッチ対象が変わらないか

`data-fulgur-cid` は `CounterPass` が content mapping にマッチした要素
ごとに採番する一意属性。`[data-fulgur-cid="N"]` 単独で対象要素を確定
させる。前置する `tag#id.class` は「その同じ要素について真である事実」
を足すだけなので、マッチする要素は変わらず specificity だけが上がる。

### specificity 効果

- `h2#s[data-fulgur-cid="N"]::after` = `(1,1,2)` は author `#s::after`
  `(1,0,1)` に勝つ。
- 全クラスを盛り込むことで `.a.b.c::after` `(0,3,1)` のような
  クラス多用ルールも被覆する。要素を直接対象とする単一 compound の
  author ルールは（後述の「既知の限界」を除き）被覆できる。
- specificity が同点の場合は source order で決まる。`InjectCssPass`
  が生成する `<style>` ノードは DOM パース後に作られ最大の node_id を
  持つため、`add_stylesheet_for_node` のオーダリング（node_id 昇順）で
  最後に置かれ、同点は注入側が勝つ。

### 既知の限界（文書化する）

`#main #s::after` のような **ID 持ち祖先セレクタ** `(2,0,2)` には
負けうる。要素自身の compound からは祖先の specificity を再現できない
ため。同じバケツに、疑似クラスを積み重ねた単一 compound
（`#s:not(.x):not(.y)::after` = `(1,2,1)`）も入る — 再構成した
`(1,1,2)` を上回る。いずれも acceptance criteria のスコープ（単一
compound `#id`）外。実ケースが出たら follow-up issue で扱う。

## 実装

変更は `CounterPass::walk_tree`（`crates/fulgur/src/blitz_adapter.rs`）
内に閉じる。

### 1. `element_specificity_prefix`

新ヘルパー。要素の `tag` + `#id`（あれば）+ `.class`（全クラス、属性
トークン順）を連結して返す。cid 割り当て直後に一度だけ計算し、
`::before` / `::after` 両方の解決ブロックで再利用する（要素の id/class
は子要素再帰の間に変化しないため安全）。

### 2. `css_escape_ident`

新ヘルパー。id を `#<ident>` で出すには CSS identifier エスケープが
必須。既存の `css_escape_string` は文字列リテラル用で別物。CSSOM
`CSS.escape` 相当のアルゴリズムを実装し、id と各クラスに適用する。
タグ名は HTML タグ名なのでエスケープ不要。

### 3. 注入箇所

`::before` / `::after` の `write!` で、`[data-fulgur-cid="{cid}"]` の
前にプレフィックスを差し込む。

### AssetBundle 経路への影響

`CounterPass` は両経路で共有される。AssetBundle 経路では注入セレクタが
`#id[data-fulgur-cid]::before` に変わるが、`data-fulgur-cid` は一意な
ので依然として対象要素のみにマッチし、競合相手も存在しない（cleaned_css
が author ルールを除去済み）。よって correctness は変わらず、CSS が
わずかに長くなるだけ。経路ごとの特別扱いは不要。

## テスト

### ユニットテスト（`blitz_adapter.rs` の `#[cfg(test)] mod tests`）

- 新規: id 持ち要素の content mapping を `CounterPass` に与え、
  `generated_css()` が `#s[data-fulgur-cid=` を含むことを assert。
- 新規: `css_escape_ident` の境界テスト（数字始まり・特殊文字・空文字
  など CSSOM アルゴリズムのエッジ）。
- 既存修正: `generated_css` を exact 比較するテスト群は、対象要素に
  id/class があれば期待値が変わる。期待文字列を更新する。

### end-to-end smoke テスト（`crates/fulgur/tests/render_smoke.rs`）

- 新規: inline `<style>` の `#id::before { content: <multi-item> }`
  をレンダリングし、全アイテムが PDF テキストレイヤに出ることを
  `hex_utf16be` で assert。
- ガードテスト昇格: `target_text_after_resolves_counter_via_counter_pass`
  を no-panic ガードから実 `[2]` テキストレイヤ assertion へ格上げ。
  fulgur-2ykw の acceptance criterion #4 をここで満たす。

### 回帰防止

- `cargo test -p fulgur --test gcpm_snapshot` の running-element 系
  （`gcpm_running_element_via_inline_style`,
  `gcpm_element_policy_first/last`）が green のまま。本案は inline
  `<style>` テキストを一切触らないので原理上影響しないが、失敗実装で
  踏んだ地雷なので明示的に確認する。
- `cargo test -p fulgur` 全体 + `cargo clippy` + `cargo fmt --check`。

## Acceptance

- inline `<style>` の `#id::before/::after { content: <multi-item> }`
  が全アイテムを PDF テキストレイヤへ描画する。
- `gcpm_snapshot` の running-element テストが green のまま。
- `target_text_after_resolves_counter_via_counter_pass` を no-panic
  ガードから実 `[2]` テキストレイヤ assertion へ昇格できる
  （fulgur-2ykw acceptance criterion #4、ここへ deferred）。

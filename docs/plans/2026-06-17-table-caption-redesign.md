# テーブル `<caption>` 描画対応 — 再設計 (fulgur-78o)

- 日付: 2026-06-17
- 対象 issue: fulgur-78o
- 旧設計: 2026-03-21 作成（v1 `Pageable` 前提）。**全面的に無効化し本ドキュメントで置き換える**。

## 1. 背景と調査結果

### 1.1 現状の挙動（実測）

`<table><caption>…</caption>…</table>` をレンダリングすると、**caption のテキストは
PDF に一切出力されない**。テーブル本体（thead / tbody / セル）は正常に描画される。

```text
入力: <table><caption>CAPTION_MARKER_TEXT</caption><thead>…</thead><tbody>…</tbody></table>
pdftotext 出力: H1 H2 / a1 / b1 / a2 / b2   ← caption は欠落
```

### 1.2 根本原因 — Blitz がテーブル caption をレイアウトしない

`convert/table.rs` で caption ノードを `convert_node` に通すよう改造して計測したところ:

```text
CAPTION DEBUG: id=10 inline_root=false size=Size{0.0,0.0} loc=Point{0.0,0.0}
  CAPTION CHILD: id=11 data=Text size=Size{0.0,0.0} loc=Point{0.0,0.0}
```

caption ノードもその子テキストも **size=0×0 / loc=0,0**。`display:block` を指定しても変わらない。

原因は blitz-dom の table box 構築にある:

```rust
// blitz-dom-0.2.4 src/layout/table.rs:226（0.3.0-alpha.5 でも src/layout/table.rs:316 で同一）
DisplayInside::Flow | FlowRoot | Flex | Grid => {
    node.remove_damage(CONSTRUCT_DESCENDENT | CONSTRUCT_FC | CONSTRUCT_BOX);
    // Probably a table caption: ignore
}
```

`<table>` の子のうち table-internal でない display を持つもの（= caption を含む）は
box 構築段階で破棄される。したがって **caption は Taffy に渡らず、geometry を一切持たない**。
これは 0.2.4 / 0.3.0-alpha.5 共通の upstream 制約。0.3 の `construct.rs` にある `caption`
参照は `all_inline` 判定用の enum match だけで、実レイアウトはしない。

### 1.3 旧設計が無効な理由

旧設計は次を前提にしていたが、いずれも現状と乖離している:

- **v1 `Pageable` ツリー**（`TablePageable` に `caption` フィールド追加、`wrap/split/draw`
  をオーバーライド）→ v1 は廃止済み。現在は `Drawables` + `PaginationGeometryTable` の
  geometry 駆動 v2 パス。
- **「caption を保持すれば配置できる」前提** → Blitz が caption に box を与えないため、
  そもそも配置すべき geometry が存在しない。手動レイアウトが必須になる。

## 2. 設計方針

### 2.1 採用案: レイアウト前 DOM 再構成パス (approach B)

caption を `<table>` の外に出し、`table` + `caption` を `width:fit-content` の
ブロック wrapper で包む。これにより Blitz が caption を**通常のブロックフロー box として
レイアウト**し、以降の convert / paginate / render が**無改造で**処理する。

実験（手で再構成した DOM を未改造 fulgur に通す）で次を確認済み:

| ケース | 結果 |
|--------|------|
| caption がテーブルより狭い | caption テキスト描画・テーブル幅で配置 ✓ |
| caption がテーブルより広い | `fit-content` がページ幅で頭打ち→折り返し。caption がテーブルより広くなる軽微な乖離のみ ✓ |

得られる無償の恩恵:

- **改ページ越え**: caption は実 box なので既存 fragmenter が処理（プロジェクト要件）
- **背景・枠線・パディング**: `block_styles` 経由で既存描画
- **インラインテキスト・画像入り caption**: `inline_root` / `block` の通常経路

### 2.2 不採用案

- **(A) fulgur 内で手動レイアウト（Parley でシェイプ→合成 geometry 注入→全セルを
  caption 高さ分オフセット→分割時にページごとに再オフセット）**: v2 の「Taffy が一度だけ
  geometry を計算」原則に正面から反する。背景・枠線・改ページも全て手書きになり、approach B
  が無償で得るものを全部自前実装することになる。却下。
- **(C) upstream Blitz 修正のみ**: 0.3 でも未対応の真の upstream gap であり、別途
  tracking issue を立てる価値はある（Blitz contributor 路線と整合）。ただし 0.2.4 で
  今すぐ動く解にはならないため、本 issue の主経路にはしない。
- **(D) multicol 型の layout hook**: 「multicol と同様、Blitz がレイアウトしないものを
  fulgur 側の Taffy hook（`FulgurLayoutTree` / `compute_root_layout` + `propagate_height_delta`）で
  計算する」案。**原理的に不可**として却下。理由は介入点が違う:
  - multicol は子を Blitz が **box 構築済み**（inline root フラグ・テキストシェイピング・
    `TextLayout` まで完成）で、hook は **位置決め（layout）だけ**を再実行して段組みに再配分する。
  - caption は Blitz が **box 構築段階で殺している**（`document.rs:1072-1091` /
    `construct.rs:647-671` の table 構築が `remove_damage(CONSTRUCT_BOX | CONSTRUCT_DESCENDENT
    | CONSTRUCT_FC)`）。結果、inline root フラグが立たず `deferred_construction_nodes`
    （テキストシェイピングのタスクキュー）にも積まれない → **テキストが一切シェイプされない**。
  - layout は「box 構築済み」を前提とする後段工程なので、構築を飛ばした caption に
    `compute_root_layout` を当てても 0×0・テキストなしのまま。
  - 構築を fulgur から強制する公開 API は無い（`deferred_construction_nodes` は `pub(crate)`、
    damage 駆動）。damage を再付与しても再 resolve でテーブル構築が再度殺す（caption は
    依然テーブルの子）。
  - 結論: caption に必要なのは layout hook ではなく **restructure hook**。採用案 (B) の
    `DomPass` がそれであり、再構成後は Blitz が construct+layout を自前で行うため、multicol が
    手動でやる `propagate_height_delta` 相当（兄弟の押し下げ）も **Taffy の通常フローが無償で**
    こなす。layout hook より結果的にコードが少ない。

## 3. 実装スケッチ

### 3.1 新規 `CaptionRestructurePass`（`DomPass` 実装）

`blitz_adapter.rs` の既存パス（`InjectCssPass` 等）と同じ機構。mutator API は揃っている:
`create_element` / `append_children` / `insert_nodes_before` / `replace_node_with` /
`set_style_property` / `child_ids`。

擬似コード:

```text
apply(doc):
  captions = scan tree for <caption> elements whose parent is <table>
  if captions empty: return            // 注: caption 無し文書はゼロコスト

  # caption-side を読むため一度だけ resolve（下記 3.2 参照）
  resolve(doc)

  for (caption_id, table_id) in captions:
     side = computed caption-side of caption_id   # top | bottom
     wrapper = create_element("div")
     set_style_property(wrapper, "display", "block")
     set_style_property(wrapper, "width", "fit-content")
     replace_node_with(table_id, [wrapper])       # wrapper を table の位置へ
     set_style_property(caption_id, "display", "block")  # table-caption を無効化
     if side == top:  append_children(wrapper, [caption_id, table_id])
     else:            append_children(wrapper, [table_id, caption_id])
```

`<caption>` 要素自体はリネームしない（`caption { … }` の著者 CSS が要素名で当たり続ける）。
ID / アンカーも保持。

### 3.2 核心的な実装判断: `caption-side` をいつ読むか

全 `DomPass` は engine.rs:348 の `resolve()` **前**に走る。よってパス時点では
`caption-side` の computed style は未解決（参照できるのは要素名・属性・インライン style のみ）。
`resolve()` は cascade + Taffy layout を含むフル解決（`doc.resolve(0.0)`）。

選択肢:

1. **採用: caption 存在時のみパス内で先行 `resolve()`** — `<caption>` を含む文書だけ
   余分な resolve を 1 回払い、computed `caption-side` を読んで再構成する。engine.rs:348 の
   resolve が再構成後のツリーを再解決するので整合は崩れない。caption 無し文書は木スキャンだけで
   ゼロコスト。**これを推奨**。
2. top のみ対応し `caption-side:bottom` は既知制限（インライン style からだけ bottom を拾う等）
   → 旧設計の段階的スコープと同じ発想だが、先行 resolve で正しく解けるなら 1 を優先。

> 実装時の確認事項: `resolve()` をパス内で呼んだ直後に mutator で再構成し、その後に
> engine の resolve が再度走る二重 resolve が blitz-dom 0.2.4 で安全か（box tree の
> 再構築が正しく走るか）を最初の TDD で潰すこと。

### 3.3 wrapper 幅の挙動

`width:fit-content` は `min(max-content, max(min-content, available))`。available は
ページ content 幅。

設計時はこう予測していた:

- caption ≤ テーブル幅 → wrapper = テーブル幅、caption がそれを埋める（理想）
- caption > テーブル幅 → wrapper はページ幅で頭打ち、caption 折り返し、テーブルは自幅のまま
  上寄せ。

> **実装後の訂正（PR #487 で実測）**: 上記の「wrapper = テーブル幅」は**実際には起きない**。
> 狭い（例 `width:260px`）テーブルでも合成 wrapper は実質ページ幅まで広がり、caption 背景は
> テーブル幅に縮まらない（理想とは逆方向の乖離）。一方で **この全幅化のおかげで**、内側
> テーブルの `margin:0 auto` センタリングと `width:100%` 全幅は caption 有無で非回帰になる
> （§7・§4.2.1）。つまり 3.3 の理想（caption をテーブル幅に縮める）を実装すると
> センタリング非回帰の根拠が崩れるトレードオフがある。詳細と追跡は fulgur-vdd1。

## 4. スコープ

### 4.1 v1 で対応

- 単一 caption、`caption-side: top`（既定）/ `bottom`
- caption の背景・枠線・パディング・インラインテキスト・画像
- caption を含むテーブルの改ページ（caption 自身が実 box なので既存経路で動作）

### 4.2 v1 非対応 / フォローアップ

- caption 幅をテーブル min-content まで伸ばす厳密な仕様準拠（3.3 の乖離）
- 複数 caption（仕様上は許容だが稀。最初の 1 つのみ扱う）
- `<colgroup>` / `<col>`（無関係だが調査中に未対応と判明。別 issue 候補）
- upstream Blitz への caption レイアウト実装（別 tracking issue / contributor 路線）

#### 4.2.1 synthetic wrapper が table-level の box セマンティクスを引き継がない（fulgur-vdd1）

approach B は `<table>` を fit-content の `<div>` wrapper で「置換」して caption を
その中へ移すため、CSS の table wrapper box 相当の外側 box が **合成 div** になる。
このため、**caption を持つテーブルに限って** 次が崩れる（PR #487 のレビューで網羅的に
洗い出し）。いずれも real だが niche で、まとめて fulgur-vdd1 で追跡:

- `position: absolute` / `fixed`、`float`、`transform`、`opacity`（合成 div ではなく内側
  table に残る。`opacity` は内側 table だけが淡くなり caption は不透明のまま）
- flex / grid item placement（`flex` / `order` / `align-self` / `grid-column` が table に残る）
- 非 auto margin（`margin-top` / `margin-left` 等の固定値）が内側 table に残り、wrapper には
  乗らない。top caption がインデントされず、margin が caption と grid の間に入る。**これは
  §4.2.1 末尾の全幅 wrapper 機序と表裏一体**で、auto margin（センタリング）が動くのと同じ
  理由で固定 margin は wrapper に移せない（移すとセンタリングが回帰する）トレードオフ
- 親スコープ・兄弟・子孫セレクタ（`body > table.report` / `table + p` / `table > caption`）が
  wrapper 挿入で不一致になる
- `display: inline-table`（合成 div が block のため別行に押し出される）
- フラグメンテーション（`break-inside: avoid` / `break-before: page` が内側 table にしか効かず
  caption が分離しうる）
- GCPM: `caption-side: bottom` で caption が table の後ろに移動し、後続の
  `CounterPass` / `StringSetPass` が見る DOM 順が変わる。`position: running()` の table を
  `RunningElementPass` がシリアライズする時点で caption は既に外に出ている
- PDF/UA: `tagging::classify_element` に caption case が無く marked content が開かれない
- 合成 div が author の `div { … }` ルールに当たりうる（緩和候補: div 以外の wrapper 要素名）
- fit-content wrapper は狭いテーブルでも実質ページ幅まで広がり、caption 背景がテーブル幅に
  縮まらない（3.3 の理想とは逆方向の乖離。ただしこの全幅化のおかげで `margin: 0 auto`
  センタリングと `width: 100%` 全幅は実測で **非回帰**——下記 §7 参照）

**不採用の緩和策**: gemini レビューが提案した「table の `style` / `class` を wrapper へ
コピー」は、margin の二重適用・背景の二重描画・class セレクタの wrapper/table 両当たりの
リスクがあり却下。共通ケース（センタリング・全幅）は実測でコピー無しに動くため、
各制限は fulgur-vdd1 で追跡し本節に明記する方針。

#### 4.2.2 cascade による非表示の尊重 + caption-side ゲート（fulgur-uoao、対応済み）

旧実装は caption に inline `display: block` を無条件で強制し、`caption-side` も無条件で
反映していたため、次が崩れていた（いずれも実測で確認）。pre-resolve 済みの computed
style を読んで修正済み:

- **`display: none` の漏れ**: `caption { display: none }`（著者 CSS）や
  `table { display: none }` 配下の caption が PDF に漏れていた。table または caption が
  `display: none` → restructure を skip（caption は table 配下のまま Blitz に drop され、
  非表示が保たれる）。
- **`visibility: hidden` の漏れ**: `table { visibility: hidden }` ではセルは正しく
  非表示になるのに caption だけが visible な wrapper へ移って漏れていた（不整合な
  half-hidden）。table **または caption** が `visibility: hidden` / `collapse` →
  restructure を skip。caption 側は `table > caption { visibility: hidden }` のように
  **移動で外れる子セレクタ**でも漏れるが、cascade を移動前に読むため pre-resolve 時点
  （caption はまだ table の子でセレクタ一致）で正しく検出できる。
- **display 上書きのゲート**: caption の `display: block` 強制は UA 既定（`table-caption`）の
  ときだけ（著者の明示 display は温存）。
- **`caption-side` のゲート**: `caption-side` は `display: table-caption` にのみ適用される
  ため、著者が caption を非 table-caption display に上書きした場合は `caption-side` を
  無視し、caption は source 順（table の上）を保つ（`on_bottom = force_block && …`）。

検証: `caption_restructure_tests` の単体テスト（`display_is_none_*` /
`visibility_is_hidden_*` / `caption_display_is_table_caption_*`）と render_smoke の
`table_caption_display_none_is_not_rendered` /
`table_caption_hidden_table_does_not_leak_caption` /
`table_caption_visibility_hidden_table_does_not_leak_caption` /
`table_caption_visibility_hidden_caption_does_not_leak` /
`table_caption_side_ignored_for_non_table_caption_display`。

## 5. テスト計画

CLAUDE.md の coverage 規約に従い VRT と lib 側の両方を置く:

- **VRT (`crates/fulgur-vrt`)**: caption-top / caption-bottom / 長文 caption（折り返し）/
  背景付き caption / 改ページをまたぐ captioned table の golden PDF。
- **lib smoke (`crates/fulgur/tests/render_smoke.rs`)**: 各ケースを
  `Engine::builder().build().render_html(html)` で `assert!(!pdf.is_empty())`。
- **pdftotext によるテキスト存在チェック**: caption テキストが PDF に含まれること
  （現状はドロップされる回帰の検知）。
- **`CaptionRestructurePass` の単体テスト**: 再構成後の DOM 構造（wrapper の子順序が
  caption-side に一致）と二重 resolve の安全性。

## 6. 参考

- 根本原因: `blitz-dom-0.2.4/src/layout/table.rs:226`（0.3.0-alpha.5: `:316`）
- 現状の caption 無視経路: `crates/fulgur/src/convert/table.rs:54-60`
  （`collect_table_cells` が caption を「コンテナ」として扱い中身もインラインルートに
  ならず欠落）
- DomPass 機構: `crates/fulgur/src/blitz_adapter.rs:171`（trait）/ `:351`（`apply_passes`）/
  engine.rs:234, :348（パス→resolve 順序）
- mutator API: `blitz-dom-0.2.4/src/mutator.rs`
- v2 アーキテクチャ: CLAUDE.md（`Drawables` / `PaginationGeometryTable` / geometry 駆動）

## 7. 実装メモ（2026-06-17 実装完了）

- `CaptionRestructurePass`（`DomPass`）を `blitz_adapter.rs` に実装。`engine.rs` で
  **`InjectCssPass` の後**に push（pass の pre-resolve が engine / `AssetBundle` 注入 CSS の
  `caption-side` も読めるようにするため。document `<style>`/`<link>` だけだと取りこぼす）。
- `caption-side` は caption 存在時のみの先行 `resolve()` で computed style から読み、
  wrapper の子順序（top→`[caption, table]` / bottom→`[table, caption]`）に反映。
- `width:fit-content` wrapper の懸念（全幅テーブルの縮小）は実測で**非回帰**を確認:
  `width:100%` テーブルは caption 有無で右列 x が不変（全幅維持）、`margin:0 auto` テーブルも
  cell 位置が caption 有無で不変（センタリング維持、PR #487 で 225.4pt 不変を再計測）。
  - **機序の訂正（PR #487）**: 当初これを「Taffy が wrapper↔テーブル幅の循環を正しく解いている」
    と書いたが、実際は逆で **合成 wrapper が fit-content にもかかわらず実質ページ幅まで広がる**
    ため、内側テーブルが全幅 wrapper の中で従来どおりセンタリング/全幅化される、という機序。
    狭いテーブルでも wrapper は縮まない（§3.3 の訂正・§4.2.1）。この全幅化と centering 非回帰は
    表裏一体で、片方を「直す」と他方が回帰する（fulgur-vdd1 に記録）。
- テスト（`crates/fulgur/tests/render_smoke.rs`）:
  `table_caption_text_renders_in_pdf` / `table_caption_side_top_renders_above_table` /
  `table_caption_side_bottom_renders_below_table` / `table_caption_side_bottom_via_injected_css` /
  `table_caption_with_nested_inline_renders` / `table_with_two_captions_does_not_panic` /
  `table_caption_preserves_full_width_table` /
  `table_caption_display_none_is_not_rendered` / `table_caption_hidden_table_does_not_leak_caption`
  （後ろ 2 つは fulgur-uoao の回帰ガード）。
- pure helper の単体テスト（`blitz_adapter::caption_restructure_tests`）:
  `collect_caption_tables` の直下 caption のみ収集・nested table・`MAX_DOM_DEPTH` 上限、
  `caption_side_is_bottom` の top/bottom 判定、`display_is_none` /
  `caption_display_is_table_caption` を網羅（coderabbit 指摘の coverage 規約対応）。
- 描画は既存の block/paragraph 経路を通り新 draw arm は無いため codecov は render_smoke で充足するが、
  CLAUDE.md の VRT+smoke 規約に合わせ VRT golden も追加:
  `crates/fulgur-vrt/fixtures/layout/table-caption-top.html`（caption-side:top・背景付き caption・
  width:100% テーブル）+ `goldens/fulgur/layout/table-caption-top.pdf`。

### 7.1 WPT coverage

`css/CSS2/tables/caption-side-applies-to-{001,003,005,016}` を
`expectations/lists/caption.txt` に PASS として追跡（`scripts/wpt/subset.txt` に
テスト + ref を追加）。これらは「`caption-side` が `table-caption` 以外の display に
効かない」ことを検証する reftest で、再構成パスが非 `<caption>` 要素を触らないため通る。
同ディレクトリの他の caption reftest（caption-position-001 等）は block / table-cell
レイアウトや外部画像など本機能と無関係な理由で現状 FAIL するため、意図的に未追跡。

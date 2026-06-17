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

- caption ≤ テーブル幅 → wrapper = テーブル幅、caption がそれを埋める（理想）
- caption > テーブル幅 → wrapper はページ幅で頭打ち、caption 折り返し、テーブルは自幅のまま
  上寄せ。CSS 仕様（テーブル幅が caption の min-content まで伸びる）とは軽微に乖離するが
  実用上許容。v1 の既知挙動として明記する。

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
  cell 位置が caption 有無で不変（センタリング維持）。Taffy が wrapper↔テーブル幅の循環を
  正しく解いている。
- テスト（`crates/fulgur/tests/render_smoke.rs`）:
  `table_caption_text_renders_in_pdf` / `table_caption_side_top_renders_above_table` /
  `table_caption_side_bottom_renders_below_table` / `table_caption_side_bottom_via_injected_css` /
  `table_caption_with_nested_inline_renders` / `table_with_two_captions_does_not_panic` /
  `table_caption_preserves_full_width_table`。
- 描画は既存の block/paragraph 経路を通り新 draw arm は無いため、codecov は render_smoke で充足。
  VRT golden は未追加（視覚回帰の保険として将来追加可）。

### 7.1 WPT coverage

`css/CSS2/tables/caption-side-applies-to-{001,003,005,016}` を
`expectations/lists/caption.txt` に PASS として追跡（`scripts/wpt/subset.txt` に
テスト + ref を追加）。これらは「`caption-side` が `table-caption` 以外の display に
効かない」ことを検証する reftest で、再構成パスが非 `<caption>` 要素を触らないため通る。
同ディレクトリの他の caption reftest（caption-position-001 等）は block / table-cell
レイアウトや外部画像など本機能と無関係な理由で現状 FAIL するため、意図的に未追跡。

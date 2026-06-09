# Nested abs-driven pagination (fulgur-puml) Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** `position:absolute` 要素（特に abs 内 abs = nested abs）が height/offset で
ページ領域を超えるとき、CSS Paged Media のフラグメント化に従って追加ページを生成し、
それを fixed-repeat 要素にも伝播させる。WPT `fixedpos-004/005/006-print` を PASS にする。

**Architecture:** 現状 `pagination_layout::append_position_absolute_body_direct_fragments` は
body 直下 abs のみを `record_subtree_fragments_at_offset` で walk するが、内側 walk が
OOF 子孫を `continue` で skip するため nested abs が処理されない（原因①）。また
`may_extend_pages = !body_has_in_flow_content` のため in-flow 併存時に abs がページ拡張
できない（原因②）。engine.rs はすでに abs パス後に `implied_page_count` を再読して
`append_position_fixed_fragments` を再実行する拡張ループを持つので、abs パスが
`implied_page_count` を正しく伸ばせば fixed-repeat への伝播（trap B）は自動で成立する。

**Tech Stack:** Rust, Stylo (computed position insets), `pagination_layout.rs` の
`walk()` 再帰 + `Fragment` / `descendant_total_pages` / `may_extend_pages`。

---

## 背景 (実測済の事実 — 着手前に再確認不要)

- 原因① 最小再現: `<body><div style="position:absolute"><div style="position:absolute;height:300vh">x</div></div></body>` → 1 ページ (期待 3)
- 原因② 最小再現: `<body>text<div style="position:absolute;bottom:-200vh">x</div></body>` → 1 ページ (期待 3)。in-flow 無し版は 3 ページ
- 既存 PASS (regression guard): `monolithic-overflow-013`, `fixedpos-001/002/003/008-print`
- vh は viewport 相対 (CB 非依存)。`top:300vh` は常に 3×viewport
- nested abs の root 空間 y = 外側 abs の解決 y + 内側 abs の CB 相対解決 y
- ページ数 assert は `crates/fulgur/tests/abs_positioned_pagination.rs` の `page_count(&pdf)` ヘルパー
  (`/Type /Page` カウント) を流用。`@page { size: 100pt 100pt; margin:0 }` で 100vh = 1 ページ

## 検証ラダー (原因帰属のため順守)

1. Fix #1 単独 → fixedpos-004 と nested 最小再現が green になるはず (004 body は in-flow 無し →
   may_extend は既に true)。ここで lib suite + WPT regression guard 確認
2. その後 Fix #2 追加 → fixedpos-005/006 と in-flow+abs 最小再現が green。full WPT + lib suite 確認

---

### Task 1: RED — nested abs と in-flow+abs のページ数特性テスト

**Files:**

- Modify (append): `crates/fulgur/tests/abs_positioned_pagination.rs`

**Step 1: Write the failing tests**

`abs_positioned_pagination.rs` 末尾に追加（`page_count` / `Engine` / `PageSize` / `Margin` は既存 import）:

```rust
/// fulgur-puml 原因①: nested abs (abs 内 abs) の高さがページ数を駆動する。
/// 内側 abs `height:300vh` → 3 ページ。外側 abs は in-flow を持たない。
#[test]
fn nested_abs_height_drives_page_count() {
    let html = r#"<!doctype html><html><head><style>
        @page { size: 100pt 100pt; margin: 0; }
        body { margin: 0; }
    </style></head><body>
      <div style="position:absolute;">
        outer
        <div style="position:absolute; top:0; height:300vh; width:50px;"></div>
      </div>
    </body></html>"#;
    let engine = Engine::builder()
        .page_size(PageSize { width: 100.0, height: 100.0 })
        .margin(Margin::uniform(0.0))
        .build();
    let pdf = engine.render_html(html).expect("render");
    assert_eq!(page_count(&pdf), 3, "nested abs height:300vh must drive 3 pages");
}

/// fulgur-puml 原因②: in-flow コンテンツがあっても abs はページを拡張できる。
/// abs `bottom:-200vh` → 3 ページ (in-flow は 1 ページ分の text)。
#[test]
fn abs_extends_pages_despite_in_flow_content() {
    let html = r#"<!doctype html><html><head><style>
        @page { size: 100pt 100pt; margin: 0; }
        body { margin: 0; }
    </style></head><body>
      in-flow text here
      <div style="position:absolute; bottom:-200vh;">x</div>
    </body></html>"#;
    let engine = Engine::builder()
        .page_size(PageSize { width: 100.0, height: 100.0 })
        .margin(Margin::uniform(0.0))
        .build();
    let pdf = engine.render_html(html).expect("render");
    assert_eq!(page_count(&pdf), 3, "abs bottom:-200vh must extend to 3 pages even with in-flow content");
}

/// fulgur-puml trap A: nested abs の offset は CB 基準で解決される。
/// 外側 abs `top:100vh` + 内側 abs `top:300vh` → 内側は 400vh (page 5) に着地。
#[test]
fn nested_abs_offset_resolves_against_cb_not_flow() {
    let html = r#"<!doctype html><html><head><style>
        @page { size: 100pt 100pt; margin: 0; }
        body { margin: 0; }
    </style></head><body>
      <div style="position:absolute; top:100vh;">
        outer on page 2
        <div style="position:absolute; top:300vh;">inner on page 5</div>
      </div>
    </body></html>"#;
    let engine = Engine::builder()
        .page_size(PageSize { width: 100.0, height: 100.0 })
        .margin(Margin::uniform(0.0))
        .build();
    let pdf = engine.render_html(html).expect("render");
    assert_eq!(page_count(&pdf), 5, "inner abs top:300vh under outer top:100vh must land on page 5 (400vh)");
}
```

**Step 2: Run to verify they fail**

Run: `cargo test -p fulgur --test abs_positioned_pagination nested_abs_height_drives_page_count abs_extends_pages_despite_in_flow_content nested_abs_offset_resolves_against_cb_not_flow 2>&1 | tail -20`
Expected: 3 つとも FAIL（おそらく page_count=1）。期待値とのズレを記録。

**Step 3: Commit (red)**

```bash
git add crates/fulgur/tests/abs_positioned_pagination.rs
git commit -m "test(pagination): red tests for nested-abs and in-flow+abs page extension"
```

---

### Task 2: Fix #1 — nested abs を訪問し CB 基準 offset で extent を集計

**Files:**

- Modify: `crates/fulgur/src/pagination_layout.rs`
  - `record_subtree_fragments_at_offset` の内側 `walk`（OOF skip する `if is_out_of_flow_positioned(child) { continue; }` ≈ L3050-3055）

**Step 1: 設計メモ（trap A 厳守）**

内側 walk が nested abs 子に出会ったとき、in-flow 子のように `offset_in_subtree += child.final_layout.location`
で単純加算してはいけない。nested abs の位置は **その CB（最寄り positioned 祖先 = 現在の subtree root の box）**
に対して `resolve_viewport_cb_location(child, child_w, child_h, cb_w, cb_h)` で解決する。

- `cb_w` / `cb_h` = nested abs の CB box の寸法。第一候補は外側 abs（= 現 walk の `node`）の
  `final_layout.size`。ただし vh ベース inset (`top:300vh`) は CB 非依存なのでこの寸法は
  パーセンテージ inset のみに効く。
- 解決した CB 相対 (x, y) に、現在の accumulated root-space offset（= この nested abs の親 = `node` の
  root 空間位置）を足して root 空間位置を得る。
- その root 空間位置を新しい `root_xy_for_paging`（または `offset_in_subtree`）として nested abs subtree を
  walk し直す（フラグメント emit と `descendant_total_pages` 更新は既存ロジックを再利用）。

**実装は TDD で確定する。** 単純加算では `nested_abs_offset_resolves_against_cb_not_flow` が
page 4 等で落ちるはず。落ちたら CB 基準解決へ寄せる。off-by-one (page 4 vs 5) を必ず潰す。

候補実装スケッチ（exact code は test を回しながら調整）:

```rust
for child_id in children {
    let Some(child) = doc.get_node(child_id) else { continue; };
    if is_out_of_flow_positioned(child) {
        // fulgur-puml 原因①: nested abs を訪問する。位置は CB (= 現 node の box) 基準で解決し、
        // root 空間 offset に変換してから recurse する (trap A)。
        let (cw, ch) = (child.final_layout.size.width, child.final_layout.size.height);
        let (cb_w, ch_h) = (node.final_layout.size.width, node.final_layout.size.height);
        let (rel_x, rel_y) = resolve_viewport_cb_location(child, cw, ch, cb_w, ch_h)
            .unwrap_or((child.final_layout.location.x, child.final_layout.location.y));
        let nested_offset = (offset_in_subtree.0 + rel_x, offset_in_subtree.1 + rel_y);
        walk(geometry, doc, child_id, nested_offset, root_xy_for_paging, body_offset,
             page_h_px, page_stride_px, total_pages, may_extend_pages, depth + 1);
        continue;
    }
    // ... 既存 in-flow 子の処理（whitespace skip, monolithic_y_adjust 等）
}
```

注意: nested abs が `position:fixed` の場合（fixedpos-004/005/006 の最内 fixed）は別パス
(`append_position_fixed_fragments`) が扱う。fixed は repeat 要素なのでこの walk で extent に
加算してはいけない。`is_out_of_flow_positioned` は abs/fixed 両方 true なので、nested 訪問は
**abs のみ**に限定する（fixed は従来どおり continue で skip）。

**Step 2-4: テストを回しながら実装 → green を確認**

Run: `cargo test -p fulgur --test abs_positioned_pagination nested_abs_height_drives_page_count nested_abs_offset_resolves_against_cb_not_flow 2>&1 | tail -20`
Expected: この 2 つ PASS。`abs_extends_pages_despite_in_flow_content` はまだ FAIL でよい
（in-flow 併存なので Fix #2 待ち）。

**Step 5: regression guard（lib + 該当 WPT）**

```bash
cargo test -p fulgur --lib 2>&1 | tail -5
cargo test -p fulgur --test abs_positioned_pagination 2>&1 | tail -10
```

WPT 該当のみ（regression 早期検知）:

```bash
FONTCONFIG_FILE="$PWD/examples/.fontconfig/fonts.conf" \
  cargo test -p fulgur-wpt 2>&1 | grep -iE "fixedpos|monolithic-overflow-013" | tail -20
```

Expected: fixedpos-004 が test=3/ref=3 で PASS 方向。monolithic-013, fixedpos-001/002/003/008 が
依然 PASS。fixedpos-005/006 はまだ FAIL でよい。

**Step 6: Commit**

```bash
git add crates/fulgur/src/pagination_layout.rs
git commit -m "fix(pagination): visit nested abs and resolve offset against CB (fulgur-puml #1)"
```

---

### Task 3: Fix #2 — in-flow 併存時も abs がページ拡張できるようにする

**Files:**

- Modify: `crates/fulgur/src/pagination_layout.rs`
  - `append_position_absolute_body_direct_fragments` の
    `record_subtree_fragments_at_offset(..., !body_has_in_flow_content)` 呼び出し（≈ L2895）

**Step 1: 変更**

abs パスは in-flow の有無に関わらずページ拡張を許可する。最小変更:

```rust
// fulgur-puml 原因②: abs は in-flow 併存でもページを拡張できる (Chrome 準拠)。
// 旧 `!body_has_in_flow_content` は abs が pagination_geometry から落ちる問題
// (fixedpos-001/002/008) とは無関係で、拡張可否を不当に制限していた。
record_subtree_fragments_at_offset(
    geometry, doc, child_id, (resolved_x, resolved_y), body_offset_xy,
    viewport_h_px, page_stride_px, pages,
    /* may_extend_pages */ true,
);
```

`body_has_in_flow_content` が他で使われていなければ dead code 警告が出る。使われていなければ
当該ローカルを削除する（`#[allow(unused)]` で誤魔化さない）。

**Step 2-4: テスト green 確認**

Run: `cargo test -p fulgur --test abs_positioned_pagination 2>&1 | tail -10`
Expected: `abs_extends_pages_despite_in_flow_content` を含む全テスト PASS。

**Step 5: full WPT + lib suite（regression 全面確認）**

```bash
cargo test -p fulgur --lib 2>&1 | tail -5
FONTCONFIG_FILE="$PWD/examples/.fontconfig/fonts.conf" cargo test -p fulgur-wpt 2>&1 | tail -30
```

Expected: fixedpos-004/005/006 が PASS 方向。**新規 FAIL（regression）が出たら、その test が
「abs を clip すべき（paginate でない）」ケースを名指ししている** → may_extend を無条件 true ではなく
条件付き（例: abs の resolved extent が実在 inset 由来のときのみ）に絞る。先回り実装はしない。

**Step 6: Commit**

```bash
git add crates/fulgur/src/pagination_layout.rs
git commit -m "fix(pagination): allow abs to extend page count with in-flow content (fulgur-puml #2)"
```

---

### Task 4: WPT expectations を PASS に昇格

**Files:**

- Modify: `crates/fulgur-wpt/expectations/css-page.txt`（L34-36 の fixedpos-004/005/006）

**Step 1: 実 PASS を確認してから昇格**

`/wpt-promote` の流儀に従い、まず該当テストが実際に PASS することを確認:

```bash
FONTCONFIG_FILE="$PWD/examples/.fontconfig/fonts.conf" \
  cargo test -p fulgur-wpt 2>&1 | grep -iE "fixedpos-00[456]"
```

**Step 2: expectations 編集**

`crates/fulgur-wpt/expectations/css-page.txt` の以下 3 行を `FAIL ... # page count mismatch...`
から `PASS` に変更し、行末に `# fulgur-puml` 参照を付す:

```text
PASS  css/css-page/fixedpos-004-print.html  # fulgur-puml
PASS  css/css-page/fixedpos-005-print.html  # fulgur-puml
PASS  css/css-page/fixedpos-006-print.html  # fulgur-puml
```

**Step 3: 昇格後の expectation で全 WPT green 確認**

```bash
FONTCONFIG_FILE="$PWD/examples/.fontconfig/fonts.conf" cargo test -p fulgur-wpt 2>&1 | tail -15
```

Expected: expectation mismatch 0。

**Step 4: Commit**

```bash
git add crates/fulgur-wpt/expectations/css-page.txt
git commit -m "test(wpt): promote fixedpos-004/005/006-print to PASS (fulgur-puml)"
```

---

### Task 5: 最終検証 + clippy/fmt

**Step 1: 全体検証**

```bash
cargo fmt --check
cargo clippy -p fulgur --all-targets 2>&1 | tail -15
cargo test -p fulgur 2>&1 | tail -5
FONTCONFIG_FILE="$PWD/examples/.fontconfig/fonts.conf" cargo test -p fulgur-wpt 2>&1 | tail -10
```

Expected: fmt OK、clippy 警告なし、全テスト green。

**Step 2: 受け入れ基準の確認（design 参照）**

- [ ] fixedpos-004/005/006-print PASS
- [ ] monolithic-overflow-013, fixedpos-001/002/003/008-print 引き続き PASS
- [ ] 既存 fulgur テスト全緑
- [ ] nested abs / in-flow+abs / CB 解決の再現テスト追加済（Task 1）

**Step 3: 必要なら fmt commit**

```bash
cargo fmt
git add -A && git commit -m "style: cargo fmt"
```

---

## 留意

- DRY: `resolve_viewport_cb_location` を再利用（新規解決ロジックを書かない）
- YAGNI: may_extend は無条件 true から始め、regression が出た test に応じてのみ条件を足す
- trap B（fixed-repeat 伝播）は engine.rs の既存「abs パス後に implied_page_count 再読 →
  fixed 再実行」ループで自動成立。abs パスが implied_page_count を伸ばすことだけ保証すればよい
- nested 訪問は **abs のみ**。fixed は `append_position_fixed_fragments` の担当（repeat 要素なので
  extent 加算してはいけない）

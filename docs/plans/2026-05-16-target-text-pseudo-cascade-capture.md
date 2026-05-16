# target-text pseudo cascade-safe capture Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Make `target-text(attr(href), before|after|first-letter)` reflect the
target element's resolved `::before` / `::after` content — including
cascade-only generated content such as `attr(data-x)` and plain strings that
fulgur currently leaves to Blitz and never captures into `AnchorMap`.

**Architecture:** The GCPM parser deliberately routes pseudo content **without**
`counter()` / `target-*` to Blitz's normal cascade (`parser.rs:721-749`:
`has_counter == false` ⇒ `content_items = None` ⇒ `CounterPass` never visits it
⇒ `before_text` / `after_text` stay empty). The fix adds a **read-only** capture
in `walk_anchors` (engine.rs) that reads each id-bearing element's pseudo node
resolved computed value directly via `primary_styles().get_counters().content`
— the same pattern as `blitz_adapter::extract_content_image_url` — and resolves
the text items. This runs after `CounterPass` + `InjectCssPass` + pagination, so
the cascade already carries both the original `attr()`/string values *and*
`CounterPass`'s injected resolved-counter overlay. One capture path therefore
covers both the cascade-only case and the counter-tracked case; no CSS is
injected and Blitz rendering is never overridden (cascade-safe).

**Tech Stack:** Rust, stylo (`style::values::generics::counters`), blitz-dom
0.2.4, `unicode-segmentation`, `unicode-properties` (new direct dep).

**Scope reconciliation (design memo is partially stale):** The issue's design
field predates work that already landed. Already implemented — **do NOT redo**:
`TargetTextKind` enum (`Content`/`Before`/`After`/`FirstLetter`,
`gcpm/mod.rs:333-345`); parser arm for `before|after|first-letter|content`
(`parser.rs:1121-1158`); `AnchorEntry.before_text`/`after_text` as `String`
(`target_ref.rs`); `resolve_target_text` kind dispatch
(`target_ref.rs:115-128`); `TargetText { url, kind }` variant. Deliberately
dropped from the memo (YAGNI): the new `TargetTextSource` enum + ~25 callsite
migration, the `Option<String>` field migration, a separate
`first_letter` `AnchorEntry` field (compute on-the-fly in the resolver instead),
and parser/drop-test rework. Net: ~4 commits, not 6.

---

## Task 1: Spike — confirm what Stylo exposes at `walk_anchors` time

This is the one assumption that breaks the whole approach if wrong:
**does `doc.get_node(node.before).primary_styles()` return the resolved pseudo
content at the point `walk_anchors` runs (after CounterPass + InjectCssPass +
pagination)?** And does Stylo 0.8 resolve `attr()` at computed time
(`Content::Items[String("...")]`) or defer it (`ContentItem::Attr(name)`)?

**Files:**

- Temp test in: `crates/fulgur/tests/render_smoke.rs`

**Step 1: Write a throwaway probe test**

Add to `crates/fulgur/tests/render_smoke.rs`:

```rust
#[test]
fn spike_pseudo_content_shape() {
    // Not a permanent test — deleted in Step 4 once findings are recorded.
    let html = r##"<!doctype html><html><head><style>
      #t::before { content: attr(data-x); }
      #t::after  { content: "lit-after"; }
    </style></head><body>
      <p>see <a href="#t">ref</a></p>
      <h2 id="t" data-x="ATTRVAL">Heading</h2>
    </body></html>"##;
    let pdf = fulgur::Engine::builder()
        .build()
        .render_html(html)
        .expect("render");
    assert!(!pdf.is_empty());
}
```

Then add a temporary `eprintln!` inside `walk_anchors`
(`crates/fulgur/src/engine.rs`, in the `if let Some(elem) = node.element_data()`
block, after `let text = collect_text_content(...)`):

```rust
for pid in [node.before, node.after].into_iter().flatten() {
    if let Some(pn) = doc.get_node(pid) {
        if let Some(st) = pn.primary_styles() {
            eprintln!("SPIKE pseudo {pid}: {:?}", st.get_counters().content);
        } else {
            eprintln!("SPIKE pseudo {pid}: NO primary_styles");
        }
    }
}
```

**Step 2: Run the probe**

Run: `cargo test -p fulgur --test render_smoke spike_pseudo_content_shape -- --nocapture 2>&1 | grep SPIKE`

Expected: lines showing the `Content` enum shape for the `attr()` and the
string pseudo. Record (in the commit message of Task 2, and below):

- Does `primary_styles()` return `Some` for the pseudo node here? (must be Yes)
- Is `attr(data-x)` `Content::Items[String("ATTRVAL")]` (resolved) or
  `Content::Items[Attr("data-x")]` (deferred)?
- Is the string `Content::Items[String("lit-after")]`?

**Step 3: Record findings, revert the probe**

Write the three answers into a code comment you will place above
`collect_pseudo_text` in Task 2. Remove the `eprintln!` and the
`spike_pseudo_content_shape` test (revert engine.rs, delete the test fn).

Run: `git diff --stat` — Expected: clean (no residual spike code).

**Step 4: No commit** (spike leaves no tracked change). Proceed to Task 2 with
findings in hand. If `primary_styles()` returned `None` for the pseudo, STOP and
escalate — the read-only-at-walk_anchors approach is invalid and the plan needs
rework (fallback: keep the `CounterPass` plumbing and additionally force
attr-only pseudo into `content_counter_mappings`).

---

## Task 2: `collect_pseudo_text` capture helper + rewire `walk_anchors`

**Files:**

- Modify: `crates/fulgur/src/engine.rs` (add `collect_pseudo_text`; call it in
  `walk_anchors` ~`engine.rs:852-869`)
- Modify: `crates/fulgur/src/engine.rs` (drop the now-redundant `pseudo_texts`
  parameter threading — see Step 5)
- Modify: `crates/fulgur/src/blitz_adapter.rs` (remove `pseudo_text_by_node`
  field, `take_pseudo_texts`, and the two `.before_text =` / `.after_text =`
  writes at ~1883/1914 — **only if** Task 1 confirmed the direct read covers the
  counter-tracked case; otherwise keep them and have `collect_pseudo_text` win
  only when non-empty)
- Test: `crates/fulgur/src/engine.rs` `#[cfg(test)] mod tests` (unit test for
  `collect_pseudo_text`)

**Step 1: Write the failing unit test**

In `engine.rs` test module (create one if absent, mirroring existing
module-level test convention):

```rust
#[test]
fn collect_pseudo_text_resolves_string_and_attr() {
    // Drives Engine end-to-end and asserts the captured AnchorEntry.
    // collect_pseudo_text has no pure-fn seam (needs a real pseudo node),
    // so exercise it through build_anchor_map via render.
    let html = r##"<!doctype html><html><head><style>
      #t::before { content: attr(data-x) " "; }
      #t::after  { content: "AFT"; }
    </style></head><body>
      <p>x <a href="#t">L</a></p>
      <h2 id="t" data-x="DX">Title</h2>
    </body></html>"##;
    let pdf = crate::Engine::builder().build().render_html(html).unwrap();
    assert!(!pdf.is_empty());
    // Behavioural assertion lives in the integration tests (Task 4);
    // here we only guard that render does not panic with the new path.
}
```

> Note: `collect_pseudo_text` reads a live `BaseDocument` pseudo node, so it has
> no clean pure-function seam. The real behavioural coverage is the integration
> tests in Task 4 (which assert resolved text appears in the PDF text layer).
> This unit test is a panic/compile guard only. Keep it minimal.

**Step 2: Run it to verify it fails**

Run: `cargo test -p fulgur --lib engine::tests::collect_pseudo_text_resolves_string_and_attr`
Expected: FAIL — test fn references nothing new yet *or* compile error once
Step 3 signature changes land; that's the red.

**Step 3: Implement `collect_pseudo_text`**

Add to `crates/fulgur/src/engine.rs` (near `collect_text_content`,
~`engine.rs:876`). Use the findings from Task 1 in the doc comment. Handle
**both** `String` and `Attr` items (Attr is the safety net for the deferred
case; harmless if Stylo resolves eagerly):

```rust
/// Capture the resolved text of a `::before` / `::after` pseudo node for
/// `target-text(url, before|after)`.
///
/// Read-only: reads the post-cascade computed value via
/// `primary_styles().get_counters().content` (same pattern as
/// `blitz_adapter::extract_content_image_url`). Runs at `walk_anchors`
/// time — after CounterPass + InjectCssPass + pagination — so the cascade
/// already carries (a) original `attr()` / string values that fulgur
/// leaves to Blitz, and (b) CounterPass's injected resolved-counter
/// overlay. One path therefore covers cascade-only and counter-tracked
/// pseudo content without injecting CSS or overriding Blitz.
///
/// Stylo 0.8 behaviour observed in Task 1 spike: <RECORD: String vs Attr,
/// primary_styles Some/None>.
///
/// `parent_elem` supplies `attr()` resolution when Stylo defers it
/// (`ContentItem::Attr`). Counter / Counters / Image / target-* /
/// leader items are skipped — text items only, joined and
/// whitespace-normalized to match `collect_text_content`.
fn collect_pseudo_text(
    doc: &BaseDocument,
    pseudo_id: Option<usize>,
    parent_elem: &blitz_dom::node::ElementData,
) -> String {
    use style::values::generics::counters::{Content, ContentItem as Sci};
    let Some(pid) = pseudo_id else {
        return String::new();
    };
    let Some(pnode) = doc.get_node(pid) else {
        return String::new();
    };
    let Some(styles) = pnode.primary_styles() else {
        return String::new();
    };
    let Content::Items(item_data) = &styles.get_counters().content else {
        return String::new();
    };
    let main = &item_data.items[..item_data.alt_start];
    let mut out = String::new();
    for item in main {
        match item {
            Sci::String(s) => out.push_str(s.as_ref()),
            Sci::Attr(a) => {
                // stylo's Attr carries the attribute name; resolve against
                // the *element*, not the pseudo (pseudo has no attrs).
                if let Some(v) = crate::blitz_adapter::get_attr(parent_elem, a.attribute.as_ref())
                {
                    out.push_str(v);
                }
            }
            _ => {}
        }
    }
    out.split_whitespace().collect::<Vec<_>>().join(" ")
}
```

> **Verify the `Sci::Attr` field path against Task 1 / the stylo source**
> before finalizing — the exact field (`a.attribute` vs `a.name` vs tuple) and
> `get_attr`'s expected name form (lower-cased?) must match. If Task 1 showed
> Stylo always resolves `attr()` eagerly to `String`, the `Sci::Attr` arm is
> dead-but-safe; keep it as documented defense and do not over-invest in its
> field plumbing (a `_ => {}`-style fallthrough is acceptable if the field
> shape is awkward — note it in the comment).

**Step 4: Wire into `walk_anchors`**

In `crates/fulgur/src/engine.rs` `walk_anchors`, replace the
`pseudo_texts`-sourced lines (~`engine.rs:858-867`):

```rust
let before_text = collect_pseudo_text(doc, node.before, elem);
let after_text = collect_pseudo_text(doc, node.after, elem);
out.insert(
    frag.to_string(),
    AnchorEntry {
        page_num,
        counters,
        text,
        before_text,
        after_text,
    },
);
```

(`elem` is already bound in the enclosing `if let Some(elem) = node.element_data()`.)

**Step 5: Remove the dead `pseudo_texts` plumbing**

Only if Task 1 confirmed the direct read also covers counter-tracked pseudo
(InjectCssPass overlay visible in `primary_styles()`):

- `engine.rs`: drop `pseudo_texts` from `build_anchor_map` /`walk_anchors`
  signatures and the `take_pseudo_texts()` call (~`engine.rs:300`); collapse the
  4-tuple at ~`engine.rs:286` to a 3-tuple.
- `blitz_adapter.rs`: remove the `pseudo_text_by_node` field (~1649), its init
  (~1686), `take_pseudo_texts` (~1731-1732), and the two pseudo-text writes
  inside the before/after resolve blocks (~1883-1887, ~1914-1918). Leave the
  rest of those blocks (the `write!(css, ...)` overlay) intact — that overlay is
  what makes the counter-resolved value visible to Step 3's read.
- Delete `AnchorPseudoText` from `target_ref.rs` (~31-36) and its
  `engine.rs:808` import if no longer referenced.

If Task 1 did **not** confirm uniform coverage: keep the plumbing, and in Step 4
make `collect_pseudo_text` the source only when it returns non-empty, else fall
back to `pseudo_texts.get(&node_id)`. Document the divergence in the commit.

**Step 6: Run the targeted + lib tests**

Run: `cargo test -p fulgur --lib 2>&1 | tail -5`
Expected: PASS, count ≥ baseline 1121 (1 new), 0 failed. Fix any fallout from
the signature changes in Step 5 before proceeding.

**Step 7: Commit**

```bash
git add crates/fulgur/src/engine.rs crates/fulgur/src/blitz_adapter.rs crates/fulgur/src/gcpm/target_ref.rs
git commit -m "feat(gcpm): cascade-safe capture of pseudo content for target-text"
```

---

## Task 3: CSS Pseudo-Elements 4 `first-letter` algorithm

Replace the simplistic `entry.text.graphemes(true).next()` in the resolver
(`target_ref.rs:126`) with the spec-conformant first-letter (leading
typographic punctuation + first letter/digit + trailing typographic
punctuation, CSS Pseudo-Elements 4 §3.2).

**Files:**

- Modify: `crates/fulgur/Cargo.toml` (add `unicode-properties`)
- Modify: `crates/fulgur/src/gcpm/target_ref.rs` (add `compute_first_letter`;
  use it in `resolve_target_text`)
- Test: `crates/fulgur/src/gcpm/target_ref.rs` `mod tests`

**Step 1: Add the dependency**

In `crates/fulgur/Cargo.toml`, after the `unicode-segmentation = "1.13"` line:

```toml
unicode-properties = "0.1"
```

Run: `cargo build -p fulgur 2>&1 | tail -2` — Expected: resolves (it is already
transitive in `Cargo.lock`; adding it as a direct dep is the only change).

**Step 2: Write the failing unit tests**

Add to the `tests` module in `crates/fulgur/src/gcpm/target_ref.rs`:

```rust
#[test]
fn first_letter_ascii() {
    assert_eq!(compute_first_letter("Hello world"), "H");
}
#[test]
fn first_letter_skips_leading_punct_and_keeps_trailing() {
    assert_eq!(compute_first_letter("「『Hello』"), "「『H");
}
#[test]
fn first_letter_digit_counts_as_letter() {
    assert_eq!(compute_first_letter("123abc"), "1");
}
#[test]
fn first_letter_empty_and_all_punct() {
    assert_eq!(compute_first_letter(""), "");
    assert_eq!(compute_first_letter("   "), "");
    assert_eq!(compute_first_letter("...!?"), "");
}
#[test]
fn first_letter_space_before_letter_yields_nothing() {
    // Leading whitespace is trimmed, but a space *between* the leading
    // punctuation run and the letter terminates the first-letter per spec.
    assert_eq!(compute_first_letter("『 H"), "");
}
#[test]
fn first_letter_grapheme_cluster() {
    // Combining sequence stays one unit.
    assert_eq!(compute_first_letter("e\u{0301}tude"), "e\u{0301}");
}
```

**Step 3: Run to verify they fail**

Run: `cargo test -p fulgur --lib gcpm::target_ref::tests::first_letter`
Expected: FAIL — `compute_first_letter` not defined.

**Step 4: Implement `compute_first_letter`**

Add to `crates/fulgur/src/gcpm/target_ref.rs` (near `resolve_target_text`):

```rust
/// First-letter per CSS Pseudo-Elements 4 §3.2: optional leading
/// typographic punctuation, the first typographic letter/digit
/// (grapheme cluster), then optional trailing typographic punctuation.
/// Whitespace before the first letter (other than fully-trimmed leading
/// whitespace) terminates the run. Returns `""` when there is no letter.
fn compute_first_letter(text: &str) -> String {
    use unicode_properties::{GeneralCategory as GC, UnicodeGeneralCategory};
    use unicode_segmentation::UnicodeSegmentation;

    fn is_punct(g: &str) -> bool {
        g.chars().all(|c| {
            matches!(
                c.general_category(),
                GC::OpenPunctuation
                    | GC::ClosePunctuation
                    | GC::InitialPunctuation
                    | GC::FinalPunctuation
                    | GC::OtherPunctuation
                    | GC::ConnectorPunctuation
                    | GC::DashPunctuation
                    | GC::MathSymbol
                    | GC::OtherSymbol
                    | GC::CurrencySymbol
                    | GC::ModifierSymbol
            )
        })
    }
    fn is_letter(g: &str) -> bool {
        g.chars().any(|c| {
            matches!(
                c.general_category(),
                GC::UppercaseLetter
                    | GC::LowercaseLetter
                    | GC::TitlecaseLetter
                    | GC::ModifierLetter
                    | GC::OtherLetter
                    | GC::DecimalNumber
                    | GC::LetterNumber
                    | GC::OtherNumber
            )
        })
    }

    let trimmed = text.trim_start_matches(char::is_whitespace);
    let gs: Vec<&str> = trimmed.graphemes(true).collect();
    let mut i = 0;
    while i < gs.len() && is_punct(gs[i]) {
        i += 1;
    }
    if i >= gs.len() || !is_letter(gs[i]) {
        return String::new();
    }
    let mut j = i + 1;
    while j < gs.len() && is_punct(gs[j]) {
        j += 1;
    }
    gs[..j].concat()
}
```

In `resolve_target_text`, replace the `FirstLetter` arm:

```rust
TargetTextKind::FirstLetter => compute_first_letter(&entry.text),
```

If `entry.text.graphemes(...)` was the only `UnicodeSegmentation` use in the
file, the import may now be unused at module scope — keep it inside
`compute_first_letter` only and remove the stale top-level `use`.

**Step 5: Run to verify pass**

Run: `cargo test -p fulgur --lib gcpm::target_ref::tests 2>&1 | tail -5`
Expected: PASS, including the pre-existing
`target_text_first_letter_is_grapheme_cluster` /
`target_text_returns_before_after_and_first_letter`. If a pre-existing
first-letter test asserted the old single-grapheme behaviour and now conflicts
with spec output, update it to the spec-correct expectation and note why in the
commit (the old behaviour was a known simplification).

**Step 6: Commit**

```bash
git add crates/fulgur/Cargo.toml crates/fulgur/Cargo.lock crates/fulgur/src/gcpm/target_ref.rs
git commit -m "feat(gcpm): CSS Pseudo-Elements 4 conformant target-text first-letter"
```

---

## Task 4: Integration smoke tests

End-to-end coverage that resolved pseudo text reaches pass 2 render. Per
CLAUDE.md coverage policy, render-path logic needs a `render_smoke.rs`
end-to-end test (codecov patch coverage does not see VRT-only paths).

**Files:**

- Test: `crates/fulgur/tests/render_smoke.rs`

**Step 1: Write the tests**

Append to `crates/fulgur/tests/render_smoke.rs`. If the harness exposes a PDF
text-extraction helper (check existing tests in the file for a
`extract_text` / `pdf_text` util — fulgur has `inspect.rs`), assert the
resolved string is present; otherwise assert non-empty PDF + no panic and rely
on Task 2/3 unit coverage for correctness.

```rust
#[test]
fn target_text_before_resolves_attr_pseudo() {
    let html = r##"<!doctype html><html><head><style>
      #sec::before { content: attr(data-tag) ": "; }
      .ref::after  { content: target-text(attr(href), before); }
    </style></head><body>
      <p><a class="ref" href="#sec"></a></p>
      <h2 id="sec" data-tag="APP">Appendix</h2>
    </body></html>"##;
    let pdf = fulgur::Engine::builder().build().render_html(html).unwrap();
    assert!(!pdf.is_empty());
    // If a text extractor is available:
    // assert!(pdf_text(&pdf).contains("APP:"));
}

#[test]
fn target_text_after_resolves_counter_via_counter_pass() {
    // Regression: counter-tracked pseudo still captured after the
    // pseudo_text_by_node removal (InjectCssPass overlay must be visible
    // to collect_pseudo_text).
    let html = r##"<!doctype html><html><head><style>
      body { counter-reset: c; }
      h2 { counter-increment: c; }
      #s::after { content: " [" counter(c) "]"; }
      .r::before { content: target-text(attr(href), after); }
    </style></head><body>
      <h2>One</h2>
      <h2 id="s">Two</h2>
      <p><a class="r" href="#s"></a></p>
    </body></html>"##;
    let pdf = fulgur::Engine::builder().build().render_html(html).unwrap();
    assert!(!pdf.is_empty());
    // assert!(pdf_text(&pdf).contains("[2]"));
}

#[test]
fn target_text_first_letter_typographic() {
    let html = r##"<!doctype html><html><head><style>
      .r::after { content: target-text(attr(href), first-letter); }
    </style></head><body>
      <p><a class="r" href="#h"></a></p>
      <h2 id="h">「Hello」</h2>
    </body></html>"##;
    let pdf = fulgur::Engine::builder().build().render_html(html).unwrap();
    assert!(!pdf.is_empty());
    // assert!(pdf_text(&pdf).contains("「H"));
}

#[test]
fn target_text_empty_for_missing_pseudo() {
    let html = r##"<!doctype html><html><head><style>
      .r::after { content: "[" target-text(attr(href), before) "]"; }
    </style></head><body>
      <p><a class="r" href="#h"></a></p>
      <h2 id="h">No pseudo here</h2>
    </body></html>"##;
    let pdf = fulgur::Engine::builder().build().render_html(html).unwrap();
    assert!(!pdf.is_empty()); // resolves to "[]" — no panic, empty capture
}
```

**Step 2: Run**

Run: `cargo test -p fulgur --test render_smoke target_text 2>&1 | tail -8`
Expected: PASS, 0 failed.

**Step 3: Decide on text-layer assertions**

Check `crates/fulgur/src/inspect.rs` / existing `render_smoke.rs` tests for a
reusable PDF-text extractor. If one exists, upgrade the `// assert!(...)`
comments to real assertions and re-run. If not, leave the non-empty assertions
and rely on Task 2/3 units — note the limitation in the commit message.

**Step 4: Commit**

```bash
git add crates/fulgur/tests/render_smoke.rs
git commit -m "test(gcpm): integration coverage for target-text pseudo capture"
```

---

## Final verification

Run before declaring done (REQUIRED SUB-SKILL:
superpowers:verification-before-completion):

```bash
cargo test -p fulgur --lib 2>&1 | tail -3
cargo test -p fulgur --test render_smoke 2>&1 | tail -3
cargo clippy -p fulgur --all-targets 2>&1 | tail -3
cargo fmt --check
FONTCONFIG_FILE="$PWD/examples/.fontconfig/fonts.conf" cargo test -p fulgur-vrt 2>&1 | tail -3
```

Expected: all green; lib count ≥ 1121 + new tests; clippy clean; fmt clean; VRT
no regressions (this change must not alter PDF output for any document that does
**not** use `target-text(url, before|after|first-letter)` — the capture path is
read-only and gated behind `has_target_refs`).

## Acceptance (from fulgur-r73p)

- [ ] `target-text(attr(href), before|after|first-letter)` parse → variant
      (already done; covered by existing parser tests — confirm green)
- [ ] pass 2 reflects target's `::before`/`::after` resolved text including
      `attr()` / string / `counter()` (Task 2 + Task 4)
- [ ] first-letter is CSS Pseudo-Elements 4 conformant (Task 3)
- [ ] existing `target-text(attr(href))` (Content default) unchanged
      (VRT + existing target_ref tests green)

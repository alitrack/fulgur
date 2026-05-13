# In-flow `height:300vh` Pagination Regression Fix Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:systematic-debugging and superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Restore in-flow tall content pagination so `<div style="height:300vh">` produces 3 pages (and `fixedpos-003-print.html` returns to PASS).

**Architecture:** Bisect against minimal HTML to identify the breaking commit on `crates/fulgur/src/pagination_layout.rs` (or related), read the diff, fix the regressed logic, restore behaviour. The fix is expected to be narrow (a few lines) and located inside the pagination total-pages calculation.

**Tech Stack:** Rust, `cargo run -p fulgur-cli`, `pdfinfo`, `git bisect run`, `cargo test -p fulgur --lib`, `cargo test -p fulgur-wpt --test wpt_css_page`.

**Beads issue:** `fulgur-sbw2` (blocks `fulgur-puml`).

---

## Task 0: Lock the repro

**Files:**

- Create: `/tmp/regression-300vh.html` (working file, not committed)
- Create: `crates/fulgur/tests/render_smoke.rs` additions OR a unit test in `pagination_layout.rs` module — pick whichever is cheaper

**Step 1: Write the failing smoke assertion**

Append to `crates/fulgur/tests/render_smoke.rs`:

```rust
#[test]
fn in_flow_300vh_paginates_to_three_pages() {
    // Regression guard: `<div style="height:300vh">` must occupy 3 pages.
    // See beads fulgur-sbw2.
    let html = r#"<!DOCTYPE html>
<body style="margin:0">
<div style="height:300vh">x</div>
</body>"#;
    let pdf = fulgur::Engine::builder()
        .build()
        .render_html(html)
        .expect("render");
    let pages = pdftools_page_count(&pdf);
    assert_eq!(pages, 3, "expected 3 pages, got {pages}");
}
```

If `pdftools_page_count` does not exist in render_smoke, use a simpler scheme: parse the PDF's `/Count` via `lopdf` or shell out to `pdfinfo` from a `#[ignore]`'d integration test. Otherwise inspect `Drawables`/page table directly via internal API.

**Cheaper alternative:** add the assertion as a unit test inside `pagination_layout.rs` `#[cfg(test)] mod tests` if the page-count is reachable from the table state without going through PDF bytes. Prefer this if a helper like `paginate_for_test` already exists.

**Step 2: Run it to confirm it fails**

```bash
cargo test -p fulgur --lib in_flow_300vh_paginates_to_three_pages -- --exact
```

Expected: FAIL with `expected 3 pages, got 1`.

**Step 3: Commit the red test**

```bash
git add crates/fulgur/tests/render_smoke.rs
git commit -m "test(pagination): pin in-flow 300vh page count regression (fulgur-sbw2)"
```

---

## Task 1: Bisect to identify the breaking commit

**Files:** none modified during bisect itself; documentation in commit message after.

**Step 1: Define the bisect predicate script**

Create a temporary script `/tmp/bisect-300vh.sh`:

```bash
#!/usr/bin/env bash
set -e
cd /home/ubuntu/fulgur/.worktrees/fulgur-puml
cargo build --quiet -p fulgur-cli 2>/dev/null || exit 125
cat > /tmp/r.html <<EOF
<!DOCTYPE html>
<body style="margin:0">
<div style="height:300vh">x</div>
</body>
EOF
cargo run --quiet -p fulgur-cli -- render /tmp/r.html -o /tmp/r.pdf >/dev/null 2>&1 || exit 125
pages=$(pdfinfo /tmp/r.pdf 2>/dev/null | awk '/^Pages:/ {print $2}')
[ "$pages" = "3" ] && exit 0 || exit 1
chmod +x /tmp/bisect-300vh.sh
```

**Step 2: Find a known-good baseline**

Walk backwards through `git log --oneline -- crates/fulgur/src/pagination_layout.rs` and test a few candidates manually (or skip straight to the wider range below):

```bash
git stash  # save the red test before bisect
git bisect start
git bisect bad HEAD
# Try several days back as a guess; widen if still bad.
git bisect good <SHA-from-2026-04-15-ish>
git bisect run /tmp/bisect-300vh.sh
git bisect log > /tmp/bisect.log
git bisect reset
git stash pop
```

If the chosen "good" point is itself bad, widen the range further (e.g., to early March commits) until `git bisect good <SHA>` is genuinely good.

**Step 3: Document the breaking commit**

Add an entry in this plan file under "Findings" with the SHA and one-line summary of what that commit changed.

---

## Task 2: Read the breaking commit and write the fix

**Files:** Whichever the breaking commit touched — most likely `crates/fulgur/src/pagination_layout.rs`. Possibly `crates/fulgur/src/convert/*.rs` or `crates/fulgur/src/paginate.rs`.

**Step 1: Read the breaking diff**

```bash
git show <BREAKING_SHA> -- crates/fulgur/src/
```

Identify: what value did the commit clamp / change / replace? Does the regression appear when the in-flow `<div height:300vh>` height is no longer counted toward the total page count, or when the page-stride / viewport math was altered?

Hypothesis candidates (record which one matched):

- **H1**: `body` height now collapses to the in-flow height it sees *after* skipping the abs/fixed children, but `<div height:300vh>` is in-flow and should be counted. A condition was inverted.
- **H2**: viewport_h vs page_h_px rounding now clamps in-flow extent to `<= 1.0 * page_h_px`.
- **H3**: A snap-to-integer applied to `total_pages` floors when it should ceil, only when no OOF children are present.

**Step 2: Write the minimal fix**

Keep the diff surgical. Add an inline comment with `(fulgur-sbw2)` and the WPT impact, but no narration of past behaviour.

**Step 3: Run unit tests**

```bash
cargo test -p fulgur --lib
```

Expected: previously red test now passes, no other regressions.

**Step 4: Commit the fix**

```bash
git add crates/fulgur/src/...
git commit -m "fix(pagination): restore in-flow tall content page count (fulgur-sbw2)"
```

---

## Task 3: Re-run WPT and update expectations

**Files:**

- Modify: `crates/fulgur-wpt/expectations/css-page.txt` (only `fixedpos-003-print`, and any incidental promotions/demotions)

**Step 1: Run the full css-page WPT**

```bash
FULGUR_WPT_REQUIRED=1 FONTCONFIG_FILE="$PWD/examples/.fontconfig/fonts.conf" \
  cargo test -p fulgur-wpt --test wpt_css_page -- --nocapture 2>&1 | tail -20
cat target/wpt-report/css-page/regressions.json
```

Expected: `regressions.json` no longer lists `fixedpos-003-print`. `fixedpos-004/005/006` may move from `test=1` back to `test=2/3`, but they remain FAIL (covered by `fulgur-puml`).

**Step 2: Update expectations for incidental shifts**

If a `test=N ref=M` value changes for `fixedpos-004/005/006`, update the comment so the file matches the new observation. Do *not* promote them to PASS — that is `fulgur-puml`.

**Step 3: Run full lib + wpt-css-page once more for clean baseline**

```bash
cargo test -p fulgur --lib
FULGUR_WPT_REQUIRED=1 FONTCONFIG_FILE="$PWD/examples/.fontconfig/fonts.conf" \
  cargo test -p fulgur-wpt --test wpt_css_page
```

**Step 4: Commit**

```bash
git add crates/fulgur-wpt/expectations/css-page.txt
git commit -m "test(wpt): refresh fixedpos page-count expectations (fulgur-sbw2)"
```

---

## Task 4: Verification gate

Run before declaring done — `superpowers:verification-before-completion`:

```bash
cargo fmt --check
cargo clippy -p fulgur -p fulgur-cli --no-deps -- -D warnings
cargo test -p fulgur --lib
FULGUR_WPT_REQUIRED=1 FONTCONFIG_FILE="$PWD/examples/.fontconfig/fonts.conf" \
  cargo test -p fulgur-wpt --test wpt_css_page
```

All must pass with no new regressions in `target/wpt-report/css-page/regressions.json`.

---

## Findings

- **Not a regression in the strict sense** — `<div style="height:300vh">` always rendered as 1 page since at least commit `3d39f1f3` (when the WPT expectation for `fixedpos-003-print` was promoted to PASS — both test and ref happened to render 1 page, masking the in-flow gap). The "regression" message in beads/discussion was misdescribed.
- **One-line cause**: `fragment_pagination_root`'s recursion gate (`pagination_layout.rs:833-849`) measures *descendant* overflow via `would_split_block_subtree` — a body-direct child whose own CSS height exceeds `page_height_px` but whose descendants all fit (`<div height:300vh>x</div>`) falls through to the whole-emit path and lands as one oversized fragment on one page.
- **Fix location**: insertion at `crates/fulgur/src/pagination_layout.rs` between the recursion gate's fallthrough (post-line 891) and the whole-emit (pre-line 902). Slices the child into one fragment per page strip with page-local `y`, advancing `page_index` accordingly. `PaginationGeometry::is_split()` flips automatically once `fragments.len() > 1`, and `render.rs` already honours the per-slice height. Atomic boxes (CSS `transform`) opt out; `contain: size` does *not* (WPT `monolithic-overflow-022-print` expects it to span 4 pages).

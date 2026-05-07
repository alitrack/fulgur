# Target Text Second Argument Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add GCPM `target-text(attr(href), before|after|first-letter)` support while preserving existing default `content` behavior.

**Architecture:** Extend `ContentItem::TargetText` with a small mode enum, parse the optional second argument, and resolve it through `AnchorEntry`. Reuse `CounterPass`'s already-resolved pseudo content for target `::before`/`::after`, and derive `first-letter` from normalized target text.

**Tech Stack:** Rust workspace, `cssparser`, existing GCPM two-pass render pipeline, `cargo test`.

---

### Task 1: Parser And Resolver Model

**Files:**
- Modify: `crates/fulgur/src/gcpm/mod.rs`
- Modify: `crates/fulgur/src/gcpm/parser.rs`
- Modify: `crates/fulgur/src/gcpm/target_ref.rs`

- [ ] **Step 1: Write failing parser/resolver tests**

Add tests proving `target-text(attr(href), before|after|first-letter|content)` parses into a target-text item and resolves different text forms.

- [ ] **Step 2: Run focused tests and verify RED**

Run: `cargo test -p fulgur gcpm::parser::tests::parse_target_text_with_supported_2nd_arg --lib`

Expected: FAIL because supported second arguments are currently dropped.

- [ ] **Step 3: Add `TargetTextKind` and extend `ContentItem::TargetText`**

Use variants `Content`, `Before`, `After`, and `FirstLetter` with `Content` as default.

- [ ] **Step 4: Parse optional second argument**

Accept `content`, `before`, `after`, and `first-letter`; drop unknown arguments or trailing tokens.

- [ ] **Step 5: Extend target text resolution**

Resolve `Content` from `AnchorEntry.text`, `Before` from `AnchorEntry.before_text`, `After` from `AnchorEntry.after_text`, and `FirstLetter` from the first Unicode scalar of normalized text.

### Task 2: Pseudo Text Capture

**Files:**
- Modify: `crates/fulgur/src/blitz_adapter.rs`
- Modify: `crates/fulgur/src/engine.rs`

- [ ] **Step 1: Write failing CounterPass/engine tests**

Add tests showing resolved `::before` and `::after` content is captured per node and made available in `AnchorMap`.

- [ ] **Step 2: Run focused tests and verify RED**

Run: `cargo test -p fulgur blitz_adapter::tests::counter_pass_records_target_text_pseudo_content --lib`

Expected: FAIL because CounterPass currently only emits CSS, not reusable pseudo text.

- [ ] **Step 3: Record pseudo text in CounterPass**

Add a per-node pseudo text map populated at the same point generated CSS is written.

- [ ] **Step 4: Pass pseudo text into `build_anchor_map`**

Populate `AnchorEntry.before_text` and `AnchorEntry.after_text` when pass 1 builds the map.

### Task 3: End-To-End Verification

**Files:**
- Modify: `crates/fulgur/tests/render_smoke.rs`

- [ ] **Step 1: Write failing smoke test**

Add an HTML render test where a reference emits `target-text(attr(href), before)`, `after`, and `first-letter` and assert extracted PDF text contains the expected fragments.

- [ ] **Step 2: Run focused test and verify RED**

Run: `cargo test -p fulgur --test render_smoke target_text_second_arg_resolves_target_fragments`

Expected: FAIL before implementation is complete.

- [ ] **Step 3: Run focused and full quality gates**

Run: `cargo fmt --check`, `cargo test -p fulgur gcpm::parser::tests::parse_target_text_with_supported_2nd_arg --lib`, `cargo test -p fulgur --test render_smoke target_text_second_arg_resolves_target_fragments`, and `cargo test --workspace`.

- [ ] **Step 4: Commit and push**

Commit the implementation and push the feature branch per project session completion rules.

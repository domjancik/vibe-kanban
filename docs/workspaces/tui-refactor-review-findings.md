# TUI Refactor Review Findings

Date: 2026-05-07

Scope reviewed:

- `crates/tui`
- the post-refactor module split and follow-up fixes on `vk/cead-design-an-altern`

Validation performed:

- `cargo test -p tui`
- `cargo clippy -p tui -- -D warnings`

## Findings

### 1. Rename editor mode is visually shared but behaviour is still plain text

Severity: Medium

The in-place session rename row renders through the shared editor renderer and uses `self.editor_mode`, so it can visually present Vim-style cursor states.

Relevant code:

- `crates/tui/src/ui/detail.rs:144`
- `crates/tui/src/app/editor.rs:482`

Problem:

- the rename UI can render with Vim-style affordances
- the rename key handler still routes through `map_text_input_key`
- there is no Vim-mode handling for rename input

Impact:

- if the composer was previously switched into Vim Normal, the rename field can look like Vim Normal while keys such as `h`, `j`, `k`, and `l` still insert text instead of navigating
- this creates a misleading editor contract and can cause accidental name edits

Suggested fix:

- either make rename explicitly standard-mode only in both rendering and status language
- or route rename editing through the same editor-mode behaviour used by composer and notes

### 2. Session list scrollbar math still assumes fixed-height rows

Severity: Low

The in-place rename row is taller than a normal session row, but the sessions list viewport and scrollbar calculations still assume each row is 2 lines high.

Relevant code:

- `crates/tui/src/ui/detail.rs:96`
- `crates/tui/src/ui/detail.rs:144`

Problem:

- normal session rows render as 2 lines
- the rename row renders 3 lines
- scrollbar offset and viewport capacity still use `rows_per_item = 2`

Impact:

- scrollbar position can drift while rename is active
- selected-row offset can be slightly wrong in compact session lists

Suggested fix:

- either treat the rename row as a fixed 3-line special case in viewport math
- or move session list scrolling to row-height-aware calculations

### 3. The refactor is not lint-clean under the repository Clippy gate

Severity: Medium

`cargo clippy -p tui -- -D warnings` currently fails.

Relevant code:

- `crates/tui/src/conversation/mod.rs:8`
- `crates/tui/src/conversation/mod.rs:10`
- `crates/tui/src/conversation/mod.rs:13`
- `crates/tui/src/editor/buffer.rs:19`

Observed failures:

- unused public re-exports in `conversation/mod.rs`
- `Iterator::last` on a double-ended iterator in `editor/buffer.rs`

Impact:

- the crate is not lint-clean
- if CI or local verification includes Clippy with warnings denied, this change set will fail the lint step

Suggested fix:

- remove or locally allow the unused re-exports
- replace the flagged iterator call with `next_back()` or equivalent

## Summary

The refactor is broadly sound and `cargo test -p tui` passes, but there are still integration issues around the new in-place rename flow and one lint regression that should be cleaned up before treating the refactor as finished.

## Structural completeness

The refactor is close to complete, but it is not fully cleaned up yet.

There does not appear to be much direct function-level duplication left in `crates/tui`, but there are still structural leftovers that suggest the final consolidation pass has not happened yet.

### Remaining leftovers

- `crates/tui/src/conversation_state.rs` still exists as a top-level module even though there is now a `crates/tui/src/conversation/` directory. This is not direct duplicate logic, but it is a leftover split point with an ambiguous final home.
- `crates/tui/src/api/client.rs` is only a re-export shim.
- `crates/tui/src/api/streams.rs` is only a re-export shim.
- `crates/tui/src/api/client.rs`, `crates/tui/src/api/streams.rs`, and `crates/tui/src/model/mod.rs` use `#[allow(unused_imports)]`, which indicates the final module surface is still carrying compatibility or transitional exports.

### Assessment

- major decomposition work is in place
- obvious business-logic duplication appears low
- final module cleanup and lint cleanup are still pending

The practical conclusion is that the refactor is mostly done structurally, but not fully complete.

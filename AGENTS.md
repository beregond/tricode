# AGENTS.md

## Project structure

Flutter + Rust hybrid using [rinf](https://github.com/cunarist/rinf) as the bridge.

```
lib/                         Flutter/Dart app
  main.dart                  Single-file UI
  src/bindings/              Generated Dart bindings — never hand-edit
native/hub/                  Rust entry-point crate (name must stay "hub")
  src/signals/mod.rs         Source of truth for all signal structs
  src/actors/                One actor per signal pair
native/tricode/              Pure Rust logic (encode, decode, Reed-Solomon)
Cargo.toml                   Workspace root (members = ["native/*"])
```

## Codegen — run after every signal change

Whenever `native/hub/src/signals/mod.rs` is modified (add/rename/remove a struct or field), regenerate the Dart bindings:

```
rinf gen
```

This overwrites everything under `lib/src/bindings/`. The tool must be installed first: `cargo install rinf_cli`. Rinf version in `pubspec.yaml` and `native/hub/Cargo.toml` must match (currently `8.8.1`).

## How signals work

1. Add a struct to `native/hub/src/signals/mod.rs`:
   - `#[derive(Deserialize, DartSignal)]` — Dart → Rust
   - `#[derive(Serialize, RustSignal)]` — Rust → Dart
2. Run `rinf gen`.
3. Rust field `snake_case` becomes Dart `camelCase` automatically.
4. Wire up the actor in `native/hub/src/actors/`.

## Key commands

| Task | Command |
|---|---|
| Run app | `flutter run` |
| Regenerate bindings | `rinf gen` |
| Dart analysis | `flutter analyze` |
| Dart format | `dart format .` |
| Rust build (check) | `cargo build` (from repo root) |
| Rust tests | `cargo test` or `cargo test -p tricode` |
| Rust lint | `cargo clippy` |
| Rust format | `cargo fmt` |
| Flutter tests | `flutter test` |

## Clippy denies in `native/hub`

These are `deny`-level in `native/hub/Cargo.toml` (not in `native/tricode`):
- `clippy::unwrap_used` — no `.unwrap()`
- `clippy::expect_used` — no `.expect()`
- `clippy::wildcard_imports` — no `use foo::*`

## Non-obvious facts

- **`native/hub` crate name is hardcoded** by rinf internals — do not rename it.
- **`lib/src/bindings/` is fully generated** — `rinf gen` overwrites all files there; don't edit them.
- **`base64_encode()` in `native/tricode/src/lib.rs`** is currently dead code (unused after the decode flow was simplified).
- **Rust edition `2024`** requires Rust ≥ 1.85; Cargo workspace resolver `"3"` requires Rust ≥ 1.74. No `.rust-toolchain` file exists, so ensure the local toolchain is recent enough.
- **No CI pipeline** and no Makefile/taskfile — all commands are run directly.

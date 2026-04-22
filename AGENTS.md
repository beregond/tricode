# AGENTS.md

## Project structure

Flutter + Rust hybrid using [rinf](https://github.com/cunarist/rinf) as the bridge.

```
lib/
  main.dart                  Single 675-line UI file (two tabs: Read/Write)
  src/bindings/              Generated Dart bindings — never hand-edit
native/hub/                  Rust entry-point crate (name must stay "hub")
  src/signals/mod.rs         Source of truth for all signal structs
  src/actors/                One actor per signal pair
  src/lib.rs                 Tokio runtime entry (current_thread); write_interface!() macro here
native/tricode/              Pure Rust logic (encode, decode, Reed-Solomon) — single lib.rs
Cargo.toml                   Workspace root (members = ["native/*"]); any new crate in native/ auto-joins
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

## Actor pattern

The `messages = "0.3.1"` crate (from crates.io, not a local path) is the actor framework. Key traits: `Actor`, `Notifiable`, `Address`, `Context`.

Each actor spawns its own Dart-signal listener in `new()` using `JoinSet::spawn`. The loop calls `SomeSignal::get_dart_signal_receiver().recv().await` and forwards via `self_addr.notify(...)`. New actors must follow this same pattern.

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
- **`write_interface!()` macro** in `hub/src/lib.rs` is required by rinf — do not remove it.
- **Tokio runs `current_thread` flavor** in hub — blocking calls inside actors will stall the entire async runtime; use `tokio::task::spawn_blocking` for CPU-heavy work.
- **`tricode::decode_image()` returns `Result<DebugInfo, String>`**, not a plain string. `DebugInfo` holds `text`, `error`, dimensions, triangle counts, and `annotated_png: Vec<u8>`.
- **`image` crate in tricode** uses `default-features = false` with only `png` + `rayon` features — only PNG I/O works; adding other formats requires editing `native/tricode/Cargo.toml`.
- **`base64_encode()` in `native/tricode/src/lib.rs`** is dead code (unused after the decode flow was simplified).
- **`native/tricode` has no `tests/` directory** — `cargo test -p tricode` only runs `#[cfg(test)]` blocks inline in `lib.rs`.
- **Rust edition `2024`** requires Rust ≥ 1.85; workspace resolver `"3"` requires Rust ≥ 1.74. No `.rust-toolchain` file exists.
- **No CI pipeline**, no Makefile/taskfile, no pre-commit hooks — all commands are run directly.
- **Web support is partially wired**: `tokio_with_wasm` and `wasm-bindgen` are already deps in hub. To enable web builds, uncomment `// use tokio_with_wasm::alias as tokio;` in `actors/mod.rs`, `actors/generation.rs`, and `hub/src/lib.rs`.
- **Dart SDK constraint is `^3.10.7`** — Flutter 3.x with Dart 3.10+ required.

# AGENTS.md

## Project structure

Flutter + Rust hybrid using [rinf](https://github.com/cunarist/rinf) as the bridge.

```
lib/
  main.dart                  Main UI file (two tabs: Read/Write)
  overlay.dart               Centre overlay helpers: text and any image format → PNG, clipboard image reading
  src/bindings/              Generated Dart bindings — never hand-edit
native/hub/                  Rust entry-point crate (name must stay "hub")
  src/signals/mod.rs         Source of truth for all signal structs
  src/actors/                One actor per signal pair
  src/lib.rs                 Tokio runtime entry (current_thread); write_interface!() macro here
native/tricode/              Pure Rust logic: lib.rs (encode, decode, frame format) + reed_solomon.rs (GF(256) codec)
Cargo.toml                   Workspace root (members = ["native/*"]); any new crate in native/ auto-joins
```

## Codegen — run after every signal change

Whenever `native/hub/src/signals/mod.rs` is modified (add/rename/remove a struct or field), regenerate the Dart bindings:

```
rinf gen
```

This overwrites everything under `lib/src/bindings/`. The tool must be installed first: `cargo install rinf_cli`. Rinf version in `pubspec.yaml` and `native/hub/Cargo.toml` must match (currently `8.10.0`; also pinned as `RINF_VERSION` in `.github/workflows/ci.yml`).

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
| tricode tests (release, fast) | `tricode-test [cargo test args]` |
| Regenerate decode fixtures | `tricode-regen-fixtures` |
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
- **`native/tricode` has no `tests/` directory** — `cargo test -p tricode` only runs `#[cfg(test)]` blocks inline in `lib.rs` and `reed_solomon.rs`.
- **Decode fixtures** live in `native/tricode/testdata/decode/`, named `{style}_{w}x{h}_len{len}_{rows}x{cols}.png`. After an intentional rendering-format change, rewrite them with `tricode-regen-fixtures`. Tests are slow in debug builds (triangle detection), so prefer `tricode-test`, which runs them with `--release`.
- **Triangle width** (`GenerateImageParams::triangle_width`) is odd, `MIN_TRIANGLE_WIDTH`..=`MAX_TRIANGLE_WIDTH` (37–149 px, default 37), and images over `MAX_IMAGE_PIXELS` (50 MP) are refused; `generate_image_debug` returns `Err` for both and the hub forwards it as `GeneratedImage.error`. The limits are duplicated as constants at the top of `lib/main.dart` — keep them in sync. Borders scale from their 51 px design width; decode fixtures are rendered at 51 px (`FIXTURE_TRIANGLE_WIDTH`).
- **Centre overlays**: `GenerateImageParams::overlay` takes only PNG bytes and a `share` of the code's area; Rust knows nothing about fonts or other image formats. Dart renders text (`textToOverlayPng`) and re-encodes picked or pasted images (`imageToOverlayPng`, scaled to ≤ 1024 px) in `lib/overlay.dart`. Rust drops every cell whose triangle reaches into the centred box, then draws the image scaled to fit inside it. `share` is capped per level by `EccLevel::max_overlay_share` (2/6/15/18%, the solid covers `survives_a_covered_centre` checks), duplicated as `overlayMaxShares` in `lib/overlay.dart`, so keep them in sync. With an overlay, `generate_image_debug` decodes its own output and sets `overlay_decodes`, which adds about 0.5 s; the UI warns when it is `false`.
- **Rendering** classifies every pixel against one shared `TriangleGrid` (vertices on pixel centres horizontally, on row boundaries vertically), so edges are straight lines across rows. Borders aren't drawn per triangle: a pixel stays blank within the border width of an edge between two shapes (`cell_shapes`: per triangle for Tiles, per joined group for Horizon), with strips mitred at corners (`TriangleGrid::on_border`). Pixels the outline crosses are anti-aliased by 4×4 supersampling (`AA_SAMPLES`), and rows render in parallel bands with rayon.
- **Decoding retries with gentler erosion**: `decode_image` runs the whole pipeline at each radius in `EROSION_RADII` (2, 1, 0 px) and stops at the first success; a success after the first adds ` (erosion N)` to `decode_method`. Default-size images succeed at 2, so tests asserting `decode_method == "lattice-fit"` still hold.
- **Code format**: the grid's whole bytes (`grid_capacity`: `(rows·cols − 8) / 8`) hold the frame `[len u16 BE][payload][CRC-32 BE]`, zero-padded, split into Reed-Solomon codewords of ≤ 255 bytes by `block_layout(capacity, level)` and interleaved (`interleave_order`). The layout depends only on capacity and `EccLevel`, which is **not stored** in the code: the decoder tries every level, **highest first**, because a single-codeword grid also decodes at every lower level (nested generator roots). The CRC rejects miscorrections. Old (pre-1.0) codes are not readable.
- **Own Reed-Solomon codec** in `reed_solomon.rs`: crates `reed-solomon` 0.2 and `reed-solomon-erasure` were dropped. The first panics on garbage input (out-of-range slices, `debug_assert` in `gf::div`), and the second only does erasure coding without checking parity. The decoder feeds it every misread grid, so `correct` must return `Err`, never panic; `never_panics_on_garbage` fuzzes that.
- **Erasures**: a cell whose sampled darkness is within `UNSURE_DARKNESS` (0.2–0.8) marks its byte erased, which costs half the correction of a wrong byte. Detailed overlays (logo edges, text) produce them; solid patches are plain errors. `decode_frame` retries each codeword without erasures if they don't help.
- **Mirrored codes decode**: grids always have even rows and odd columns, so a flipped image is still a valid lattice. Each grid is sampled once (`sample_cells`), then read in every `FLIPS` order (`pack_data_bits`); a non-identity match adds ` (flipped horizontally|vertically|both ways)` to `decode_method`.
- **Grid search runs in parallel** (`par_iter().find_map_first` over each estimate's neighbourhood), since every grid costs 4 flips × 7 masks × 4 levels. A decode that fails takes about 0.5 s for small codes and 3 s for the largest on a desktop, mostly triangle detection repeated per erosion radius.
- **Bare UTF-8 payloads** (codes without an envelope) decode as `("plain", text)`. `unpack_payload` tries UTF-8 *before* generic msgpack, because every ASCII byte is a msgpack integer (`"abcde"` would read as `97`).
- **`devbin/`** holds dev scripts; `.envrc` adds it to `PATH` via direnv (run `direnv allow` once). Without direnv, call them as `devbin/<script>`.
- **Rust edition `2024`** requires Rust ≥ 1.85; workspace resolver `"3"` requires Rust ≥ 1.74. No `.rust-toolchain` file exists.
- **CI** is `.github/workflows/ci.yml` (push to `main` and every pull request): rustfmt, clippy with `-D warnings`, `tricode-test`, `rinf gen` + `dart format` check on tracked Dart files + `flutter analyze` + `flutter test`, and shellcheck on `devbin/`. Rust and Flutter versions are pinned there; bump them deliberately. No Makefile/taskfile or pre-commit hooks.
- **Web support is partially wired**: `tokio_with_wasm` and `wasm-bindgen` are already deps in hub. To enable web builds, uncomment `// use tokio_with_wasm::alias as tokio;` in `actors/mod.rs`, `actors/generation.rs`, and `hub/src/lib.rs`.
- **Dart SDK constraint is `^3.10.7`** — Flutter 3.x with Dart 3.10+ required.

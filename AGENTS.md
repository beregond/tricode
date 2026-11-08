# AGENTS.md

## Project structure

Flutter + Rust hybrid using [rinf](https://github.com/cunarist/rinf) as the bridge.

```
lib/
  main.dart                  Entry point, theme
  home_page.dart             Tabs (Read/Generate), the settings dialog (debug info, theme) and the about dialog (version, homepage, licences, copyright)
  read_tab.dart              Read tab: decode from a file or the clipboard
  generate_tab.dart          Generate tab: inputs, options, preview
  overlay_editor.dart        Centre-overlay options (OverlaySettings + OverlayEditor)
  image_io.dart              Save/copy/paste images with snack-bar feedback, reveal in file manager
  code_options.dart          Mask names, triangle-width limits, ECC levels, styles (mirror Rust constants)
  color_presets.dart         Colour presets; color_widgets.dart: swatch picker, checkerboard, preset preview
  debug_tables.dart          Debug info tables; font_picker.dart: installed-font dialog
  logging.dart               Log setup: package:logging, Rust events (RustLog), uncaught-error handlers
  overlay.dart               Centre overlay helpers: text and any image format → PNG, clipboard image reading
  system_fonts.dart          Lists installed font families on desktop (fc-list / macOS channel / PowerShell)
  src/bindings/              Generated Dart bindings — never hand-edit
native/hub/                  Rust entry-point crate (name must stay "hub")
  src/signals/mod.rs         Source of truth for all signal structs
  src/actors/                One actor per signal pair
  src/lib.rs                 Tokio runtime entry (current_thread); write_interface!() macro here
  src/logging.rs             tracing subscriber that forwards events to Dart; panic hook
native/tricode/src/          Pure Rust logic; lib.rs only declares modules and re-exports the public API
  payload.rs                 msgpack envelope and its fallbacks
  frame.rs                   EccLevel, block layout, encode_frame/decode_frame (+ reed_solomon.rs: GF(256) codec)
  grid.rs                    Triangle/image sizes, grid capacity and sizing, cell layout, orientation marks
  mask.rs                    MASKS, mix/unmix, colour transitions
  png.rs                     encode_png
  generate/                  mod.rs (params, generate_image_debug), geometry.rs (TriangleGrid), render.rs
                             (shapes, borders, anti-aliasing), horizon.rs (Horizon grouping), shade.rs
                             (colour variance), overlay.rs
  decode/                    mod.rs (decode_image: cut-offs, polarities, erosion), detect.rs (triangles,
                             anchors), lattice.rs (lattice fit), count.rs (naive/cluster estimates),
                             search.rs (grid strategies), read.rs (sampling, flips, unmasking), annotate.rs
  tests.rs                   End-to-end tests and shared test helpers (`params`, `payload`, …)
native/tricode-cli/          Command-line frontend, binary `tricode` (src/main.rs: `encode`, `decode`); mirrors the Generate/Read options except text overlays and colour presets
docs/privacy.html            Privacy policy for the store listings
Cargo.toml                   Workspace root (members = ["native/*"]); any new crate in native/ auto-joins
```

## Codegen — run after every signal change

Whenever `native/hub/src/signals/mod.rs` is modified (add/rename/remove a struct or field), regenerate the Dart bindings:

```
rinf gen
```

This overwrites everything under `lib/src/bindings/`. The tool must be installed first: `cargo install rinf_cli`. Rinf version in `pubspec.yaml` and `native/hub/Cargo.toml` must match (currently `8.11.0`; also pinned as `RINF_VERSION` in `.github/workflows/ci.yml`).

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
| Run CLI | `cargo run --release -p tricode-cli -- encode "hi" -o hi.png` / `-- decode hi.png` |
| Run app on Linux with Rust logs | `tricode-run` (`RUST_LOG=warn tricode-run` to change the filter) |
| Regenerate bindings | `rinf gen` |
| Dart analysis | `flutter analyze` |
| Dart format | `dart format .` |
| Rust build (check) | `cargo build` (from repo root) |
| Rust tests | `cargo test` or `cargo test -p tricode` |
| tricode tests (release, fast) | `tricode-test [cargo test args]` |
| Regenerate decode fixtures | `tricode-regen-fixtures` |
| Regenerate app icons | `tricode-regen-icons` |
| Regenerate Rust licence list | `tricode-regen-licenses` |
| Rust lint | `cargo clippy` |
| Rust format | `cargo fmt` |
| Flutter tests | `flutter test` |

## Clippy denies in `native/hub`

These are `deny`-level in `native/hub/Cargo.toml` (not in `native/tricode`):
- `clippy::unwrap_used` — no `.unwrap()`
- `clippy::expect_used` — no `.expect()`
- `clippy::wildcard_imports` — no `use foo::*`

Both crates also deny `print_stdout`, `print_stderr` and `dbg_macro`; see Logging.

## Logging

Never print. Rust logs with `tracing` macros (`debug!`, `trace!` with structured fields), Dart with `package:logging` (`Logger('<file>')`, and `catch (e, stack)` blocks pass both to the log).
- **tricode** only emits events and never installs a subscriber. Its tests use `use test_log::test;`, so `RUST_LOG=tricode=trace tricode-test <name> -- --nocapture` shows a test's decode trace (failing tests print it without `--nocapture`).
- **hub** (`src/logging.rs`) installs the subscriber, sends each event to Dart as a `RustLog` signal (spans and fields folded into `message`), and replaces rinf's panic hook so panics are logged in release too. Never log payload text (user content): log its length.
- **Dart** (`lib/logging.dart`) re-logs Rust events as `rust.<target>`, sends records to `dart:developer` (DevTools Logging view) and, in debug builds, to the console via `debugPrint`. It also sets `FlutterError.onError` and `PlatformDispatcher.onError`.
- **Filter**: Rust defaults to hub and tricode at debug, the rest at info. Override with `flutter run --dart-define=RUST_LOG=info,tricode=trace` (`Targets` syntax, sent as `RustLogFilter` at start-up); `tricode-run` does this on Linux, with that filter as its default.
- **Release builds** compile out Rust `debug!`/`trace!` (`release_max_level_info` on hub's `tracing` dependency; set there so `tricode-test`, a release build of tricode alone, keeps them), and Dart drops records below INFO.

## Non-obvious facts

- **`native/hub` crate name is hardcoded** by rinf internals — do not rename it.
- **`lib/src/bindings/` is fully generated** — `rinf gen` overwrites all files there; don't edit them.
- **`write_interface!()` macro** in `hub/src/lib.rs` is required by rinf — do not remove it.
- **Tokio runs `current_thread` flavor** in hub — blocking calls inside actors will stall the entire async runtime; use `tokio::task::spawn_blocking` for CPU-heavy work (the generation and decode actors do).
- **`tricode::decode_image()` returns `Result<DecodeDebug, String>`**, not a plain string. `Err` means no anchors were found at all; a grid that doesn't decode is `Ok` with `text: None` and `error` set. `DecodeDebug` also holds dimensions, triangle counts, `decode_method` and `annotated_png: Vec<u8>`.
- **`image` crate in tricode** uses `default-features = false` with only `png` + `rayon` features — only PNG I/O works; adding other formats requires editing `native/tricode/Cargo.toml`. The CLI has its own `image` dependency with `jpeg` and `webp`, and re-encodes other formats to PNG for decoding and overlays (scaled to ≤ 1024 px like the app).
- **Decode input is PNG only**: the Read tab's `_decode` re-encodes anything else (JPEG files, clipboard JPEGs, camera photos) with `imageToPng` in `lib/overlay.dart` before sending `ImageBytes`. On Android and iOS (`canTakePhoto`) it also offers **Take Photo** (`takePhoto` in `lib/image_io.dart`, `image_picker`, scaled to ≤ 2048 px); iOS needs `NSCameraUsageDescription` in `ios/Runner/Info.plist`.
- **`native/tricode` has no `tests/` directory**: unit tests sit in `#[cfg(test)] mod tests` at the bottom of their module, and end-to-end tests in `src/tests.rs`, whose helpers (`params`, `payload`, `all_cases`, `grid_of`) the module tests reuse. Build test parameters as `GenerateImageParams { style, ..params(&text) }`.
- **Decode fixtures** live in `native/tricode/testdata/decode/`, named `{style}_{w}x{h}_len{len}_{rows}x{cols}.png`. After an intentional rendering-format change, rewrite them with `tricode-regen-fixtures`. Tests are slow in debug builds (triangle detection), so prefer `tricode-test`, which runs them with `--release` but keeps overflow checks on (`CARGO_PROFILE_RELEASE_OVERFLOW_CHECKS`), since the app's debug build panics on overflow.
- **Triangle width** (`GenerateImageParams::triangle_width`) is odd, `MIN_TRIANGLE_WIDTH`..=`MAX_TRIANGLE_WIDTH` (37–149 px, default 37), and images over `MAX_IMAGE_PIXELS` (50 MP) are refused; `generate_image_debug` returns `Err` for both and the hub forwards it as `GeneratedImage.error`. The limits are duplicated in `lib/code_options.dart` — keep them in sync. Borders scale from their 51 px design width; decode fixtures are rendered at 51 px (`FIXTURE_TRIANGLE_WIDTH`).
- **Copy and Download on mobile**: copying an image (`super_clipboard`) needs the `DataProvider` `<provider>` in `android/app/src/main/AndroidManifest.xml`, or Android reports success while the clipboard stays empty. Download (`savePng`) puts the image in the photo gallery with `gal` on Android and iOS (`NSPhotoLibraryAddUsageDescription` in `Info.plist`, `WRITE_EXTERNAL_STORAGE` up to API 29), because the app's own storage directory is invisible to gallery and file manager apps.
- **Centre overlays**: `GenerateImageParams::overlay` takes only PNG bytes and a `share` of the code's area; Rust knows nothing about fonts or other image formats. Dart renders text (`textToOverlayPng`, styled by `OverlayTextStyle`: font from `overlayFonts`, bold, italic, colour (may be transparent), and an optional outline drawn as a stroke under the fill; by default the text takes the primary colour and the outline the secondary; cropped to its ink) and re-encodes picked or pasted images (`imageToOverlayPng`, scaled to ≤ 1024 px) in `lib/overlay.dart`. Rust drops every cell whose triangle reaches into the centred box, then draws the image scaled to fit inside it. `share` is capped per level by `EccLevel::max_overlay_share` (2/6/15/18%, the solid covers `survives_a_covered_centre` checks), duplicated as `EccLevel.maxOverlayShare` in `lib/overlay.dart`, so keep them in sync.
- **Overlay fonts** are bundled OFL fonts in `assets/fonts/<Family>/` (font file + `OFL.txt`), declared under `fonts:` and the licences under `assets:` in `pubspec.yaml`, and listed in `overlayFonts`; `registerOverlayFontLicenses()` adds them to the licence page. `flutter test` doesn't load them (it renders a placeholder font), so check new fonts visually. On desktop the picker also offers installed fonts (`installedFontFamilies`, drawn by family name via `OverlayFont.installed`): Linux runs `fc-list`, macOS answers the `tricode/fonts` channel in `macos/Runner/MainFlutterWindow.swift`, Windows asks .NET through PowerShell. `flutter test` points fontconfig at its own font set, so `fc-list` there sees only a few fonts.
- **Three colours**: primary triangles (bit 1, `primary_color`, always opaque), secondary triangles (bit 0, `secondary_color`) and the background (`bg_color`: margin and the borders between shapes). Both primary and secondary cells are drawn as shapes (`render::Shape`; Horizon groups each colour with its own `HorizonLayout`). Secondary defaults to the background colour, which renders exactly as if only primary triangles were drawn; presets give it its own colour with `ColorPreset(..., secondary:)`. **Colour variance** (`primary_variance`, `secondary_variance`, `generate/shade.rs`) raises each shape's HSV value by 0 to 7 points (`MAX_VALUE_SHIFT`; never lowers it, clamped at 100 % rather than wrapping), picked by hashing `seed` with the shape; Stain gives every cell its own shape id for this (it has no borders). `seed` (0 = clock, resolved once in `generate/mod.rs`) also drives the Horizon grouping.
- **Rendering** classifies every pixel against one shared `TriangleGrid` (vertices on pixel centres horizontally, on row boundaries vertically), so edges are straight lines across rows. Borders aren't drawn per triangle: a pixel stays blank within the border width of an edge between two shapes (`cell_shapes`: per triangle for Tiles, per joined group for Horizon), with strips mitred at corners (`TriangleGrid::on_border`). Pixels the outline crosses are anti-aliased by 4×4 supersampling (`AA_SAMPLES`), and rows render in parallel bands with rayon.
- **Decoding retries with gentler erosion**: `decode_image` runs the whole pipeline at each radius in `EROSION_RADII` (2, 1, 0 px) and stops at the first success; a success after the first adds ` (erosion N)` to `decode_method`. Default-size images succeed at 2, so tests asserting `decode_method == "lattice-fit"` still hold.
- **Code format**: the grid's whole bytes (`grid_capacity`: `(rows·cols − 8) / 8`) hold the frame `[len u16 BE][payload][CRC-32 BE]`, zero-padded, split into Reed-Solomon codewords of ≤ 255 bytes by `block_layout(capacity, level)` and interleaved (`interleave_order`). The layout depends only on capacity and `EccLevel`, which is **not stored** in the code: the decoder tries every level, **highest first**, because a single-codeword grid also decodes at every lower level (nested generator roots). The CRC rejects miscorrections. Old (pre-1.0) codes are not readable.
- **Own Reed-Solomon codec** in `reed_solomon.rs`: crates `reed-solomon` 0.2 and `reed-solomon-erasure` were dropped. The first panics on garbage input (out-of-range slices, `debug_assert` in `gf::div`), and the second only does erasure coding without checking parity. The decoder feeds it every misread grid, so `correct` must return `Err`, never panic; `never_panics_on_garbage` fuzzes that.
- **Erasures**: a cell whose sampled darkness is within `UNSURE_DARKNESS` (0.2–0.8) marks its byte erased, which costs half the correction of a wrong byte. Detailed overlays (logo edges, text) produce them; solid patches are plain errors. `decode_frame` retries each codeword without erasures if they don't help.
- **Mirrored codes decode**: grids always have even rows and odd columns, so a flipped image is still a valid lattice. Each grid is sampled once (`sample_cells`), then read in every `FLIPS` order (`pack_data_bits`); a non-identity match adds ` (flipped horizontally|vertically|both ways)` to `decode_method`.
- **Every generated code is read back**: `generate_image_debug` sets `decode_warning` (forwarded as `GeneratedImage.decode_warning`, empty when fine) and the UI shows it. Primary and secondary colours whose brightness differs by less than `MIN_LUMA_GAP` (24, a guess not measured on photos) are reported at once; otherwise it reads its own output with `decode_rendered`, the decoder's fast lane: only the Otsu cut-off, the guessed polarity, erosion 2 and the fitted lattice, unflipped (`read_fitted_grid`). A pristine image that fails that fails every fallback too, so a failing check costs one detection pass instead of all of `decode_image`. Colour presets in `lib/color_presets.dart` (`colorPresets`) are checked against the same gap by `test/color_presets_test.dart`.
- **Otsu cut-off**: `decode_image` turns the grayscale image black and white at its Otsu level (`otsu_level`), so any two colours far enough apart decode, both light or both dark included. The fixed mid-grey `DARK_LUMA` (128) is a fallback, tried only when it splits over 1% of pixels differently (`splits_differently`); a success with it adds ` (fixed cut-off)` to `decode_method`. Everything after that step reads the black-and-white image.
- **Negative codes decode**: `decode_image` also runs the whole erosion loop on the inverted grayscale image (light triangles on a dark background), and a match adds ` (inverted)` to `decode_method` (after any flip/erosion suffix). The polarity tried first is guessed from `edge_luminance` (the margin is background), so both polarities decode at normal speed; only a failing decode pays twice.
- **Grid search runs in parallel** (`par_iter().find_map_first` over each estimate's neighbourhood), since every grid costs 4 flips × 7 masks × 4 levels. A decode that fails takes about 0.5 s for small codes and 3 s for the largest on a desktop, mostly triangle detection repeated per erosion radius.
- **Bare UTF-8 payloads** (codes without an envelope) decode as `("plain", text)`. `unpack_payload` tries UTF-8 *before* generic msgpack, because every ASCII byte is a msgpack integer (`"abcde"` would read as `97`).
- **Binary names**: the CLI is `tricode`, the Flutter desktop app is `tricode-gui` (`BINARY_NAME` in `linux/` and `windows/CMakeLists.txt`, `PRODUCT_NAME` in `macos/Runner/Configs/AppInfo.xcconfig`, plus the `.app` references in the Xcode project), so both install side by side. The Dart package is still `tricode_app`. The CLI crate may print (it has no `print_*` denies); decoded text goes to stdout, everything else (including `--verbose` debug info) to stderr, and logs follow `RUST_LOG` (default `warn`).
- **App ID** is `io.github.beregond.tricode` on Android, iOS, macOS and Linux (`applicationId`, `PRODUCT_BUNDLE_IDENTIFIER`, `APPLICATION_ID`); stores tie the listing to it, so it must not change after the first upload.
- **Licence page**: Flutter's page (About → View licenses) finds only Dart packages, so `registerAppLicenses` in `lib/home_page.dart` adds Tricode's `LICENSE` and `assets/licenses/rust.json`, the crates compiled into the app on every shipped platform. `tricode-regen-licenses` writes that file with `cargo-about` (`cargo install cargo-about --features cli`, version pinned as `CARGO_ABOUT_VERSION` in `ci.yml`) and `jq`, configured by `about.toml`; rerun it after any `Cargo.lock` change or CI fails. A dependency under a new licence must be added to `accepted` there.
- **Privacy policy** is `docs/privacy.html`, linked from the store listings. It promises no network access and no data collection: update it when adding a permission, networking or anything that handles user data.
- **App icons** and the in-app logo (`assets/logo/tricode.png`, shown by `TricodeLogo`) are all rendered from `assets/logo/tricode.svg` by `tricode-regen-icons` (needs `rsvg-convert` and ImageMagick); edit the SVG, never the PNGs. iOS and web maskable icons get a white background (App Store refuses alpha; maskable icons are cropped). Linux installs `linux/runner/resources/app_icon.png` into the bundle's `data/` and `my_application.cc` sets it on the window.
- **`devbin/`** holds dev scripts; `.envrc` adds it to `PATH` via direnv (run `direnv allow` once). Without direnv, call them as `devbin/<script>`.
- **Rust edition `2024`** requires Rust ≥ 1.85; workspace resolver `"3"` requires Rust ≥ 1.74. No `.rust-toolchain` file exists.
- **CI** is `.github/workflows/ci.yml` (push to `main` and every pull request): rustfmt, clippy with `-D warnings`, `tricode-test`, a fixture check (runs `tricode-regen-fixtures` and fails on any diff, since stale fixtures can still decode), a licence-list check (same idea with `tricode-regen-licenses`), `rinf gen` + `dart format` check on tracked Dart files + `flutter analyze` + `flutter test`, shellcheck on `devbin/`, and actionlint on the workflows. Cargo runs with `--locked` and `flutter pub get` with `--enforce-lockfile`, so commit lockfile changes. Rust and Flutter versions are pinned there; bump them deliberately. No Makefile/taskfile or pre-commit hooks.
- **Web support is partially wired**: `tokio_with_wasm` and `wasm-bindgen` are already deps in hub. To enable web builds, uncomment `// use tokio_with_wasm::alias as tokio;` in `actors/mod.rs`, `actors/generation.rs`, `actors/decode.rs`, and `hub/src/lib.rs`.
- **Dart SDK constraint is `^3.10.7`** — Flutter 3.x with Dart 3.10+ required.

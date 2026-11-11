import 'package:flutter/material.dart';

/// Ready-made code colours offered above the colour pickers: primary triangles
/// (bit 1), secondary triangles (bit 0) and the background. Secondary triangles
/// take the background colour unless given their own.
class ColorPreset {
  const ColorPreset(
    this.name,
    this.primary,
    this.background, {
    Color? secondary,
  }) : _secondary = secondary;

  final String name;
  final Color primary;
  final Color background;
  final Color? _secondary;

  Color get secondary => _secondary ?? background;

  bool matches(Color primary, Color secondary, Color background) =>
      primary.toARGB32() == this.primary.toARGB32() &&
      secondary.toARGB32() == this.secondary.toARGB32() &&
      background.toARGB32() == this.background.toARGB32();
}

/// Suggested the first time the Horizon style is picked.
const zeroDawnPreset = ColorPreset(
  'Zero Dawn',
  Color(0xFF4a97dd),
  Color(0xFF90c2e3),
);

/// The primary and secondary colours of a preset must differ in brightness by at least
/// `MIN_LUMA_GAP` (native/tricode-core/src/decode/mod.rs); either may be the
/// lighter one. The first is the default black on white.
const colorPresets = [
  ColorPreset('Classic', Color(0xFF000000), Color(0xFFFFFFFF)),
  zeroDawnPreset,
  ColorPreset('Ink', Color(0xFF1B2A41), Color(0xFFF4EFE6)),
  ColorPreset('Forest', Color(0xFF1F4D3A), Color(0xFFE8F0E3)),
  ColorPreset('Terracotta', Color(0xFF7A2E1F), Color(0xFFF6E7D8)),
  ColorPreset('Plum', Color(0xFF4A1F4D), Color(0xFFF3E9F2)),
  ColorPreset('Night', Color(0xFFF2F4F5), Color(0xFF1E2A33)),
  ColorPreset('Blueprint', Color(0xFFFFFFFF), Color(0xFF1F4E79)),
];

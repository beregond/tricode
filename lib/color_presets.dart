import 'package:flutter/material.dart';

/// A ready-made pair of code colours offered above the colour pickers.
class ColorPreset {
  const ColorPreset(this.name, this.foreground, this.background);

  final String name;
  final Color foreground;
  final Color background;

  bool matches(Color foreground, Color background) =>
      foreground.toARGB32() == this.foreground.toARGB32() &&
      background.toARGB32() == this.background.toARGB32();
}

/// Suggested the first time the Horizon style is picked.
const zeroDawnPreset = ColorPreset(
  'Zero Dawn',
  Color(0xFF90ADD3),
  Color(0xFFC0CAD7),
);

/// The two colours of a preset must differ in brightness by at least
/// `MIN_LUMA_GAP` (native/tricode/src/decode/mod.rs); either may be the
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

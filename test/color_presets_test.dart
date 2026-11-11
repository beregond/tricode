import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';

import 'package:tricode_app/color_presets.dart';

/// Grayscale value the decoder sees (the `image` crate's BT.709 luma).
double _luma(Color c) =>
    0.2126 * c.r * 255 + 0.7152 * c.g * 255 + 0.0722 * c.b * 255;

void main() {
  test('every preset is far enough apart in brightness to decode', () {
    // `MIN_LUMA_GAP` in native/tricode-core/src/decode/mod.rs.
    const minLumaGap = 24;
    for (final preset in colorPresets) {
      final gap = (_luma(preset.primary) - _luma(preset.secondary)).abs();
      expect(
        gap >= minLumaGap,
        isTrue,
        reason: '${preset.name}: brightness gap $gap',
      );
    }
  });

  test('presets have unique names and colours', () {
    final names = colorPresets.map((p) => p.name).toSet();
    final triples = colorPresets
        .map(
          (p) => (
            p.primary.toARGB32(),
            p.secondary.toARGB32(),
            p.background.toARGB32(),
          ),
        )
        .toSet();
    expect(names.length, colorPresets.length);
    expect(triples.length, colorPresets.length);
  });
}

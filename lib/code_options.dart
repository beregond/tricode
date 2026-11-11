/// Choices offered for a generated code, mirroring constants in the Rust
/// crate (native/tricode-core/src); keep them in sync.
library;

import 'src/bindings/bindings.dart';

/// Data masks the generator can XOR the payload with, in the same order as
/// `MASK_NAMES` / `MASKS` in native/tricode-core/src/mask.rs (the index is sent to
/// Rust).
const maskNames = [
  'Checkerboard',
  '2×2 blocks',
  'Right wave',
  'Left wave',
  'Vertical lines',
  'Horizontal lines',
  'None',
];

/// Triangle base width limits in pixels, matching `MIN_TRIANGLE_WIDTH`,
/// `DEFAULT_TRIANGLE_WIDTH` and `MAX_TRIANGLE_WIDTH` in
/// native/tricode-core/src/grid.rs.
const minTriangleWidth = 37;
const defaultTriangleWidth = 37;
const maxTriangleWidth = 149;

/// Preset triangle widths offered in the size dropdown.
const triangleWidthPresets = [
  (defaultTriangleWidth, 'Standard'),
  (75, 'High (×2)'),
  (maxTriangleWidth, 'Print (×4)'),
];

/// Why [text] is not an allowed triangle width, or null when it is.
String? triangleWidthError(String text) {
  final width = int.tryParse(text.trim());
  if (width == null) return 'Enter a number';
  if (width < minTriangleWidth || width > maxTriangleWidth) {
    return 'Must be $minTriangleWidth–$maxTriangleWidth px';
  }
  if (width.isEven) return 'Must be odd';
  return null;
}

extension EccLevelLabel on EccLevel {
  /// The level's name, with how much of the code it survives being damaged.
  String get label => switch (this) {
    EccLevel.low => 'Low (~7% damage)',
    EccLevel.medium => 'Medium (~15% damage)',
    EccLevel.quartile => 'Quartile (~25% damage)',
    EccLevel.high => 'High (~30% damage)',
  };
}

/// The level codes are generated at by default, as `DEFAULT_ECC_LEVEL`.
const defaultEccLevel = EccLevel.medium;

extension CodeStyleLabel on CodeStyle {
  String get label => switch (this) {
    CodeStyle.tiles => 'Tiles',
    CodeStyle.stain => 'Stain',
    CodeStyle.horizon => 'Horizon',
  };
}

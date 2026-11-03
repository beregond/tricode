/// Choices offered for a generated code, mirroring constants in the Rust
/// crate (native/tricode/src); keep them in sync.
library;

/// Data masks the generator can XOR the payload with, in the same order as
/// `MASK_NAMES` / `MASKS` in native/tricode/src/mask.rs (the index is sent to
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
/// native/tricode/src/grid.rs.
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

/// Error-correction levels, indexed as `SmallText.eccLevel` expects, with how
/// much of the code each survives being damaged.
const eccLevelNames = [
  'Low (~7% damage)',
  'Medium (~15% damage)',
  'Quartile (~25% damage)',
  'High (~30% damage)',
];

/// The level codes are generated at by default: Medium.
const defaultEccLevel = 1;

/// Code styles, indexed as `SmallText.style` expects.
enum CodeStyle {
  tiles('Tiles'),
  stain('Stain'),
  horizon('Horizon');

  const CodeStyle(this.label);

  final String label;
}

/// Small widgets for showing and picking colours.
library;

import 'package:flutter/material.dart';
import 'package:flutter_colorpicker/flutter_colorpicker.dart';

import 'color_presets.dart';

/// A colour sample with a label; tapping it opens a colour picker.
class ColorSwatchButton extends StatelessWidget {
  const ColorSwatchButton({
    super.key,
    required this.label,
    required this.color,
    required this.showAlpha,
    required this.onChanged,
  });

  final String label;
  final Color color;
  final bool showAlpha;
  final ValueChanged<Color> onChanged;

  void _openPicker(BuildContext context) {
    Color pickerColor = color;

    showDialog<void>(
      context: context,
      builder: (ctx) => StatefulBuilder(
        builder: (ctx, setS) => AlertDialog(
          title: Text('Pick ${label.toLowerCase()} color'),
          content: SingleChildScrollView(
            child: Column(
              mainAxisSize: MainAxisSize.min,
              children: [
                ColorPicker(
                  pickerColor: pickerColor,
                  onColorChanged: (c) => setS(() => pickerColor = c),
                  enableAlpha: showAlpha,
                  labelTypes: const [],
                  hexInputBar: true,
                ),
              ],
            ),
          ),
          actions: [
            TextButton(
              onPressed: () => Navigator.of(ctx).pop(),
              child: const Text('Cancel'),
            ),
            TextButton(
              onPressed: () {
                Navigator.of(ctx).pop();
                onChanged(pickerColor);
              },
              child: const Text('Select'),
            ),
          ],
        ),
      ),
    );
  }

  @override
  Widget build(BuildContext context) {
    final isTransparent = color.a == 0;
    return GestureDetector(
      onTap: () => _openPicker(context),
      child: Row(
        mainAxisSize: MainAxisSize.min,
        children: [
          Container(
            width: 28,
            height: 28,
            decoration: BoxDecoration(
              border: Border.all(color: Colors.grey),
              borderRadius: BorderRadius.circular(4),
              color: isTransparent ? null : color,
            ),
            child: isTransparent
                ? ClipRRect(
                    borderRadius: BorderRadius.circular(3),
                    child: CustomPaint(painter: CheckerPainter()),
                  )
                : null,
          ),
          const SizedBox(width: 6),
          Text(label, style: const TextStyle(fontSize: 13)),
        ],
      ),
    );
  }
}

/// A grey and white checkerboard, shown behind transparent images.
class CheckerPainter extends CustomPainter {
  @override
  void paint(Canvas canvas, Size size) {
    const cellSize = 4.0;
    final light = Paint()..color = Colors.white;
    final dark = Paint()..color = const Color(0xFFCCCCCC);
    for (double y = 0; y < size.height; y += cellSize) {
      for (double x = 0; x < size.width; x += cellSize) {
        final isLight =
            ((x / cellSize).floor() + (y / cellSize).floor()) % 2 == 0;
        canvas.drawRect(
          Rect.fromLTWH(x, y, cellSize, cellSize),
          isLight ? light : dark,
        );
      }
    }
  }

  @override
  bool shouldRepaint(CheckerPainter old) => false;
}

/// A primary and a secondary triangle in the preset's colours on its background.
class PresetPreview extends StatelessWidget {
  const PresetPreview({super.key, required this.preset});

  final ColorPreset preset;

  @override
  Widget build(BuildContext context) {
    return Container(
      decoration: BoxDecoration(
        color: preset.background,
        border: Border.all(color: Colors.grey),
        borderRadius: BorderRadius.circular(4),
      ),
      child: CustomPaint(
        size: const Size.square(18),
        painter: _TrianglePainter(preset.primary, preset.secondary),
      ),
    );
  }
}

class _TrianglePainter extends CustomPainter {
  _TrianglePainter(this.primary, this.secondary);

  final Color primary;
  final Color secondary;

  @override
  void paint(Canvas canvas, Size size) {
    // An upward primary triangle and a downward secondary one sharing an edge, as in a
    // code; the secondary one disappears when it takes the background colour.
    final inset = size.width * 0.15;
    final (left, top) = (inset, inset);
    final (width, bottom) = (size.width - 2 * inset, size.height - inset);
    final third = width / 3;
    final up = Path()
      ..moveTo(left, bottom)
      ..lineTo(left + 2 * third, bottom)
      ..lineTo(left + third, top)
      ..close();
    final down = Path()
      ..moveTo(left + third, top)
      ..lineTo(left + width, top)
      ..lineTo(left + 2 * third, bottom)
      ..close();
    canvas.drawPath(up, Paint()..color = primary);
    canvas.drawPath(down, Paint()..color = secondary);
  }

  @override
  bool shouldRepaint(_TrianglePainter oldDelegate) =>
      oldDelegate.primary != primary || oldDelegate.secondary != secondary;
}

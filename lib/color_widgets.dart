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
          title: Text('Pick $label color'),
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

/// A small triangle in the preset's triangle colour on its background.
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
        painter: _TrianglePainter(preset.foreground),
      ),
    );
  }
}

class _TrianglePainter extends CustomPainter {
  _TrianglePainter(this.color);

  final Color color;

  @override
  void paint(Canvas canvas, Size size) {
    final inset = size.width * 0.2;
    final path = Path()
      ..moveTo(size.width / 2, inset)
      ..lineTo(size.width - inset, size.height - inset)
      ..lineTo(inset, size.height - inset)
      ..close();
    canvas.drawPath(path, Paint()..color = color);
  }

  @override
  bool shouldRepaint(_TrianglePainter oldDelegate) =>
      oldDelegate.color != color;
}

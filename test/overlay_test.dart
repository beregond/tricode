import 'dart:typed_data';
import 'dart:ui' as ui;

import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';

import 'package:tricode_app/overlay.dart';

/// Width and height of encoded image [bytes].
Future<(int, int)> _size(Uint8List bytes) async {
  final codec = await ui.instantiateImageCodec(bytes);
  final frame = await codec.getNextFrame();
  return (frame.image.width, frame.image.height);
}

/// Width, height and alpha channel of encoded image [bytes].
Future<(int, int, List<int>)> _alpha(Uint8List bytes) async {
  final codec = await ui.instantiateImageCodec(bytes);
  final image = (await codec.getNextFrame()).image;
  final data = await image.toByteData(format: ui.ImageByteFormat.rawRgba);
  final rgba = data!.buffer.asUint8List();
  return (
    image.width,
    image.height,
    [for (var i = 3; i < rgba.length; i += 4) rgba[i]],
  );
}

bool _isPng(Uint8List bytes) =>
    bytes.length > 8 &&
    bytes[0] == 0x89 &&
    bytes[1] == 0x50 &&
    bytes[2] == 0x4E &&
    bytes[3] == 0x47;

void main() {
  testWidgets('text renders to a wide PNG', (tester) async {
    await tester.runAsync(() async {
      final png = await textToOverlayPng(
        'Hello',
        const OverlayTextStyle(color: Colors.black),
      );
      expect(_isPng(png), isTrue);
      final (width, height) = await _size(png);
      expect(width, greaterThan(height));
    });
  });

  testWidgets('text in every font and style is cropped to its ink', (
    tester,
  ) async {
    await tester.runAsync(() async {
      for (final font in overlayFonts) {
        for (final (bold, italic, outline) in [
          (false, false, false),
          (true, true, false),
          (true, false, true),
        ]) {
          final label = '${font.name} b=$bold i=$italic o=$outline';
          final png = await textToOverlayPng(
            'Hey',
            OverlayTextStyle(
              color: Colors.black,
              font: font,
              bold: bold,
              italic: italic,
              outlineColor: outline ? Colors.white : null,
            ),
          );
          final (width, height, alpha) = await _alpha(png);
          // Ink reaches within the 2 px crop margin (plus 1 for rounding) of
          // every side.
          bool inkAt(int x, int y) => alpha[y * width + x] > 0;
          final nearEdge = <String, bool>{
            'left': [
              for (var y = 0; y < height; y++)
                for (var x = 0; x <= 3; x++) inkAt(x, y),
            ].any((v) => v),
            'right': [
              for (var y = 0; y < height; y++)
                for (var x = width - 4; x < width; x++) inkAt(x, y),
            ].any((v) => v),
            'top': [
              for (var y = 0; y <= 3; y++)
                for (var x = 0; x < width; x++) inkAt(x, y),
            ].any((v) => v),
            'bottom': [
              for (var y = height - 4; y < height; y++)
                for (var x = 0; x < width; x++) inkAt(x, y),
            ].any((v) => v),
          };
          expect(
            nearEdge.values.every((v) => v),
            isTrue,
            reason: '$label: $nearEdge',
          );
        }
      }
    });
  });

  testWidgets('an outline widens the text, and hollow text covers less', (
    tester,
  ) async {
    await tester.runAsync(() async {
      Future<int> inked(Color color, Color? outlineColor) async {
        final png = await textToOverlayPng(
          'O',
          OverlayTextStyle(color: color, outlineColor: outlineColor),
        );
        final (_, _, alpha) = await _alpha(png);
        return alpha.where((a) => a > 128).length;
      }

      final filled = await inked(Colors.black, null);
      expect(await inked(Colors.black, Colors.white), greaterThan(filled));
      expect(
        await inked(Colors.transparent, Colors.white),
        lessThan(filled * 0.8),
      );
    });
  });

  testWidgets('large images are scaled down keeping their aspect', (
    tester,
  ) async {
    await tester.runAsync(() async {
      final recorder = ui.PictureRecorder();
      Canvas(recorder).drawRect(
        const Rect.fromLTWH(0, 0, 2048, 512),
        Paint()..color = Colors.red,
      );
      final image = await recorder.endRecording().toImage(2048, 512);
      final data = await image.toByteData(format: ui.ImageByteFormat.png);
      final png = await imageToOverlayPng(data!.buffer.asUint8List());
      expect(_isPng(png), isTrue);
      expect(await _size(png), (overlayMaxSide, overlayMaxSide ~/ 4));
    });
  });
}

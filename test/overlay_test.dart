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

bool _isPng(Uint8List bytes) =>
    bytes.length > 8 &&
    bytes[0] == 0x89 &&
    bytes[1] == 0x50 &&
    bytes[2] == 0x4E &&
    bytes[3] == 0x47;

void main() {
  testWidgets('text renders to a wide PNG', (tester) async {
    await tester.runAsync(() async {
      final png = await textToOverlayPng('Hello', Colors.black);
      expect(_isPng(png), isTrue);
      final (width, height) = await _size(png);
      expect(width, greaterThan(height));
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

/// Images drawn over the centre of a generated code. Rust only receives PNG
/// bytes, so everything that knows about fonts or image formats lives here.
library;

import 'dart:async';
import 'dart:math';
import 'dart:typed_data';
import 'dart:ui' as ui;

import 'package:flutter/material.dart';
import 'package:super_clipboard/super_clipboard.dart';

/// Largest share of the code's area an overlay may take at each error
/// correction level (indexed like `SmallText.eccLevel`), matching
/// `EccLevel::max_overlay_share` in native/tricode/src/lib.rs.
const overlayMaxShares = [0.02, 0.06, 0.15, 0.18];

/// Longest side, in pixels, overlay images are scaled down to before being
/// sent. Rust scales them again to fit the code, so more would only cost time.
const overlayMaxSide = 1024;

/// Font size text overlays are rendered at; Rust scales the image down to fit.
const _overlayFontSize = 160.0;

/// Decodes an image in any format Flutter reads (PNG, JPEG, GIF, WebP, BMP…)
/// and re-encodes it as PNG, no larger than [overlayMaxSide] on either side.
Future<Uint8List> imageToOverlayPng(Uint8List bytes) async {
  final buffer = await ui.ImmutableBuffer.fromUint8List(bytes);
  final descriptor = await ui.ImageDescriptor.encoded(buffer);
  final scale = min(
    1.0,
    overlayMaxSide / max(descriptor.width, descriptor.height),
  );
  final codec = await descriptor.instantiateCodec(
    targetWidth: max(1, (descriptor.width * scale).round()),
    targetHeight: max(1, (descriptor.height * scale).round()),
  );
  final frame = await codec.getNextFrame();
  try {
    return await _encodePng(frame.image);
  } finally {
    frame.image.dispose();
    codec.dispose();
    descriptor.dispose();
    buffer.dispose();
  }
}

/// Renders [text] (centred, one or more lines) in [color] on a transparent
/// background, as PNG bytes.
Future<Uint8List> textToOverlayPng(String text, Color color) async {
  final painter = TextPainter(
    text: TextSpan(
      text: text,
      style: TextStyle(
        color: color,
        fontSize: _overlayFontSize,
        fontWeight: FontWeight.bold,
        height: 1.1,
      ),
    ),
    textAlign: TextAlign.center,
    textDirection: TextDirection.ltr,
  )..layout();
  // A little room around the glyphs so descenders and overhangs aren't cut.
  const padding = _overlayFontSize * 0.1;
  final width = (painter.width + 2 * padding).ceil();
  final height = (painter.height + 2 * padding).ceil();
  final recorder = ui.PictureRecorder();
  painter.paint(Canvas(recorder), const Offset(padding, padding));
  painter.dispose();
  final picture = recorder.endRecording();
  final image = await picture.toImage(width, height);
  try {
    return await _encodePng(image);
  } finally {
    image.dispose();
    picture.dispose();
  }
}

Future<Uint8List> _encodePng(ui.Image image) async {
  final data = await image.toByteData(format: ui.ImageByteFormat.png);
  if (data == null) throw StateError('Failed to encode PNG');
  return data.buffer.asUint8List();
}

/// Thrown by [readClipboardImage] when this platform has no clipboard access.
class ClipboardUnavailable implements Exception {
  const ClipboardUnavailable();
}

/// The image on the clipboard as PNG or JPEG bytes, or null when there is
/// none. Throws [ClipboardUnavailable] where the clipboard can't be read.
Future<Uint8List?> readClipboardImage() async {
  final clipboard = SystemClipboard.instance;
  if (clipboard == null) throw const ClipboardUnavailable();
  final reader = await clipboard.read();
  for (final format in [Formats.png, Formats.jpeg]) {
    if (!reader.canProvide(format)) continue;
    final completer = Completer<Uint8List?>();
    reader.getFile(format, (file) async {
      try {
        completer.complete(await file.readAll());
      } catch (e) {
        completer.complete(null);
      }
    });
    final bytes = await completer.future;
    if (bytes != null) return bytes;
  }
  return null;
}

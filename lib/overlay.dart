/// Images drawn over the centre of a generated code. Rust only receives PNG
/// bytes, so everything that knows about fonts or image formats lives here.
library;

import 'dart:async';
import 'dart:math';
import 'dart:ui' as ui;

import 'package:flutter/foundation.dart';
import 'package:flutter/material.dart';
import 'package:flutter/services.dart';
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

/// A font offered for text overlays: one of [overlayFonts], or one installed
/// on the computer.
@immutable
class OverlayFont {
  const OverlayFont(this.name, this.family, {this.variableWeight = false});

  /// An installed font, drawn by asking the system for [family].
  const OverlayFont.installed(String family) : this(family, family);

  /// Shown in the font picker.
  final String name;

  /// Family registered in pubspec.yaml; null for the platform's own font.
  final String? family;

  /// Whether the font file covers every weight (a variable font), so bold is
  /// drawn with the font's real bold rather than a synthesised one.
  final bool variableWeight;

  @override
  bool operator ==(Object other) =>
      other is OverlayFont && other.name == name && other.family == family;

  @override
  int get hashCode => Object.hash(name, family);
}

/// The platform's own font, used when no other is chosen.
const platformFont = OverlayFont('System', null);

/// Fonts offered for text overlays; all but the first are bundled in
/// assets/fonts so the text looks the same on every platform.
const overlayFonts = [
  platformFont,
  OverlayFont('Serif', 'Lora', variableWeight: true),
  OverlayFont('Mono', 'JetBrainsMono', variableWeight: true),
  OverlayFont('Display', 'BebasNeue'),
  OverlayFont('Script', 'Pacifico'),
  OverlayFont('Handwriting', 'Caveat', variableWeight: true),
];

/// How text overlays are drawn.
@immutable
class OverlayTextStyle {
  const OverlayTextStyle({
    required this.color,
    this.font = platformFont,
    this.bold = true,
    this.italic = false,
    this.outline = false,
  });

  final Color color;

  final OverlayFont font;
  final bool bold;
  final bool italic;

  /// Draw only the letters' outlines instead of filling them.
  final bool outline;

  /// The Flutter text style for drawing at [fontSize], also used to preview
  /// fonts in the picker.
  TextStyle toTextStyle(double fontSize) {
    final weight = bold ? FontWeight.w700 : FontWeight.w400;
    return TextStyle(
      fontFamily: font.family,
      fontSize: fontSize,
      fontWeight: weight,
      fontVariations: font.variableWeight
          ? [FontVariation.weight(weight.value.toDouble())]
          : null,
      fontStyle: italic ? FontStyle.italic : FontStyle.normal,
      height: 1.1,
      // An outline replaces the fill colour with a stroke in that colour.
      color: outline ? null : color,
      foreground: outline
          ? (Paint()
              ..style = PaintingStyle.stroke
              ..strokeWidth = fontSize * _outlineWidth
              ..strokeJoin = StrokeJoin.round
              ..color = color)
          : null,
    );
  }
}

/// Outline stroke width as a share of the font size.
const _outlineWidth = 0.05;

/// Renders [text] (centred, one or more lines) in [style] on a transparent
/// background, as PNG bytes.
Future<Uint8List> textToOverlayPng(String text, OverlayTextStyle style) async {
  final painter = TextPainter(
    text: TextSpan(text: text, style: style.toTextStyle(_overlayFontSize)),
    textAlign: TextAlign.center,
    textDirection: TextDirection.ltr,
  )..layout();
  // Generous room around the layout box, since slanted and script glyphs and
  // the outline stroke reach past it; the result is cropped to the ink.
  const padding = _overlayFontSize * 0.5;
  final width = (painter.width + 2 * padding).ceil();
  final height = (painter.height + 2 * padding).ceil();
  final recorder = ui.PictureRecorder();
  painter.paint(Canvas(recorder), const Offset(padding, padding));
  painter.dispose();
  final picture = recorder.endRecording();
  final image = await picture.toImage(width, height);
  try {
    final ink = await _inkBounds(image);
    if (ink == null) return await _encodePng(image);
    // Keep a sliver of margin so anti-aliased edges aren't clipped.
    final crop = ink
        .inflate(2)
        .intersect(Rect.fromLTWH(0, 0, width.toDouble(), height.toDouble()));
    final cropRecorder = ui.PictureRecorder();
    Canvas(
      cropRecorder,
    ).drawImageRect(image, crop, Offset.zero & crop.size, Paint());
    final cropPicture = cropRecorder.endRecording();
    final cropped = await cropPicture.toImage(
      crop.width.round(),
      crop.height.round(),
    );
    try {
      return await _encodePng(cropped);
    } finally {
      cropped.dispose();
      cropPicture.dispose();
    }
  } finally {
    image.dispose();
    picture.dispose();
  }
}

/// The smallest rectangle holding every visible pixel of [image], or null
/// when it is fully transparent.
Future<Rect?> _inkBounds(ui.Image image) async {
  final data = await image.toByteData(format: ui.ImageByteFormat.rawRgba);
  if (data == null) return null;
  var (left, top, right, bottom) = (image.width, image.height, -1, -1);
  for (var y = 0; y < image.height; y++) {
    for (var x = 0; x < image.width; x++) {
      if (data.getUint8((y * image.width + x) * 4 + 3) == 0) continue;
      left = min(left, x);
      right = max(right, x);
      top = min(top, y);
      bottom = max(bottom, y);
    }
  }
  if (right < 0) return null;
  return Rect.fromLTRB(
    left.toDouble(),
    top.toDouble(),
    right + 1.0,
    bottom + 1.0,
  );
}

/// Adds the bundled fonts' licences to the app's licence page.
void registerOverlayFontLicenses() {
  LicenseRegistry.addLicense(() async* {
    for (final font in overlayFonts) {
      final family = font.family;
      if (family == null) continue;
      yield LicenseEntryWithLineBreaks([
        family,
      ], await rootBundle.loadString('assets/fonts/$family/OFL.txt'));
    }
  });
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

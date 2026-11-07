/// The Generate tab's centre overlay options: none, a line of text or an
/// image, and how much of the code it covers.
library;

import 'dart:async';
import 'dart:typed_data';

import 'package:flutter/material.dart';
import 'package:logging/logging.dart';

import 'color_widgets.dart';
import 'font_picker.dart';
import 'image_io.dart';
import 'overlay.dart';
import 'src/bindings/bindings.dart';
import 'system_fonts.dart';

final _log = Logger('overlay_editor');

/// What is drawn over the centre of a generated code.
enum OverlayMode { none, text, image }

/// The overlay's settings, owned by the Generate tab and edited by an
/// [OverlayEditor].
class OverlaySettings extends ChangeNotifier {
  OverlayMode mode = OverlayMode.none;
  final TextEditingController text = TextEditingController();

  /// PNG, already scaled down by [imageToOverlayPng].
  Uint8List? image;
  OverlayFont font = platformFont;
  bool bold = true;
  bool italic = false;
  bool outline = false;

  /// Colour of a text overlay; null = the code's primary colour.
  Color? textColor;

  /// Colour of the text's outline; null = the code's secondary colour.
  Color? outlineColor;

  /// Size as a fraction of the largest the error correction level allows, so
  /// changing the level keeps the overlay's relative size.
  double size = 1.0;

  /// Share of the code's area the overlay takes at [eccLevel].
  double share(EccLevel eccLevel) => eccLevel.maxOverlayShare * size;

  OverlayTextStyle textStyle(Color primaryColor, Color secondaryColor) =>
      OverlayTextStyle(
        color: textColor ?? primaryColor.withAlpha(255),
        font: font,
        bold: bold,
        italic: italic,
        outlineColor: outline
            ? outlineColor ?? secondaryColor.withAlpha(255)
            : null,
      );

  /// The overlay to draw over the code as PNG bytes; empty for none.
  Future<Uint8List> png(Color primaryColor, Color secondaryColor) async {
    switch (mode) {
      case OverlayMode.none:
        return Uint8List(0);
      case OverlayMode.text:
        final line = text.text.trim();
        if (line.isEmpty) return Uint8List(0);
        return textToOverlayPng(line, textStyle(primaryColor, secondaryColor));
      case OverlayMode.image:
        return image ?? Uint8List(0);
    }
  }

  /// Applies [change] and tells the editor to rebuild.
  void update(VoidCallback change) {
    change();
    notifyListeners();
  }

  /// Back to no overlay, with every option at its default.
  void reset() => update(() {
    mode = OverlayMode.none;
    text.clear();
    image = null;
    size = 1.0;
    font = platformFont;
    bold = true;
    italic = false;
    outline = false;
    textColor = null;
    outlineColor = null;
  });

  @override
  void dispose() {
    text.dispose();
    super.dispose();
  }
}

/// Edits [settings], calling [onChanged] whenever the code should be
/// regenerated with them.
class OverlayEditor extends StatefulWidget {
  const OverlayEditor({
    super.key,
    required this.settings,
    required this.eccLevel,
    required this.primaryColor,
    required this.secondaryColor,
    required this.onChanged,
    required this.onModeChanged,
  });

  final OverlaySettings settings;
  final EccLevel eccLevel;
  final Color primaryColor;
  final Color secondaryColor;
  final VoidCallback onChanged;

  /// Called instead of [onChanged] when the mode changes, so the owner can
  /// adjust its own settings before regenerating.
  final ValueChanged<OverlayMode> onModeChanged;

  @override
  State<OverlayEditor> createState() => _OverlayEditorState();
}

class _OverlayEditorState extends State<OverlayEditor> {
  Timer? _textDebounce;

  // Installed font families, listed the first time the picker opens.
  Future<List<String>>? _installedFonts;

  OverlaySettings get _settings => widget.settings;

  /// Applies [change] and regenerates the code.
  void _change(VoidCallback change) {
    _settings.update(change);
    widget.onChanged();
  }

  @override
  void dispose() {
    _textDebounce?.cancel();
    super.dispose();
  }

  @override
  Widget build(BuildContext context) {
    return ListenableBuilder(
      listenable: _settings,
      builder: (context, _) => Padding(
        padding: const EdgeInsets.symmetric(horizontal: 16.0),
        child: InputDecorator(
          decoration: const InputDecoration(
            labelText: 'Logo',
            border: OutlineInputBorder(),
          ),
          child: Column(
            children: [
              SegmentedButton<OverlayMode>(
                segments: const [
                  ButtonSegment(value: OverlayMode.none, label: Text('None')),
                  ButtonSegment(value: OverlayMode.text, label: Text('Text')),
                  ButtonSegment(value: OverlayMode.image, label: Text('Image')),
                ],
                selected: {_settings.mode},
                onSelectionChanged: (selection) {
                  _settings.update(() => _settings.mode = selection.first);
                  widget.onModeChanged(selection.first);
                },
              ),
              if (_settings.mode == OverlayMode.text) ...[
                const SizedBox(height: 12),
                _buildTextField(),
                const SizedBox(height: 12),
                _buildTextStyleOptions(context),
              ],
              if (_settings.mode == OverlayMode.image) ...[
                const SizedBox(height: 12),
                _buildImageOptions(),
              ],
              if (_settings.mode != OverlayMode.none) ...[
                const SizedBox(height: 8),
                _buildSizeOptions(),
              ],
            ],
          ),
        ),
      ),
    );
  }

  Widget _buildTextField() {
    return TextField(
      controller: _settings.text,
      minLines: 1,
      maxLines: 3,
      decoration: const InputDecoration(
        labelText: 'Logo text',
        border: OutlineInputBorder(),
      ),
      onChanged: (_) {
        _textDebounce?.cancel();
        _textDebounce = Timer(
          const Duration(milliseconds: 500),
          widget.onChanged,
        );
      },
    );
  }

  /// Font, style and colour of a text overlay.
  Widget _buildTextStyleOptions(BuildContext context) {
    final style = _settings.textStyle(
      widget.primaryColor,
      widget.secondaryColor,
    );
    return Column(
      children: [
        _buildFontDropdown(context),
        const SizedBox(height: 8),
        Wrap(
          alignment: WrapAlignment.center,
          crossAxisAlignment: WrapCrossAlignment.center,
          spacing: 8,
          runSpacing: 8,
          children: [
            FilterChip(
              label: const Text('Bold'),
              selected: _settings.bold,
              onSelected: (on) => _change(() => _settings.bold = on),
            ),
            FilterChip(
              label: const Text('Italic'),
              selected: _settings.italic,
              onSelected: (on) => _change(() => _settings.italic = on),
            ),
            FilterChip(
              label: const Text('Outline'),
              selected: _settings.outline,
              onSelected: (on) => _change(() => _settings.outline = on),
            ),
            const SizedBox(width: 8),
            ColorSwatchButton(
              label: 'Text',
              color: style.color,
              // Transparent text with an outline gives hollow letters.
              showAlpha: true,
              onChanged: (c) => _change(() => _settings.textColor = c),
            ),
            if (_settings.textColor != null)
              TextButton(
                onPressed: () => _change(() => _settings.textColor = null),
                child: const Text('Match primary'),
              ),
            if (style.outlineColor case final outlineColor?) ...[
              ColorSwatchButton(
                label: 'Outline',
                color: outlineColor,
                showAlpha: false,
                onChanged: (c) => _change(() => _settings.outlineColor = c),
              ),
              if (_settings.outlineColor != null)
                TextButton(
                  onPressed: () => _change(() => _settings.outlineColor = null),
                  child: const Text('Match secondary'),
                ),
            ],
          ],
        ),
      ],
    );
  }

  Widget _buildFontDropdown(BuildContext context) {
    final current = _settings.font;
    return InputDecorator(
      decoration: const InputDecoration(
        labelText: 'Font',
        border: OutlineInputBorder(),
        contentPadding: EdgeInsets.symmetric(horizontal: 12, vertical: 4),
      ),
      child: DropdownButtonHideUnderline(
        // null = "Installed fonts…", which opens a picker.
        child: DropdownButton<OverlayFont?>(
          value: current,
          isExpanded: true,
          items: [
            for (final font in [
              ...overlayFonts,
              // An installed font picked earlier.
              if (!overlayFonts.contains(current)) current,
            ])
              DropdownMenuItem(
                value: font,
                // Each name previewed in its own font.
                child: Text(
                  font.name,
                  overflow: TextOverflow.ellipsis,
                  style: OverlayTextStyle(
                    color: Theme.of(context).colorScheme.onSurface,
                    font: font,
                    bold: false,
                  ).toTextStyle(16),
                ),
              ),
            if (canListInstalledFonts)
              const DropdownMenuItem(
                value: null,
                child: Text('Installed fonts…'),
              ),
          ],
          onChanged: (font) async {
            if (font != null) {
              _change(() => _settings.font = font);
              return;
            }
            final family = await showDialog<String>(
              context: context,
              builder: (_) => InstalledFontPicker(
                families: _installedFonts ??= installedFontFamilies(),
              ),
            );
            if (family != null) {
              _change(() => _settings.font = OverlayFont.installed(family));
            }
          },
        ),
      ),
    );
  }

  Widget _buildImageOptions() {
    final image = _settings.image;
    return Row(
      mainAxisAlignment: MainAxisAlignment.center,
      children: [
        if (image != null) ...[
          Container(
            width: 56,
            height: 56,
            decoration: BoxDecoration(
              border: Border.all(color: Colors.grey),
              borderRadius: BorderRadius.circular(4),
            ),
            child: ClipRRect(
              borderRadius: BorderRadius.circular(3),
              child: Stack(
                fit: StackFit.expand,
                children: [
                  CustomPaint(painter: CheckerPainter()),
                  Image.memory(image, fit: BoxFit.contain),
                ],
              ),
            ),
          ),
          IconButton(
            tooltip: 'Remove image',
            icon: const Icon(Icons.close),
            onPressed: () => _change(() => _settings.image = null),
          ),
          const SizedBox(width: 8),
        ],
        OutlinedButton.icon(
          onPressed: () async {
            final bytes = await pickImageFile(const [
              'png',
              'jpg',
              'jpeg',
              'gif',
              'webp',
              'bmp',
            ]);
            if (bytes != null) await _setImage(bytes);
          },
          icon: const Icon(Icons.image),
          label: const Text('Choose…'),
        ),
        const SizedBox(width: 8),
        OutlinedButton.icon(
          onPressed: () async {
            final bytes = await pasteImage(context);
            if (bytes != null) await _setImage(bytes);
          },
          icon: const Icon(Icons.content_paste),
          label: const Text('Paste'),
        ),
      ],
    );
  }

  /// Uses [bytes] (any image format Flutter reads) as the overlay image.
  Future<void> _setImage(Uint8List bytes) async {
    try {
      final png = await imageToOverlayPng(bytes);
      if (!mounted) return;
      _change(() => _settings.image = png);
    } catch (e, stack) {
      _log.warning('Could not read overlay image', e, stack);
      if (mounted) showMessage(context, 'Could not read image: $e');
    }
  }

  Widget _buildSizeOptions() {
    final percent = (_settings.share(widget.eccLevel) * 100).toStringAsFixed(1);
    final maxPercent = widget.eccLevel.maxOverlayShare * 100;
    return Column(
      children: [
        Row(
          children: [
            const Text('Size'),
            Expanded(
              child: Slider(
                value: _settings.size,
                min: 0.2,
                max: 1.0,
                divisions: 8,
                label: '$percent%',
                onChanged: (value) =>
                    _settings.update(() => _settings.size = value),
                onChangeEnd: (_) => widget.onChanged(),
              ),
            ),
            SizedBox(
              width: 56,
              child: Text('$percent%', textAlign: TextAlign.end),
            ),
          ],
        ),
        Text(
          'Up to ${maxPercent.toStringAsFixed(0)}% of the code at this '
          'error correction level; raise it for a larger logo.',
          style: const TextStyle(fontSize: 12, color: Colors.grey),
          textAlign: TextAlign.center,
        ),
      ],
    );
  }
}

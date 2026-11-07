/// The Generate tab: encode text as a code image and tune how it looks.
library;

import 'dart:async';
import 'dart:typed_data';
import 'dart:math';
import 'dart:ui' as ui;

import 'package:flutter/gestures.dart';
import 'package:flutter/material.dart';
import 'package:logging/logging.dart';
import 'package:rinf/rinf.dart';

import 'code_options.dart';
import 'color_presets.dart';
import 'color_widgets.dart';
import 'debug_tables.dart';
import 'image_io.dart';
import 'overlay_editor.dart';
import 'src/bindings/bindings.dart';

final _log = Logger('generate_tab');

const _defaultWidthRatio = 21;
const _defaultHeightRatio = 13;

/// How long typing in a field waits before regenerating.
const _typingDelay = Duration(milliseconds: 500);

/// Screens narrower than this (Material's compact width) are phone-sized.
const _compactWidth = 600.0;

/// Generates codes. Its input is cleared when the user leaves the tab, which
/// [tabController] tells it about; [tabIndex] is its own index there.
class GenerateTab extends StatefulWidget {
  const GenerateTab({
    super.key,
    required this.tabController,
    required this.tabIndex,
    required this.showDebugInfo,
  });

  final TabController tabController;
  final int tabIndex;

  /// Also offers the Plain payload type and a forced mask.
  final bool showDebugInfo;

  @override
  State<GenerateTab> createState() => _GenerateTabState();
}

class _GenerateTabState extends State<GenerateTab>
    with AutomaticKeepAliveClientMixin {
  final _textController = TextEditingController();
  final _textFocusNode = FocusNode();
  final _widthRatioController = TextEditingController(
    text: '$_defaultWidthRatio',
  );
  final _heightRatioController = TextEditingController(
    text: '$_defaultHeightRatio',
  );
  final _seedController = TextEditingController(
    text: (DateTime.now().millisecondsSinceEpoch ~/ 1000).toString(),
  );
  final _triangleWidthController = TextEditingController(
    text: '$defaultTriangleWidth',
  );
  final _overlay = OverlaySettings();
  Timer? _seedDebounce;
  Timer? _triangleWidthDebounce;
  StreamSubscription<RustSignalPack<GeneratedImage>>? _generatedImageSub;

  // Options.
  PayloadType _payloadType = PayloadType.text;
  CodeStyle _style = CodeStyle.tiles;
  Color _primaryColor = Colors.black; // triangles for bit 1
  Color _secondaryColor = Colors.white; // triangles for bit 0
  Color _bgColor = Colors.white;
  // Shapes of each colour shaded slightly apart, by the seed.
  bool _primaryVariance = false;
  bool _secondaryVariance = false;
  EccLevel _eccLevel = defaultEccLevel;
  int _triangleWidth = defaultTriangleWidth;
  bool _customTriangleWidth = false;
  int? _mask; // null = picked by the generator; debug only

  // What is shown.
  bool _showAdvanced = false; // options beyond the text
  bool _showPresets = false; // colour preset chips under the pickers
  final _coloursOffered = <CodeStyle>{}; // see the style selector
  bool _logoLevelOffered = false; // see _onLogoModeChanged

  // The result.
  bool _isGenerating = false;
  String? _generateError;
  _GeneratedCode? _code;
  final _resultKey = GlobalKey(); // marks where the result starts
  bool _revealResult = false; // scroll to the result once it arrives
  int _requestedTriangleWidth = defaultTriangleWidth; // of the pending image

  @override
  bool get wantKeepAlive => true;

  bool get _hasText => _textController.text.trim().isNotEmpty;

  @override
  void initState() {
    super.initState();
    widget.tabController.addListener(_onTabChanged);

    // Hide the preview only when the content changes (not just focus/cursor).
    var lastInput = _input;
    void clearPreviewIfChanged() {
      final input = _input;
      if (input == lastInput) return;
      lastInput = input;
      if (_code != null) {
        setState(() {
          _code = null;
          _generateError = null;
        });
      }
    }

    _textController.addListener(clearPreviewIfChanged);
    _textController.addListener(() => setState(() {}));
    _widthRatioController.addListener(clearPreviewIfChanged);
    _heightRatioController.addListener(clearPreviewIfChanged);

    _generatedImageSub = GeneratedImage.rustSignalStream.listen(
      (signal) => _showGenerated(signal.message),
    );
  }

  /// The inputs whose change makes the shown code stale.
  (String, String, String) get _input => (
    _textController.text,
    _widthRatioController.text,
    _heightRatioController.text,
  );

  @override
  void didUpdateWidget(GenerateTab oldWidget) {
    super.didUpdateWidget(oldWidget);
    if (oldWidget.showDebugInfo && !widget.showDebugInfo) {
      setState(() {
        if (_payloadType == PayloadType.plain) {
          _payloadType = PayloadType.text;
        }
        _mask = null;
      });
    }
  }

  @override
  void dispose() {
    widget.tabController.removeListener(_onTabChanged);
    _generatedImageSub?.cancel();
    _textController.dispose();
    _textFocusNode.dispose();
    _widthRatioController.dispose();
    _heightRatioController.dispose();
    _seedController.dispose();
    _seedDebounce?.cancel();
    _triangleWidthController.dispose();
    _triangleWidthDebounce?.cancel();
    _overlay.dispose();
    super.dispose();
  }

  /// Focuses the text when the user enters this tab and clears the input when
  /// they leave it. The error correction level, seed and payload type stay.
  void _onTabChanged() {
    final controller = widget.tabController;
    if (!controller.indexIsChanging) return;
    if (controller.index == widget.tabIndex) {
      // After the tab animation.
      Future.delayed(const Duration(milliseconds: 100), () {
        if (mounted) _textFocusNode.requestFocus();
      });
    }
    if (controller.previousIndex == widget.tabIndex) _reset();
  }

  void _reset() {
    _textController.clear();
    _widthRatioController.text = '$_defaultWidthRatio';
    _heightRatioController.text = '$_defaultHeightRatio';
    _triangleWidthController.text = '$defaultTriangleWidth';
    _overlay.reset();
    setState(() {
      _code = null;
      _isGenerating = false;
      _generateError = null;
      _style = CodeStyle.tiles;
      _mask = null;
      _triangleWidth = defaultTriangleWidth;
      _customTriangleWidth = false;
      _primaryColor = Colors.black;
      _secondaryColor = Colors.white;
      _bgColor = Colors.white;
      _primaryVariance = false;
      _secondaryVariance = false;
    });
  }

  Future<void> _showGenerated(GeneratedImage message) async {
    if (!mounted) return;
    if (message.error.isNotEmpty) {
      setState(() {
        _code = null;
        _isGenerating = false;
        _generateError = message.error;
      });
      _scrollToResult();
      return;
    }
    final png = Uint8List.fromList(message.pngBytes);
    final (width, height) = await _imageSize(png);
    if (!mounted) return;
    setState(() {
      _code = _GeneratedCode(
        png: png,
        width: width,
        height: height,
        triangleWidth: _requestedTriangleWidth,
        debug: message,
      );
      _isGenerating = false;
      _generateError = null;
    });
    _scrollToResult();
  }

  /// Scrolls the result to the top of the tab, if the Generate button asked
  /// for it: on a phone it would otherwise land under the keyboard.
  void _scrollToResult() {
    if (!_revealResult) return;
    _revealResult = false;
    WidgetsBinding.instance.addPostFrameCallback((_) {
      final context = _resultKey.currentContext;
      if (context == null) return;
      Scrollable.ensureVisible(
        context,
        duration: const Duration(milliseconds: 300),
        curve: Curves.easeOut,
      );
    });
  }

  static Future<(int, int)> _imageSize(Uint8List png) async {
    final codec = await ui.instantiateImageCodec(png);
    final frame = await codec.getNextFrame();
    final size = (frame.image.width, frame.image.height);
    frame.image.dispose();
    codec.dispose();
    return size;
  }

  /// A positive ratio typed into [controller], or [fallback].
  static int _ratio(TextEditingController controller, int fallback) {
    final value = int.tryParse(controller.text) ?? fallback;
    return value > 0 ? value : fallback;
  }

  Future<void> _generate() async {
    setState(() {
      _isGenerating = true;
      _generateError = null;
      _requestedTriangleWidth = _triangleWidth;
    });

    final Uint8List overlayPng;
    try {
      overlayPng = await _overlay.png(_primaryColor, _secondaryColor);
    } catch (e, stack) {
      _log.warning('Overlay rendering failed', e, stack);
      if (!mounted) return;
      setState(() {
        _isGenerating = false;
        _generateError = 'Failed to render the logo: $e';
      });
      _scrollToResult();
      return;
    }

    SmallText(
      text: _textController.text,
      widthRatio: _ratio(_widthRatioController, _defaultWidthRatio),
      heightRatio: _ratio(_heightRatioController, _defaultHeightRatio),
      style: _style,
      payloadType: _payloadType,
      primaryColor: _primaryColor.withAlpha(255).toARGB32(),
      secondaryColor: _secondaryColor.toARGB32(),
      bgColor: _bgColor.toARGB32(),
      seed: Uint64(BigInt.from(int.tryParse(_seedController.text.trim()) ?? 0)),
      primaryVariance: _primaryVariance,
      secondaryVariance: _secondaryVariance,
      mask: widget.showDebugInfo ? _mask : null,
      triangleWidth: _triangleWidth,
      eccLevel: _eccLevel,
      overlayPng: overlayPng,
      overlayShare: _overlay.share(_eccLevel),
    ).sendSignalToRust();
  }

  /// Generates from the button: hides the keyboard and shows the result.
  void _generateAndReveal() {
    FocusScope.of(context).unfocus();
    _revealResult = true;
    _generate();
  }

  /// Regenerates the code with the current settings, if there is text to encode.
  void _regenerate() {
    if (_hasText) _generate();
  }

  /// Applies [change] and regenerates the code.
  void _change(VoidCallback change) {
    setState(change);
    _regenerate();
  }

  /// Applies [change], regenerates the code and scrolls to it once it
  /// arrives, for choices that change the whole code (style, payload type).
  void _changeAndReveal(VoidCallback change) {
    setState(change);
    _regenerateAndReveal();
  }

  /// Regenerates the code and scrolls to it once it arrives, for settings
  /// that change the whole code (triangle size, error correction).
  void _regenerateAndReveal() {
    if (!_hasText) return;
    _revealResult = true;
    _generate();
  }

  /// Regenerates once the user has stopped typing for a moment.
  Timer _regenerateLater(Timer? pending) {
    pending?.cancel();
    return Timer(_typingDelay, _regenerate);
  }

  @override
  Widget build(BuildContext context) {
    super.build(context);
    return SingleChildScrollView(
      // Room to scroll the last controls clear of the system navigation bar
      // and anything else covering the bottom of a phone screen.
      padding: EdgeInsets.only(bottom: scrollEndPadding(context)),
      child: Column(
        children: [
          Padding(padding: const EdgeInsets.all(16.0), child: _buildText()),
          _buildPayloadType(),
          const SizedBox(height: 16),
          _buildStyle(),
          const SizedBox(height: 16),
          _buildColors(),
          if (_showPresets) ...[const SizedBox(height: 12), _buildPresets()],
          const SizedBox(height: 16),
          // Size, error correction and the rest are tucked away, so the tab
          // stays simple unless the user wants to tweak them.
          TextButton.icon(
            onPressed: () => setState(() => _showAdvanced = !_showAdvanced),
            icon: Icon(_showAdvanced ? Icons.expand_less : Icons.expand_more),
            label: const Text('Advanced options'),
          ),
          const SizedBox(height: 8),
          if (_showAdvanced) ..._buildAdvancedOptions(),
          ElevatedButton(
            onPressed: _hasText ? _generateAndReveal : null,
            style: ElevatedButton.styleFrom(
              minimumSize: const Size(240, 56),
              textStyle: Theme.of(context).textTheme.titleMedium,
            ),
            child: const Text('Generate'),
          ),
          SizedBox(key: _resultKey, height: 20),
          ..._buildResult(context),
        ],
      ),
    );
  }

  Widget _buildText() {
    return TextField(
      controller: _textController,
      focusNode: _textFocusNode,
      keyboardType: TextInputType.multiline,
      textInputAction: TextInputAction.newline,
      minLines: 3,
      maxLines: null,
      decoration: const InputDecoration(
        labelText: 'Enter text',
        border: OutlineInputBorder(),
      ),
    );
  }

  Widget _buildPayloadType() {
    return SegmentedButton<PayloadType>(
      segments: [
        const ButtonSegment(value: PayloadType.text, label: Text('Text')),
        const ButtonSegment(value: PayloadType.url, label: Text('URL')),
        if (widget.showDebugInfo)
          const ButtonSegment(value: PayloadType.plain, label: Text('Plain')),
      ],
      selected: {_payloadType},
      onSelectionChanged: (selection) =>
          _changeAndReveal(() => _payloadType = selection.first),
    );
  }

  Widget _buildStyle() {
    return SegmentedButton<CodeStyle>(
      segments: [
        for (final style in CodeStyle.values)
          ButtonSegment(value: style, label: Text(style.label)),
      ],
      selected: {_style},
      onSelectionChanged: (selection) {
        String? suggestion;
        _changeAndReveal(() {
          _style = selection.first;
          // The first switch to Horizon or Stain suggests the Zero Dawn colours,
          // with colour variance on for Stain and off for Horizon, but only over
          // the untouched black and white default.
          final suggests =
              _style == CodeStyle.horizon || _style == CodeStyle.stain;
          if (suggests &&
              _coloursOffered.add(_style) &&
              colorPresets.first.matches(
                _primaryColor,
                _secondaryColor,
                _bgColor,
              )) {
            _applyPreset(zeroDawnPreset);
            if (_style == CodeStyle.stain) {
              _primaryVariance = true;
              _secondaryVariance = true;
              suggestion =
                  '${_style.label} looks best with the ${zeroDawnPreset.name} '
                  'colors and color variance, so both are now on';
            } else {
              final varied = _primaryVariance || _secondaryVariance;
              _primaryVariance = false;
              _secondaryVariance = false;
              suggestion = varied
                  ? '${_style.label} looks best with the '
                        '${zeroDawnPreset.name} colors and no color variance, '
                        'so the colors are now on and variance is off'
                  : '${_style.label} looks best with the '
                        '${zeroDawnPreset.name} colors, so they are now on';
            }
          }
        });
        if (suggestion != null) showMessage(context, suggestion!);
      },
    );
  }

  void _applyPreset(ColorPreset preset) {
    _primaryColor = preset.primary;
    _secondaryColor = preset.secondary;
    _bgColor = preset.background;
  }

  Widget _buildColors() {
    return Wrap(
      alignment: WrapAlignment.center,
      crossAxisAlignment: WrapCrossAlignment.center,
      spacing: 20,
      runSpacing: 8,
      children: [
        ColorSwatchButton(
          label: 'Primary',
          color: _primaryColor,
          showAlpha: false,
          onChanged: (c) => _change(() => _primaryColor = c),
        ),
        ColorSwatchButton(
          label: 'Secondary',
          color: _secondaryColor,
          showAlpha: true,
          onChanged: (c) => _change(() => _secondaryColor = c),
        ),
        ColorSwatchButton(
          label: 'Background',
          color: _bgColor,
          showAlpha: true,
          onChanged: (c) => _change(() => _bgColor = c),
        ),
        TextButton.icon(
          onPressed: () => setState(() => _showPresets = !_showPresets),
          icon: Icon(_showPresets ? Icons.expand_less : Icons.palette),
          label: const Text('Swatches'),
        ),
      ],
    );
  }

  Widget _buildPresets() {
    return Wrap(
      alignment: WrapAlignment.center,
      spacing: 8,
      runSpacing: 8,
      children: [
        for (final preset in colorPresets)
          ChoiceChip(
            avatar: PresetPreview(preset: preset),
            label: Text(preset.name),
            selected: preset.matches(_primaryColor, _secondaryColor, _bgColor),
            onSelected: (_) => _change(() => _applyPreset(preset)),
          ),
      ],
    );
  }

  /// The first logo raises the error correction to high, which repairs the
  /// most covered triangles and so allows the largest logo.
  void _onLogoModeChanged(OverlayMode mode) {
    String? message;
    if (mode != OverlayMode.none && !_logoLevelOffered) {
      _logoLevelOffered = true;
      if (_eccLevel != EccLevel.high) {
        setState(() => _eccLevel = EccLevel.high);
        message =
            'Error correction raised to high, so the code still reads with '
            'a logo over it';
      }
    }
    _regenerate();
    if (message != null) showMessage(context, message);
  }

  List<Widget> _buildAdvancedOptions() {
    return [
      _secondaryRow(_buildAspectRatio()),
      const SizedBox(height: 16),
      // How much of the code may be damaged (scratched, covered by a logo)
      // and still decode. Higher levels enlarge the grid.
      _secondaryRow(_buildEccLevel()),
      const SizedBox(height: 16),
      _secondaryRow(_buildTriangleSize()),
      const SizedBox(height: 16),
      _secondaryRow(_buildColorVariance()),
      const SizedBox(height: 16),
      _secondaryRow(_buildSeed()),
      const SizedBox(height: 16),
      OverlayEditor(
        settings: _overlay,
        eccLevel: _eccLevel,
        primaryColor: _primaryColor,
        secondaryColor: _secondaryColor,
        onChanged: _regenerate,
        onModeChanged: _onLogoModeChanged,
      ),
      const SizedBox(height: 16),
      if (widget.showDebugInfo) ...[
        Padding(
          padding: const EdgeInsets.symmetric(horizontal: 16.0),
          child: _buildMask(),
        ),
        const SizedBox(height: 16),
      ],
    ];
  }

  /// An option that rarely changes: half the width and centred, so it doesn't
  /// take a full row like the main inputs, except on a phone-sized screen,
  /// where half the width is too narrow for its labels.
  Widget _secondaryRow(Widget child) {
    if (MediaQuery.sizeOf(context).width < _compactWidth) {
      return Padding(
        padding: const EdgeInsets.symmetric(horizontal: 16.0),
        child: child,
      );
    }
    return FractionallySizedBox(widthFactor: 0.5, child: child);
  }

  Widget _buildAspectRatio() {
    Widget ratioField(
      TextEditingController controller,
      String label,
      int hint,
    ) {
      return Expanded(
        child: TextField(
          controller: controller,
          keyboardType: TextInputType.number,
          decoration: InputDecoration(
            labelText: label,
            border: const OutlineInputBorder(),
            hintText: '$hint',
          ),
        ),
      );
    }

    return Row(
      children: [
        ratioField(_widthRatioController, 'Width Ratio', _defaultWidthRatio),
        const Padding(
          padding: EdgeInsets.symmetric(horizontal: 8.0),
          child: Text(
            ':',
            style: TextStyle(fontSize: 24, fontWeight: FontWeight.bold),
          ),
        ),
        ratioField(_heightRatioController, 'Height Ratio', _defaultHeightRatio),
      ],
    );
  }

  Widget _buildEccLevel() {
    return _Dropdown<EccLevel>(
      label: 'Error correction',
      value: _eccLevel,
      items: [for (final level in EccLevel.values) (level, level.label)],
      onChanged: (level) {
        if (level == null) return;
        setState(() => _eccLevel = level);
        _regenerateAndReveal();
      },
    );
  }

  Widget _buildTriangleSize() {
    return Row(
      crossAxisAlignment: CrossAxisAlignment.start,
      children: [
        Expanded(
          // null = custom width typed into the field beside it
          child: _Dropdown<int?>(
            label: 'Triangle size',
            value: _customTriangleWidth ? null : _triangleWidth,
            items: [
              for (final (width, name) in triangleWidthPresets)
                (width, '$name — $width px'),
              (null, 'Custom…'),
            ],
            onChanged: (width) {
              setState(() {
                _customTriangleWidth = width == null;
                if (width != null) _triangleWidth = width;
                _triangleWidthController.text = '$_triangleWidth';
              });
              if (width != null) _regenerateAndReveal();
            },
          ),
        ),
        if (_customTriangleWidth) ...[
          const SizedBox(width: 8),
          SizedBox(
            width: 120,
            child: TextField(
              controller: _triangleWidthController,
              keyboardType: TextInputType.number,
              decoration: InputDecoration(
                labelText: 'Width (px)',
                helperText: 'Odd, $minTriangleWidth–$maxTriangleWidth',
                errorText: triangleWidthError(_triangleWidthController.text),
                border: const OutlineInputBorder(),
              ),
              onChanged: (text) {
                setState(() {});
                if (triangleWidthError(text) != null) return;
                _triangleWidth = int.parse(text.trim());
                _triangleWidthDebounce = _regenerateLater(
                  _triangleWidthDebounce,
                );
              },
            ),
          ),
        ],
      ],
    );
  }

  Widget _buildMask() {
    return _Dropdown<int?>(
      label: 'Mask (debug)',
      value: _mask,
      items: [
        (null, 'Auto (most color changes)'),
        for (final (index, name) in maskNames.indexed) (index, name),
      ],
      onChanged: (mask) => _change(() => _mask = mask),
    );
  }

  /// Shifts each shape's brightness a little, picked by the seed, separately
  /// for either colour.
  Widget _buildColorVariance() {
    return InputDecorator(
      decoration: const InputDecoration(
        labelText: 'Color variance',
        helperText: 'Shades each shape slightly lighter',
        border: OutlineInputBorder(),
        contentPadding: EdgeInsets.symmetric(horizontal: 12, vertical: 8),
      ),
      child: Wrap(
        spacing: 8,
        runSpacing: 8,
        children: [
          FilterChip(
            label: const Text('Primary'),
            selected: _primaryVariance,
            onSelected: (on) => _change(() => _primaryVariance = on),
          ),
          FilterChip(
            label: const Text('Secondary'),
            selected: _secondaryVariance,
            onSelected: (on) => _change(() => _secondaryVariance = on),
          ),
        ],
      ),
    );
  }

  /// Always shown so the options don't jump around; only the Horizon style
  /// and colour variance use a seed, so it's disabled otherwise.
  Widget _buildSeed() {
    final enabled =
        _style == CodeStyle.horizon || _primaryVariance || _secondaryVariance;
    return Row(
      children: [
        Expanded(
          child: TextField(
            controller: _seedController,
            enabled: enabled,
            keyboardType: TextInputType.number,
            decoration: InputDecoration(
              labelText: 'Seed',
              hintText: 'Leave empty for current time',
              helperText: enabled
                  ? null
                  : 'Used by the Horizon style and color variance',
              border: const OutlineInputBorder(),
            ),
            onChanged: (_) => _seedDebounce = _regenerateLater(_seedDebounce),
          ),
        ),
        const SizedBox(width: 8),
        IconButton(
          tooltip: 'Randomize seed',
          icon: const Icon(Icons.shuffle),
          onPressed: enabled
              ? () => _change(() => _seedController.text = '${_randomSeed()}')
              : null,
        ),
      ],
    );
  }

  /// A random 10-digit seed. `nextInt` only goes up to 2^32, so it is built
  /// from a leading 1–9 and 9 more digits.
  static int _randomSeed() {
    final rng = Random();
    return (1 + rng.nextInt(9)) * 1000000000 + rng.nextInt(1000000000);
  }

  List<Widget> _buildResult(BuildContext context) {
    final errorStyle = TextStyle(color: Theme.of(context).colorScheme.error);
    final code = _code;
    if (_isGenerating) {
      return const [
        SizedBox(width: 50, height: 50, child: CircularProgressIndicator()),
        SizedBox(height: 8),
        Text('Generating image...'),
        SizedBox(height: 20),
      ];
    }
    if (_generateError case final error?) {
      return [
        Padding(
          padding: const EdgeInsets.symmetric(horizontal: 16.0),
          child: Text(error, textAlign: TextAlign.center, style: errorStyle),
        ),
        const SizedBox(height: 20),
      ];
    }
    if (code == null) return const [];
    final warning = code.debug.decodeWarning;
    return [
      const Divider(height: 40, thickness: 1),
      Row(
        mainAxisAlignment: MainAxisAlignment.center,
        children: [
          ElevatedButton.icon(
            onPressed: () => savePng(context, code.png, 'generated_image'),
            icon: const Icon(Icons.download),
            label: const Text('Download Image'),
          ),
          const SizedBox(width: 12),
          ElevatedButton.icon(
            onPressed: () => copyPng(context, code.png),
            icon: const Icon(Icons.copy),
            label: const Text('Copy to Clipboard'),
          ),
        ],
      ),
      if (warning.isNotEmpty) ...[
        const SizedBox(height: 12),
        Padding(
          padding: const EdgeInsets.symmetric(horizontal: 16.0),
          child: Text(warning, textAlign: TextAlign.center, style: errorStyle),
        ),
      ],
      const SizedBox(height: 16),
      // A new code starts at the default zoom.
      _CodePreview(key: ObjectKey(code), code: code),
      if (widget.showDebugInfo) ...[
        const SizedBox(height: 24),
        const Divider(),
        const SizedBox(height: 8),
        const Padding(
          padding: EdgeInsets.symmetric(horizontal: 16.0),
          child: DebugHeading(),
        ),
        const SizedBox(height: 8),
        Padding(
          padding: const EdgeInsets.symmetric(horizontal: 16.0),
          child: GenerateDebugTable(code.debug),
        ),
      ],
      const SizedBox(height: 20),
    ];
  }
}

/// A generated code and what it was generated with.
class _GeneratedCode {
  const _GeneratedCode({
    required this.png,
    required this.width,
    required this.height,
    required this.triangleWidth,
    required this.debug,
  });

  final Uint8List png;
  final int width;
  final int height;
  final int triangleWidth;
  final GeneratedImage debug;
}

/// A dropdown in an outlined box with a label, as `(value, label)` items.
class _Dropdown<T> extends StatelessWidget {
  const _Dropdown({
    super.key,
    required this.label,
    required this.value,
    required this.items,
    required this.onChanged,
  });

  final String label;
  final T value;
  final List<(T, String)> items;
  final ValueChanged<T?> onChanged;

  @override
  Widget build(BuildContext context) {
    return InputDecorator(
      decoration: InputDecoration(
        labelText: label,
        border: const OutlineInputBorder(),
        contentPadding: const EdgeInsets.symmetric(horizontal: 12, vertical: 4),
      ),
      child: DropdownButtonHideUnderline(
        child: DropdownButton<T>(
          value: value,
          isExpanded: true,
          items: [
            for (final (value, label) in items)
              DropdownMenuItem(value: value, child: Text(label)),
          ],
          onChanged: onChanged,
        ),
      ),
    );
  }
}

/// The generated code over a checkerboard (showing a transparent background),
/// with zoom and pan.
class _CodePreview extends StatefulWidget {
  const _CodePreview({super.key, required this.code});

  final _GeneratedCode code;

  @override
  State<_CodePreview> createState() => _CodePreviewState();
}

class _CodePreviewState extends State<_CodePreview> {
  static const _maxHeight = 400.0;
  static const _minZoom = 0.5;
  static const _maxZoom = 5.0;

  final _transformationController = TransformationController();
  double _zoomLevel = 1.0;

  @override
  void dispose() {
    _transformationController.dispose();
    super.dispose();
  }

  void _zoom(double level) =>
      setState(() => _zoomLevel = level.clamp(_minZoom, _maxZoom));

  @override
  Widget build(BuildContext context) {
    final code = widget.code;
    final availableWidth = MediaQuery.of(context).size.width - 32;
    // Zoom controls only help when the image is bigger than its box.
    final showZoomControls =
        code.width > availableWidth || code.height > _maxHeight;
    return Column(
      children: [
        Listener(
          onPointerSignal: (event) {
            // Registering here (as an ancestor) means this fires before the
            // parent SingleChildScrollView's listener, so we can claim the
            // event and prevent page scrolling. InteractiveViewer registers its
            // own claim in the same frame and, being a descendant, wins the
            // resolver race — so zooming still works normally.
            if (event is PointerScrollEvent) {
              GestureBinding.instance.pointerSignalResolver.register(
                event,
                (_) {},
              );
            }
          },
          child: Container(
            constraints: BoxConstraints(
              minWidth: 300,
              minHeight: 200,
              maxWidth: max(300.0, availableWidth),
              maxHeight: _maxHeight,
            ),
            decoration: BoxDecoration(
              border: Border.all(color: Colors.grey),
              borderRadius: BorderRadius.circular(8),
            ),
            child: ClipRRect(
              borderRadius: BorderRadius.circular(7),
              child: Stack(
                children: [
                  Positioned.fill(
                    child: CustomPaint(painter: CheckerPainter()),
                  ),
                  InteractiveViewer(
                    transformationController: _transformationController,
                    boundaryMargin: const EdgeInsets.all(20),
                    minScale: _minZoom,
                    maxScale: _maxZoom,
                    clipBehavior: Clip.hardEdge,
                    child: Transform.scale(
                      scale: _zoomLevel,
                      child: Image.memory(code.png, fit: BoxFit.contain),
                    ),
                  ),
                ],
              ),
            ),
          ),
        ),
        const SizedBox(height: 8),
        Text(
          '${code.width} × ${code.height} px'
          ' · triangles ${code.triangleWidth} px wide',
          style: const TextStyle(fontSize: 12, color: Colors.grey),
        ),
        if (showZoomControls) ...[
          const SizedBox(height: 8),
          Row(
            mainAxisAlignment: MainAxisAlignment.center,
            children: [
              IconButton(
                icon: const Icon(Icons.zoom_out),
                onPressed: () => _zoom(_zoomLevel - 0.2),
              ),
              SizedBox(
                width: 150,
                child: Slider(
                  value: _zoomLevel,
                  min: _minZoom,
                  max: _maxZoom,
                  divisions: 45,
                  label: '${(_zoomLevel * 100).toStringAsFixed(0)}%',
                  onChanged: _zoom,
                ),
              ),
              IconButton(
                icon: const Icon(Icons.zoom_in),
                onPressed: () => _zoom(_zoomLevel + 0.2),
              ),
              IconButton(
                icon: const Icon(Icons.refresh),
                onPressed: () => _zoom(1.0),
              ),
            ],
          ),
        ],
      ],
    );
  }
}

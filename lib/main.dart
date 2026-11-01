import 'dart:async';
import 'dart:io';
import 'dart:math';
import 'dart:ui' as ui;

import 'package:flutter/foundation.dart';

import 'package:flutter/gestures.dart';
import 'package:flutter/material.dart';
import 'package:flutter_colorpicker/flutter_colorpicker.dart';
import 'package:rinf/rinf.dart';
import 'package:file_selector/file_selector.dart';
import 'package:path_provider/path_provider.dart';
import 'package:url_launcher/url_launcher.dart';
import 'package:super_clipboard/super_clipboard.dart';

import 'overlay.dart';
import 'system_fonts.dart';
import 'src/bindings/bindings.dart';

/// Data masks the generator can XOR the payload with, in the same order as
/// `MASK_NAMES` / `MASKS` in native/tricode/src/lib.rs (the index is sent to Rust).
const _maskNames = [
  'Checkerboard',
  '2×2 blocks',
  'Right wave',
  'Left wave',
  'Vertical lines',
  'Horizontal lines',
  'None',
];

/// Triangle base width limits in pixels, matching `MIN_TRIANGLE_WIDTH`,
/// `DEFAULT_TRIANGLE_WIDTH` and `MAX_TRIANGLE_WIDTH` in native/tricode/src/lib.rs.
const _minTriangleWidth = 37;
const _defaultTriangleWidth = 37;
const _maxTriangleWidth = 149;

/// Preset triangle widths offered in the size dropdown.
/// Error-correction levels, indexed as `SmallText.eccLevel` expects, with how
/// much of the code each survives being damaged.
const _eccLevelNames = [
  'Low (~7% damage)',
  'Medium (~15% damage)',
  'Quartile (~25% damage)',
  'High (~30% damage)',
];

const _triangleWidthPresets = [
  (_defaultTriangleWidth, 'Standard'),
  (75, 'High (×2)'),
  (_maxTriangleWidth, 'Print (×4)'),
];

/// Label of the action that reveals a saved file, in each system's own words.
String get _revealLabel {
  if (Platform.isMacOS) return 'Show in Finder';
  if (Platform.isWindows) return 'Show in Explorer';
  return 'Show in folder';
}

/// Opens the system file manager with [path] selected, or at least its folder
/// open. Returns false when nothing could be opened.
Future<bool> _revealInFileManager(String path) async {
  final file = File(path).absolute;
  try {
    if (Platform.isMacOS) {
      final result = await Process.run('open', ['-R', file.path]);
      if (result.exitCode == 0) return true;
    } else if (Platform.isWindows) {
      // Explorer exits with 1 even when it worked, so the exit code is ignored.
      await Process.run('explorer.exe', ['/select,', file.path]);
      return true;
    } else if (Platform.isLinux) {
      // Nautilus, Dolphin, Nemo, Caja and others implement this freedesktop
      // interface, which also selects the file; xdg-open below only opens the
      // folder.
      final result = await Process.run('dbus-send', [
        '--session',
        '--print-reply',
        '--dest=org.freedesktop.FileManager1',
        '--type=method_call',
        '/org/freedesktop/FileManager1',
        'org.freedesktop.FileManager1.ShowItems',
        // dbus-send splits array items on commas, so escape them in the URI.
        'array:string:${file.uri.toString().replaceAll(',', '%2C')}',
        'string:',
      ]);
      if (result.exitCode == 0) return true;
    }
  } on ProcessException {
    // The tool isn't installed or can't be run (e.g. sandboxed); fall back.
  }
  return launchUrl(Uri.directory(file.parent.path));
}

/// Why [text] is not an allowed triangle width, or null when it is.
String? _triangleWidthError(String text) {
  final width = int.tryParse(text.trim());
  if (width == null) return 'Enter a number';
  if (width < _minTriangleWidth || width > _maxTriangleWidth) {
    return 'Must be $_minTriangleWidth–$_maxTriangleWidth px';
  }
  if (width.isEven) return 'Must be odd';
  return null;
}

Future<void> main() async {
  await initializeRust(assignRustSignal);
  registerOverlayFontLicenses();
  runApp(const MyApp());
}

/// The app's brand colour; both colour schemes are generated from it.
const _primaryColor = Color(0xFF90add3);

/// Light, dark or follow the system; picked in the settings dialog.
final themeMode = ValueNotifier(ThemeMode.system);

/// A colour scheme generated from [_primaryColor], keeping it as the exact primary colour.
ColorScheme _colorScheme(Brightness brightness) => ColorScheme.fromSeed(
  seedColor: _primaryColor,
  brightness: brightness,
  dynamicSchemeVariant: DynamicSchemeVariant.fidelity,
).copyWith(primary: _primaryColor, onPrimary: Colors.white);

ThemeData _theme(Brightness brightness) {
  final colorScheme = _colorScheme(brightness);
  return ThemeData(
    colorScheme: colorScheme,
    appBarTheme: AppBarTheme(
      backgroundColor: colorScheme.primary,
      foregroundColor: colorScheme.onPrimary,
    ),
    tabBarTheme: TabBarThemeData(
      labelColor: colorScheme.onPrimary,
      unselectedLabelColor: colorScheme.onPrimary.withValues(alpha: 0.7),
      indicatorColor: colorScheme.onPrimary,
    ),
  );
}

class MyApp extends StatelessWidget {
  const MyApp({super.key});

  @override
  Widget build(BuildContext context) {
    return ValueListenableBuilder(
      valueListenable: themeMode,
      builder: (context, mode, _) => MaterialApp(
        title: 'Tricode',
        theme: _theme(Brightness.light),
        darkTheme: _theme(Brightness.dark),
        themeMode: mode,
        home: const MyHomePage(),
      ),
    );
  }
}

class MyHomePage extends StatefulWidget {
  const MyHomePage({super.key});

  @override
  State<MyHomePage> createState() => _MyHomePageState();
}

class _MyHomePageState extends State<MyHomePage>
    with SingleTickerProviderStateMixin {
  late final TabController _tabController;
  final TextEditingController _textController = TextEditingController();
  final FocusNode _textFocusNode = FocusNode();
  final TextEditingController _widthRatioController = TextEditingController(
    text: '21',
  );
  final TextEditingController _heightRatioController = TextEditingController(
    text: '13',
  );
  final TextEditingController _horizonSeedController = TextEditingController(
    text: (DateTime.now().millisecondsSinceEpoch ~/ 1000).toString(),
  );
  Timer? _seedDebounce;
  final TextEditingController _triangleWidthController = TextEditingController(
    text: '$_defaultTriangleWidth',
  );
  Timer? _triangleWidthDebounce;
  int _triangleWidth = _defaultTriangleWidth;
  bool _customTriangleWidth = false;
  int _requestedTriangleWidth = _defaultTriangleWidth; // of the pending image
  int _imageTriangleWidth = _defaultTriangleWidth; // of the shown image
  String? _generateError;
  StreamSubscription<RustSignalPack<GeneratedImage>>? _generatedImageSub;
  StreamSubscription<RustSignalPack<DecodeResult>>? _decodeResultSub;
  Uint8List? _latestImage;
  double _zoomLevel = 1.0;
  int? _imageWidth;
  int? _imageHeight;
  bool _isGenerating = false;
  bool _isDecoding = false;
  int _selectedStyle = 0; // 0 = Tiles, 1 = Stain
  int _eccLevel = 1; // index into _eccLevelNames; 1 = Medium
  bool _showAdvanced = false; // Write tab options beyond the text
  bool _showPresets = false; // colour preset chips under the pickers
  bool _horizonColoursOffered = false; // see the style selector
  bool? _decodeSuccess;
  String? _decodeText;
  String _decodeContentType = '';
  DecodeResult? _decodeDebug;
  GeneratedImage? _generateDebug;
  bool _showDebugInfo = false;
  String _selectedPayloadType = 'text';
  int? _selectedMask; // null = most balanced (automatic), debug only
  Color _fgColor = Colors.black;
  Color _bgColor = Colors.white;
  _OverlayMode _overlayMode = _OverlayMode.none;
  final TextEditingController _overlayTextController = TextEditingController();
  Timer? _overlayTextDebounce;
  Uint8List? _overlayImage; // PNG, already scaled down by imageToOverlayPng
  OverlayFont _overlayFont = platformFont;
  // Installed font families, listed the first time the picker opens.
  Future<List<String>>? _installedFonts;
  bool _overlayBold = true;
  bool _overlayItalic = false;
  bool _overlayOutline = false;
  Color? _overlayTextColor; // null = the triangle colour

  OverlayTextStyle get _overlayTextStyle => OverlayTextStyle(
    color: (_overlayTextColor ?? _fgColor).withAlpha(255),
    font: _overlayFont,
    bold: _overlayBold,
    italic: _overlayItalic,
    outline: _overlayOutline,
  );
  // Overlay size as a fraction of the largest the error correction level allows,
  // so changing the level keeps the overlay's relative size.
  double _overlaySize = 1.0;

  /// Share of the code's area the overlay takes at the current settings.
  double get _overlayShare => overlayMaxShares[_eccLevel] * _overlaySize;

  bool get _shouldShowZoomControls {
    if (_imageWidth == null || _imageHeight == null) return false;
    final containerWidth = MediaQuery.of(context).size.width - 32;
    final containerHeight = 400.0;
    return _imageWidth! > containerWidth || _imageHeight! > containerHeight;
  }

  @override
  void initState() {
    super.initState();

    _tabController = TabController(length: 2, vsync: this);
    _tabController.addListener(() {
      if (!_tabController.indexIsChanging) return;
      // When switching to the Generate tab (index 1), focus the textarea
      if (_tabController.index == 1) {
        // Use a short delay to ensure the tab animation is complete
        Future.delayed(const Duration(milliseconds: 100), () {
          if (mounted) {
            _textFocusNode.requestFocus();
          }
        });
      }
      // When leaving the Read tab (index 0), reset its state
      if (_tabController.previousIndex == 0) {
        setState(() {
          _decodeSuccess = null;
          _decodeText = null;
          _decodeContentType = '';
          _decodeDebug = null;
          _isDecoding = false;
        });
      }
      // When leaving the Generate tab (index 1), reset its state
      if (_tabController.previousIndex == 1) {
        _textController.clear();
        _widthRatioController.text = '21';
        _heightRatioController.text = '13';
        setState(() {
          _latestImage = null;
          _imageWidth = null;
          _imageHeight = null;
          _isGenerating = false;
          _generateDebug = null;
          _selectedStyle = 0;
          _selectedMask = null;
          _triangleWidth = _defaultTriangleWidth;
          _customTriangleWidth = false;
          _triangleWidthController.text = '$_defaultTriangleWidth';
          _generateError = null;
          _fgColor = Colors.black;
          _bgColor = Colors.white;
          _overlayMode = _OverlayMode.none;
          _overlayTextController.clear();
          _overlayImage = null;
          _overlaySize = 1.0;
          _overlayFont = platformFont;
          _overlayBold = true;
          _overlayItalic = false;
          _overlayOutline = false;
          _overlayTextColor = null;
        });
      }
    });

    // Hide the preview only when the actual text content changes (not just focus/cursor)
    String lastText = _textController.text;
    String lastWidth = _widthRatioController.text;
    String lastHeight = _heightRatioController.text;

    void clearPreviewIfChanged() {
      final textChanged = _textController.text != lastText;
      final widthChanged = _widthRatioController.text != lastWidth;
      final heightChanged = _heightRatioController.text != lastHeight;
      lastText = _textController.text;
      lastWidth = _widthRatioController.text;
      lastHeight = _heightRatioController.text;
      if ((textChanged || widthChanged || heightChanged) &&
          _latestImage != null) {
        setState(() {
          _latestImage = null;
          _imageWidth = null;
          _imageHeight = null;
          _generateDebug = null;
          _generateError = null;
        });
      }
    }

    _textController.addListener(clearPreviewIfChanged);
    _textController.addListener(() => setState(() {}));
    _widthRatioController.addListener(clearPreviewIfChanged);
    _heightRatioController.addListener(clearPreviewIfChanged);

    _generatedImageSub = GeneratedImage.rustSignalStream.listen((signal) async {
      if (!mounted) return;
      if (signal.message.error.isNotEmpty) {
        setState(() {
          _latestImage = null;
          _imageWidth = null;
          _imageHeight = null;
          _isGenerating = false;
          _generateDebug = null;
          _generateError = signal.message.error;
        });
        return;
      }
      final imageBytes = Uint8List.fromList(signal.message.pngBytes);

      // Decode image to get dimensions
      final codec = await ui.instantiateImageCodec(imageBytes);
      final frame = await codec.getNextFrame();
      final image = frame.image;

      if (!mounted) return;
      setState(() {
        _latestImage = imageBytes;
        _imageWidth = image.width;
        _imageHeight = image.height;
        _zoomLevel = 1.0; // Reset zoom when new image is generated
        _isGenerating = false; // Stop loading indicator
        _generateDebug = signal.message;
        _generateError = null;
        _imageTriangleWidth = _requestedTriangleWidth;
      });
    });

    _decodeResultSub = DecodeResult.rustSignalStream.listen((signal) {
      if (!mounted) return;
      final msg = signal.message;
      setState(() {
        _isDecoding = false;
        _decodeSuccess = msg.success;
        _decodeText = msg.success ? msg.text : null;
        _decodeContentType = msg.success ? msg.contentType : '';
        _decodeDebug = msg;
      });
    });
  }

  /// The overlay to draw over the code as PNG bytes; empty for none.
  Future<Uint8List> _overlayPng() async {
    switch (_overlayMode) {
      case _OverlayMode.none:
        return Uint8List(0);
      case _OverlayMode.text:
        final text = _overlayTextController.text.trim();
        if (text.isEmpty) return Uint8List(0);
        return textToOverlayPng(text, _overlayTextStyle);
      case _OverlayMode.image:
        return _overlayImage ?? Uint8List(0);
    }
  }

  Future<void> _sendTextToRust() async {
    final text = _textController.text;
    final widthRatio = int.tryParse(_widthRatioController.text) ?? 21;
    final heightRatio = int.tryParse(_heightRatioController.text) ?? 13;

    // Ensure positive non-zero values
    final safeWidth = widthRatio > 0 ? widthRatio : 21;
    final safeHeight = heightRatio > 0 ? heightRatio : 13;

    setState(() {
      _isGenerating = true; // Start loading indicator
      _generateError = null;
      _requestedTriangleWidth = _triangleWidth;
    });

    final effectiveBg = _bgColor;
    final Uint8List overlayPng;
    try {
      overlayPng = await _overlayPng();
    } catch (e) {
      if (!mounted) return;
      setState(() {
        _isGenerating = false;
        _generateError = 'Failed to render the overlay: $e';
      });
      return;
    }

    SmallText(
      text: text,
      widthRatio: safeWidth,
      heightRatio: safeHeight,
      style: _selectedStyle,
      payloadType: _selectedPayloadType,
      fgColor: _fgColor.withAlpha(255).toARGB32(),
      bgColor: effectiveBg.toARGB32(),
      horizonSeed: Uint64(
        BigInt.from(int.tryParse(_horizonSeedController.text.trim()) ?? 0),
      ),
      mask: _showDebugInfo ? _selectedMask : null,
      triangleWidth: _triangleWidth,
      eccLevel: _eccLevel,
      overlayPng: overlayPng,
      overlayShare: _overlayShare,
    ).sendSignalToRust();
  }

  /// Regenerates the code with the current settings, if there is text to encode.
  void _regenerate() {
    if (_textController.text.trim().isNotEmpty) {
      _sendTextToRust();
    }
  }

  /// Uses [bytes] (any image format Flutter reads) as the overlay image.
  Future<void> _setOverlayImage(Uint8List bytes) async {
    try {
      final png = await imageToOverlayPng(bytes);
      if (!mounted) return;
      setState(() => _overlayImage = png);
      _regenerate();
    } catch (e) {
      if (mounted) {
        ScaffoldMessenger.of(
          context,
        ).showSnackBar(SnackBar(content: Text('Could not read image: $e')));
      }
    }
  }

  Future<void> _pickOverlayImage() async {
    const typeGroup = XTypeGroup(
      label: 'images',
      extensions: ['png', 'jpg', 'jpeg', 'gif', 'webp', 'bmp'],
    );
    final file = await openFile(acceptedTypeGroups: [typeGroup]);
    if (file == null) return;
    await _setOverlayImage(await file.readAsBytes());
  }

  Future<void> _pasteOverlayImage() async {
    final Uint8List? bytes;
    try {
      bytes = await readClipboardImage();
    } on ClipboardUnavailable {
      if (mounted) {
        ScaffoldMessenger.of(context).showSnackBar(
          const SnackBar(
            content: Text('Clipboard not available on this platform'),
          ),
        );
      }
      return;
    }
    if (bytes == null) {
      if (mounted) {
        ScaffoldMessenger.of(context).showSnackBar(
          const SnackBar(content: Text('No image found in clipboard')),
        );
      }
      return;
    }
    await _setOverlayImage(bytes);
  }

  Future<void> _pickAndDecodeImage() async {
    try {
      const XTypeGroup typeGroup = XTypeGroup(
        label: 'images',
        extensions: ['png', 'jpg', 'jpeg'],
      );

      final XFile? file = await openFile(acceptedTypeGroups: [typeGroup]);

      if (file == null) {
        // User canceled the dialog
        return;
      }

      final bytes = await file.readAsBytes();

      setState(() {
        _isDecoding = true;
        _decodeSuccess = null;
        _decodeText = null;
      });

      ImageBytes(bytes: bytes.toList()).sendSignalToRust();
    } catch (e) {
      if (mounted) {
        ScaffoldMessenger.of(
          context,
        ).showSnackBar(SnackBar(content: Text('Error reading file: $e')));
        setState(() {
          _isDecoding = false;
        });
      }
    }
  }

  Future<void> _pasteAndDecodeImage() async {
    try {
      final Uint8List? imageBytes;
      try {
        imageBytes = await readClipboardImage();
      } on ClipboardUnavailable {
        if (mounted) {
          ScaffoldMessenger.of(context).showSnackBar(
            const SnackBar(
              content: Text('Clipboard not available on this platform'),
            ),
          );
        }
        return;
      }

      if (imageBytes == null) {
        if (mounted) {
          ScaffoldMessenger.of(context).showSnackBar(
            const SnackBar(content: Text('No image found in clipboard')),
          );
        }
        return;
      }

      setState(() {
        _isDecoding = true;
        _decodeSuccess = null;
        _decodeText = null;
      });

      ImageBytes(bytes: imageBytes).sendSignalToRust();
    } catch (e) {
      if (mounted) {
        ScaffoldMessenger.of(context).showSnackBar(
          SnackBar(content: Text('Error pasting from clipboard: $e')),
        );
        setState(() {
          _isDecoding = false;
        });
      }
    }
  }

  Future<void> _downloadImage() async {
    if (_latestImage == null) return;

    try {
      // Generate a default filename with timestamp
      final timestamp = DateTime.now()
          .toIso8601String()
          .replaceAll(':', '-')
          .split('.')[0];
      final defaultName = 'generated_image_$timestamp.png';

      if (Platform.isAndroid || Platform.isIOS) {
        // For mobile platforms, save to downloads directory
        final directory = Platform.isAndroid
            ? await getExternalStorageDirectory()
            : await getApplicationDocumentsDirectory();

        if (directory == null) {
          if (mounted) {
            ScaffoldMessenger.of(context).showSnackBar(
              const SnackBar(
                content: Text('Could not access storage directory'),
              ),
            );
          }
          return;
        }

        final filePath = '${directory.path}/$defaultName';
        final file = File(filePath);
        await file.writeAsBytes(_latestImage!);

        if (mounted) {
          ScaffoldMessenger.of(
            context,
          ).showSnackBar(SnackBar(content: Text('Image saved to: $filePath')));
        }
      } else {
        // For desktop platforms, use file picker
        final filePath = await getSaveLocation(
          suggestedName: defaultName,
          acceptedTypeGroups: [
            const XTypeGroup(label: 'PNG Images', extensions: ['png']),
          ],
        );

        if (filePath == null) {
          // User canceled the dialog
          return;
        }

        final file = XFile.fromData(
          _latestImage!,
          mimeType: 'image/png',
          name: defaultName,
        );

        await file.saveTo(filePath.path);

        if (mounted) _showSavedSnackBar(filePath.path);
      }
    } catch (e) {
      if (mounted) {
        ScaffoldMessenger.of(
          context,
        ).showSnackBar(SnackBar(content: Text('Error saving image: $e')));
      }
    }
  }

  /// Tells the user where a file was saved, with an action that reveals it in
  /// the system file manager.
  void _showSavedSnackBar(String path) {
    ScaffoldMessenger.of(context).showSnackBar(
      SnackBar(
        content: Text('Image saved to: $path'),
        duration: const Duration(seconds: 8),
        action: SnackBarAction(
          label: _revealLabel,
          onPressed: () async {
            if (await _revealInFileManager(path) || !mounted) return;
            ScaffoldMessenger.of(context).showSnackBar(
              const SnackBar(content: Text('Could not open the file manager')),
            );
          },
        ),
      ),
    );
  }

  Future<void> _copyImageToClipboard() async {
    if (_latestImage == null) return;

    try {
      final clipboard = SystemClipboard.instance;
      if (clipboard == null) {
        if (mounted) {
          ScaffoldMessenger.of(context).showSnackBar(
            const SnackBar(
              content: Text('Clipboard not available on this platform'),
            ),
          );
        }
        return;
      }

      final item = DataWriterItem();
      item.add(Formats.png(_latestImage!));

      await clipboard.write([item]);

      if (mounted) {
        ScaffoldMessenger.of(context).showSnackBar(
          const SnackBar(content: Text('Image copied to clipboard')),
        );
      }
    } catch (e) {
      if (mounted) {
        ScaffoldMessenger.of(context).showSnackBar(
          SnackBar(content: Text('Error copying to clipboard: $e')),
        );
      }
    }
  }

  Future<void> _downloadAnnotatedImage(DecodeResult debug) async {
    if (debug.annotatedImage.isEmpty) return;

    try {
      final timestamp = DateTime.now()
          .toIso8601String()
          .replaceAll(':', '-')
          .split('.')[0];
      final defaultName = 'annotated_image_$timestamp.png';

      if (Platform.isAndroid || Platform.isIOS) {
        final directory = Platform.isAndroid
            ? await getExternalStorageDirectory()
            : await getApplicationDocumentsDirectory();

        if (directory == null) {
          if (mounted) {
            ScaffoldMessenger.of(context).showSnackBar(
              const SnackBar(
                content: Text('Could not access storage directory'),
              ),
            );
          }
          return;
        }

        final filePath = '${directory.path}/$defaultName';
        final file = File(filePath);
        await file.writeAsBytes(debug.annotatedImage);

        if (mounted) {
          ScaffoldMessenger.of(
            context,
          ).showSnackBar(SnackBar(content: Text('Image saved to: $filePath')));
        }
      } else {
        final filePath = await getSaveLocation(
          suggestedName: defaultName,
          acceptedTypeGroups: [
            const XTypeGroup(label: 'PNG Images', extensions: ['png']),
          ],
        );

        if (filePath == null) {
          return;
        }

        final file = XFile.fromData(
          Uint8List.fromList(debug.annotatedImage),
          mimeType: 'image/png',
          name: defaultName,
        );

        await file.saveTo(filePath.path);

        if (mounted) _showSavedSnackBar(filePath.path);
      }
    } catch (e) {
      if (mounted) {
        ScaffoldMessenger.of(
          context,
        ).showSnackBar(SnackBar(content: Text('Error saving image: $e')));
      }
    }
  }

  Future<void> _copyAnnotatedImageToClipboard(DecodeResult debug) async {
    if (debug.annotatedImage.isEmpty) return;

    try {
      final clipboard = SystemClipboard.instance;
      if (clipboard == null) {
        if (mounted) {
          ScaffoldMessenger.of(context).showSnackBar(
            const SnackBar(
              content: Text('Clipboard not available on this platform'),
            ),
          );
        }
        return;
      }

      final item = DataWriterItem();
      item.add(Formats.png(Uint8List.fromList(debug.annotatedImage)));

      await clipboard.write([item]);

      if (mounted) {
        ScaffoldMessenger.of(context).showSnackBar(
          const SnackBar(content: Text('Image copied to clipboard')),
        );
      }
    } catch (e) {
      if (mounted) {
        ScaffoldMessenger.of(context).showSnackBar(
          SnackBar(content: Text('Error copying to clipboard: $e')),
        );
      }
    }
  }

  @override
  Widget build(BuildContext context) {
    return DefaultTabController(
      length: 2,
      child: Scaffold(
        appBar: AppBar(
          title: const Text('Tricode'),
          actions: [
            Padding(
              padding: EdgeInsets.only(right: kDebugMode ? 48.0 : 0.0),
              child: IconButton(
                icon: const Icon(Icons.settings),
                tooltip: 'Settings',
                onPressed: () {
                  showDialog<void>(
                    context: context,
                    builder: (context) => StatefulBuilder(
                      builder: (context, setDialogState) => AlertDialog(
                        title: const Text('Settings'),
                        content: Column(
                          mainAxisSize: MainAxisSize.min,
                          crossAxisAlignment: CrossAxisAlignment.start,
                          children: [
                            CheckboxListTile(
                              title: const Text('Show debug info'),
                              value: _showDebugInfo,
                              onChanged: (value) {
                                setDialogState(() {});
                                final enabled = value ?? false;
                                setState(() {
                                  _showDebugInfo = enabled;
                                  if (!enabled &&
                                      _selectedPayloadType == 'plain') {
                                    _selectedPayloadType = 'text';
                                  }
                                  if (!enabled) {
                                    _selectedMask = null;
                                  }
                                });
                              },
                            ),
                            const SizedBox(height: 16),
                            const Text('Theme'),
                            const SizedBox(height: 8),
                            SegmentedButton<ThemeMode>(
                              segments: const [
                                ButtonSegment(
                                  value: ThemeMode.system,
                                  icon: Icon(Icons.brightness_auto),
                                  label: Text('System'),
                                ),
                                ButtonSegment(
                                  value: ThemeMode.light,
                                  icon: Icon(Icons.light_mode),
                                  label: Text('Light'),
                                ),
                                ButtonSegment(
                                  value: ThemeMode.dark,
                                  icon: Icon(Icons.dark_mode),
                                  label: Text('Dark'),
                                ),
                              ],
                              selected: {themeMode.value},
                              onSelectionChanged: (selection) {
                                setDialogState(() {});
                                themeMode.value = selection.first;
                              },
                            ),
                          ],
                        ),
                        actions: [
                          TextButton(
                            onPressed: () => Navigator.of(context).pop(),
                            child: const Text('Close'),
                          ),
                        ],
                      ),
                    ),
                  );
                },
              ),
            ),
          ],
          bottom: TabBar(
            controller: _tabController,
            tabs: const [
              Tab(text: 'Read'),
              Tab(text: 'Generate'),
            ],
          ),
        ),
        body: TabBarView(
          controller: _tabController,
          children: [
            Center(
              child: SingleChildScrollView(
                child: Padding(
                  padding: const EdgeInsets.all(24.0),
                  child: Column(
                    mainAxisAlignment: MainAxisAlignment.center,
                    children: [
                      const Text(
                        'Select an image to decode',
                        style: TextStyle(
                          fontSize: 18,
                          fontWeight: FontWeight.bold,
                        ),
                      ),
                      const SizedBox(height: 20),
                      Row(
                        mainAxisAlignment: MainAxisAlignment.center,
                        children: [
                          ElevatedButton.icon(
                            onPressed: _pickAndDecodeImage,
                            icon: const Icon(Icons.file_upload),
                            label: const Text('Select Image File'),
                          ),
                          const SizedBox(width: 16),
                          ElevatedButton.icon(
                            onPressed: _pasteAndDecodeImage,
                            icon: const Icon(Icons.content_paste),
                            label: const Text('Paste from Clipboard'),
                          ),
                        ],
                      ),
                      const SizedBox(height: 20),
                      if (_isDecoding) ...[
                        const SizedBox(
                          width: 50,
                          height: 50,
                          child: CircularProgressIndicator(),
                        ),
                        const SizedBox(height: 8),
                        const Text('Decoding image...'),
                      ] else if (_decodeSuccess == true) ...[
                        const Text(
                          'Decoded successfully:',
                          style: TextStyle(
                            fontSize: 16,
                            fontWeight: FontWeight.bold,
                            color: Colors.green,
                          ),
                        ),
                        const SizedBox(height: 8),
                        _ContentTypeBadge(_decodeContentType),
                        const SizedBox(height: 6),
                        if (_decodeContentType == 'url') ...[
                          GestureDetector(
                            onTap: () async {
                              final uri = Uri.tryParse(_decodeText!);
                              if (uri != null && await canLaunchUrl(uri)) {
                                await launchUrl(
                                  uri,
                                  mode: LaunchMode.externalApplication,
                                );
                              }
                            },
                            child: Container(
                              width: double.infinity,
                              padding: const EdgeInsets.all(16.0),
                              decoration: BoxDecoration(
                                border: Border.all(color: Colors.green),
                                borderRadius: BorderRadius.circular(8),
                              ),
                              child: Text(
                                _decodeText!,
                                style: const TextStyle(
                                  fontSize: 14,
                                  color: Colors.blue,
                                  decoration: TextDecoration.underline,
                                ),
                              ),
                            ),
                          ),
                        ] else ...[
                          Container(
                            width: double.infinity,
                            padding: const EdgeInsets.all(16.0),
                            decoration: BoxDecoration(
                              border: Border.all(
                                color: _decodeContentType == 'plain'
                                    ? Colors.orange
                                    : Colors.green,
                              ),
                              borderRadius: BorderRadius.circular(8),
                            ),
                            child: SelectableText(
                              _decodeText!,
                              style: const TextStyle(fontSize: 14),
                            ),
                          ),
                        ],
                      ] else if (_decodeSuccess == false) ...[
                        const Text(
                          'Decoding failed',
                          style: TextStyle(
                            fontSize: 16,
                            fontWeight: FontWeight.bold,
                            color: Colors.red,
                          ),
                        ),
                      ],
                      if (_showDebugInfo && _decodeDebug != null) ...[
                        const SizedBox(height: 24),
                        const Divider(),
                        const SizedBox(height: 8),
                        const Align(
                          alignment: Alignment.centerLeft,
                          child: Text(
                            'Debug info',
                            style: TextStyle(
                              fontSize: 13,
                              fontWeight: FontWeight.bold,
                              color: Colors.grey,
                            ),
                          ),
                        ),
                        const SizedBox(height: 8),
                        _DebugTable(_decodeDebug!),
                        if (_decodeDebug!.annotatedImage.isNotEmpty) ...[
                          const SizedBox(height: 12),
                          const Align(
                            alignment: Alignment.centerLeft,
                            child: Text(
                              'Annotated image:',
                              style: TextStyle(
                                fontSize: 13,
                                color: Colors.grey,
                              ),
                            ),
                          ),
                          const SizedBox(height: 8),
                          Row(
                            mainAxisAlignment: MainAxisAlignment.center,
                            children: [
                              ElevatedButton.icon(
                                onPressed: () =>
                                    _downloadAnnotatedImage(_decodeDebug!),
                                icon: const Icon(Icons.download),
                                label: const Text('Download'),
                              ),
                              const SizedBox(width: 12),
                              ElevatedButton.icon(
                                onPressed: () => _copyAnnotatedImageToClipboard(
                                  _decodeDebug!,
                                ),
                                icon: const Icon(Icons.copy),
                                label: const Text('Copy'),
                              ),
                            ],
                          ),
                          const SizedBox(height: 8),
                          Container(
                            constraints: const BoxConstraints(
                              maxWidth: 400,
                              maxHeight: 400,
                            ),
                            decoration: BoxDecoration(
                              border: Border.all(color: Colors.grey),
                              borderRadius: BorderRadius.circular(8),
                            ),
                            child: ClipRRect(
                              borderRadius: BorderRadius.circular(7),
                              child: Image.memory(
                                Uint8List.fromList(
                                  _decodeDebug!.annotatedImage,
                                ),
                                fit: BoxFit.contain,
                              ),
                            ),
                          ),
                        ],
                      ],
                    ],
                  ),
                ),
              ),
            ),
            SingleChildScrollView(
              child: Column(
                mainAxisAlignment: MainAxisAlignment.start,
                children: [
                  Padding(
                    padding: const EdgeInsets.all(16.0),
                    child: TextField(
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
                    ),
                  ),
                  SegmentedButton<String>(
                    segments: [
                      const ButtonSegment(value: 'text', label: Text('Text')),
                      const ButtonSegment(value: 'url', label: Text('URL')),
                      if (_showDebugInfo)
                        const ButtonSegment(
                          value: 'plain',
                          label: Text('Plain'),
                        ),
                    ],
                    selected: {_selectedPayloadType},
                    onSelectionChanged: (selection) {
                      setState(() {
                        _selectedPayloadType = selection.first;
                      });
                      if (_textController.text.trim().isNotEmpty) {
                        _sendTextToRust();
                      } else {
                        setState(() {
                          _latestImage = null;
                          _imageWidth = null;
                          _imageHeight = null;
                          _generateDebug = null;
                        });
                      }
                    },
                  ),
                  const SizedBox(height: 16),
                  SegmentedButton<int>(
                    segments: const [
                      ButtonSegment(value: 0, label: Text('Tiles')),
                      ButtonSegment(value: 1, label: Text('Stain')),
                      ButtonSegment(value: 2, label: Text('Horizon')),
                    ],
                    selected: {_selectedStyle},
                    onSelectionChanged: (selection) {
                      setState(() {
                        _selectedStyle = selection.first;
                        // The first switch to Horizon suggests its own colours,
                        // but only over the untouched black and white default.
                        if (_selectedStyle == 2 && !_horizonColoursOffered) {
                          _horizonColoursOffered = true;
                          if (colorPresets.first.matches(_fgColor, _bgColor)) {
                            _fgColor = zeroDawnPreset.foreground;
                            _bgColor = zeroDawnPreset.background;
                          }
                        }
                      });
                      if (_textController.text.trim().isNotEmpty) {
                        _sendTextToRust();
                      }
                    },
                  ),
                  const SizedBox(height: 16),
                  // Color pickers
                  Row(
                    mainAxisAlignment: MainAxisAlignment.center,
                    children: [
                      _ColorSwatch(
                        label: 'Triangles',
                        color: _fgColor,
                        showAlpha: false,
                        onChanged: (c) {
                          setState(() => _fgColor = c);
                          if (_textController.text.trim().isNotEmpty) {
                            _sendTextToRust();
                          }
                        },
                      ),
                      const SizedBox(width: 24),
                      _ColorSwatch(
                        label: 'Background',
                        color: _bgColor,
                        showAlpha: true,
                        onChanged: (c) {
                          setState(() => _bgColor = c);
                          if (_textController.text.trim().isNotEmpty) {
                            _sendTextToRust();
                          }
                        },
                      ),
                      const SizedBox(width: 16),
                      TextButton.icon(
                        onPressed: () =>
                            setState(() => _showPresets = !_showPresets),
                        icon: Icon(
                          _showPresets ? Icons.expand_less : Icons.palette,
                        ),
                        label: const Text('Swatches'),
                      ),
                    ],
                  ),
                  if (_showPresets) ...[
                    const SizedBox(height: 12),
                    Wrap(
                      alignment: WrapAlignment.center,
                      spacing: 8,
                      runSpacing: 8,
                      children: [
                        for (final preset in colorPresets)
                          ChoiceChip(
                            avatar: _PresetPreview(preset: preset),
                            label: Text(preset.name),
                            selected: preset.matches(_fgColor, _bgColor),
                            onSelected: (_) {
                              setState(() {
                                _fgColor = preset.foreground;
                                _bgColor = preset.background;
                              });
                              if (_textController.text.trim().isNotEmpty) {
                                _sendTextToRust();
                              }
                            },
                          ),
                      ],
                    ),
                  ],
                  const SizedBox(height: 16),
                  // Size, error correction and the rest are tucked away, so
                  // the tab stays simple unless the user wants to tweak them.
                  TextButton.icon(
                    onPressed: () =>
                        setState(() => _showAdvanced = !_showAdvanced),
                    icon: Icon(
                      _showAdvanced ? Icons.expand_less : Icons.expand_more,
                    ),
                    label: const Text('Advanced options'),
                  ),
                  const SizedBox(height: 8),
                  if (_showAdvanced) ...[
                    Padding(
                      padding: const EdgeInsets.symmetric(horizontal: 16.0),
                      child: Row(
                        children: [
                          Expanded(
                            child: TextField(
                              controller: _widthRatioController,
                              keyboardType: TextInputType.number,
                              decoration: const InputDecoration(
                                labelText: 'Width Ratio',
                                border: OutlineInputBorder(),
                                hintText: '21',
                              ),
                            ),
                          ),
                          const Padding(
                            padding: EdgeInsets.symmetric(horizontal: 8.0),
                            child: Text(
                              ':',
                              style: TextStyle(
                                fontSize: 24,
                                fontWeight: FontWeight.bold,
                              ),
                            ),
                          ),
                          Expanded(
                            child: TextField(
                              controller: _heightRatioController,
                              keyboardType: TextInputType.number,
                              decoration: const InputDecoration(
                                labelText: 'Height Ratio',
                                border: OutlineInputBorder(),
                                hintText: '13',
                              ),
                            ),
                          ),
                        ],
                      ),
                    ),
                    const SizedBox(height: 16),
                    // How much of the code may be damaged (scratched, covered by
                    // a logo) and still decode. Higher levels enlarge the grid.
                    // Half the width, like the triangle size below.
                    FractionallySizedBox(
                      widthFactor: 0.5,
                      child: InputDecorator(
                        decoration: const InputDecoration(
                          labelText: 'Error correction',
                          border: OutlineInputBorder(),
                          contentPadding: EdgeInsets.symmetric(
                            horizontal: 12,
                            vertical: 4,
                          ),
                        ),
                        child: DropdownButtonHideUnderline(
                          child: DropdownButton<int>(
                            value: _eccLevel,
                            isExpanded: true,
                            items: [
                              for (final (index, name)
                                  in _eccLevelNames.indexed)
                                DropdownMenuItem(
                                  value: index,
                                  child: Text(name),
                                ),
                            ],
                            onChanged: (level) {
                              if (level == null) return;
                              setState(() {
                                _eccLevel = level;
                              });
                              if (_textController.text.trim().isNotEmpty) {
                                _sendTextToRust();
                              }
                            },
                          ),
                        ),
                      ),
                    ),
                    const SizedBox(height: 16),
                    // Half the width, centred: the size rarely changes, so it
                    // shouldn't take a full row like the main inputs.
                    FractionallySizedBox(
                      widthFactor: 0.5,
                      child: Row(
                        crossAxisAlignment: CrossAxisAlignment.start,
                        children: [
                          Expanded(
                            child: InputDecorator(
                              decoration: const InputDecoration(
                                labelText: 'Triangle size',
                                border: OutlineInputBorder(),
                                contentPadding: EdgeInsets.symmetric(
                                  horizontal: 12,
                                  vertical: 4,
                                ),
                              ),
                              child: DropdownButtonHideUnderline(
                                // null = custom width typed into the field beside it
                                child: DropdownButton<int?>(
                                  value: _customTriangleWidth
                                      ? null
                                      : _triangleWidth,
                                  isExpanded: true,
                                  items: [
                                    for (final (width, name)
                                        in _triangleWidthPresets)
                                      DropdownMenuItem(
                                        value: width,
                                        child: Text('$name — $width px'),
                                      ),
                                    const DropdownMenuItem(
                                      value: null,
                                      child: Text('Custom…'),
                                    ),
                                  ],
                                  onChanged: (width) {
                                    setState(() {
                                      _customTriangleWidth = width == null;
                                      if (width != null) {
                                        _triangleWidth = width;
                                      }
                                      _triangleWidthController.text =
                                          '$_triangleWidth';
                                    });
                                    if (width != null &&
                                        _textController.text
                                            .trim()
                                            .isNotEmpty) {
                                      _sendTextToRust();
                                    }
                                  },
                                ),
                              ),
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
                                  helperText:
                                      'Odd, $_minTriangleWidth–$_maxTriangleWidth',
                                  errorText: _triangleWidthError(
                                    _triangleWidthController.text,
                                  ),
                                  border: const OutlineInputBorder(),
                                ),
                                onChanged: (text) {
                                  setState(() {});
                                  if (_triangleWidthError(text) != null) return;
                                  _triangleWidth = int.parse(text.trim());
                                  _triangleWidthDebounce?.cancel();
                                  _triangleWidthDebounce = Timer(
                                    const Duration(milliseconds: 500),
                                    () {
                                      if (_textController.text
                                          .trim()
                                          .isNotEmpty) {
                                        _sendTextToRust();
                                      }
                                    },
                                  );
                                },
                              ),
                            ),
                          ],
                        ],
                      ),
                    ),
                    const SizedBox(height: 16),
                    _buildOverlayOptions(context),
                    const SizedBox(height: 16),
                    if (_showDebugInfo) ...[
                      Padding(
                        padding: const EdgeInsets.symmetric(horizontal: 16.0),
                        child: InputDecorator(
                          decoration: const InputDecoration(
                            labelText: 'Mask (debug)',
                            border: OutlineInputBorder(),
                            contentPadding: EdgeInsets.symmetric(
                              horizontal: 12,
                              vertical: 4,
                            ),
                          ),
                          child: DropdownButtonHideUnderline(
                            child: DropdownButton<int?>(
                              value: _selectedMask,
                              isExpanded: true,
                              items: [
                                const DropdownMenuItem(
                                  value: null,
                                  child: Text('Auto (most balanced)'),
                                ),
                                for (final (index, name) in _maskNames.indexed)
                                  DropdownMenuItem(
                                    value: index,
                                    child: Text(name),
                                  ),
                              ],
                              onChanged: (mask) {
                                setState(() {
                                  _selectedMask = mask;
                                });
                                if (_textController.text.trim().isNotEmpty) {
                                  _sendTextToRust();
                                }
                              },
                            ),
                          ),
                        ),
                      ),
                      const SizedBox(height: 16),
                    ],
                    // Always shown so the options don't jump around; only the
                    // Horizon style uses a seed, so it's disabled for the others.
                    Padding(
                      padding: const EdgeInsets.symmetric(horizontal: 16.0),
                      child: Row(
                        children: [
                          Expanded(
                            child: TextField(
                              controller: _horizonSeedController,
                              enabled: _selectedStyle == 2,
                              keyboardType: TextInputType.number,
                              decoration: InputDecoration(
                                labelText: 'Horizon seed',
                                hintText: 'Leave empty for current time',
                                helperText: _selectedStyle == 2
                                    ? null
                                    : 'Only used by the Horizon style',
                                border: const OutlineInputBorder(),
                              ),
                              onChanged: (_) {
                                _seedDebounce?.cancel();
                                _seedDebounce = Timer(
                                  const Duration(milliseconds: 500),
                                  () {
                                    if (_textController.text
                                        .trim()
                                        .isNotEmpty) {
                                      _sendTextToRust();
                                    }
                                  },
                                );
                              },
                            ),
                          ),
                          const SizedBox(width: 8),
                          IconButton(
                            tooltip: 'Randomize seed',
                            icon: const Icon(Icons.shuffle),
                            onPressed: _selectedStyle != 2
                                ? null
                                : () {
                                    final rng = Random();
                                    // nextInt max is 2^32, so build 10 digits from two calls.
                                    // High part: 1–9, low part: 0–999999999 (9 digits zero-padded).
                                    final high = 1 + rng.nextInt(9);
                                    final low = rng.nextInt(1000000000);
                                    final seed = high * 1000000000 + low;
                                    setState(() {
                                      _horizonSeedController.text = seed
                                          .toString();
                                    });
                                    if (_textController.text
                                        .trim()
                                        .isNotEmpty) {
                                      _sendTextToRust();
                                    }
                                  },
                          ),
                        ],
                      ),
                    ),
                    const SizedBox(height: 16),
                  ],
                  ElevatedButton(
                    onPressed: _textController.text.trim().isEmpty
                        ? null
                        : _sendTextToRust,
                    child: const Text('Generate'),
                  ),
                  const SizedBox(height: 20),
                  if (_isGenerating) ...[
                    const SizedBox(
                      width: 50,
                      height: 50,
                      child: CircularProgressIndicator(),
                    ),
                    const SizedBox(height: 8),
                    const Text('Generating image...'),
                    const SizedBox(height: 20),
                  ] else if (_generateError != null) ...[
                    Padding(
                      padding: const EdgeInsets.symmetric(horizontal: 16.0),
                      child: Text(
                        _generateError!,
                        textAlign: TextAlign.center,
                        style: TextStyle(
                          color: Theme.of(context).colorScheme.error,
                        ),
                      ),
                    ),
                    const SizedBox(height: 20),
                  ] else if (_latestImage != null) ...[
                    const Divider(height: 40, thickness: 1),
                    Row(
                      mainAxisAlignment: MainAxisAlignment.center,
                      children: [
                        ElevatedButton.icon(
                          onPressed: _downloadImage,
                          icon: const Icon(Icons.download),
                          label: const Text('Download Image'),
                        ),
                        const SizedBox(width: 12),
                        ElevatedButton.icon(
                          onPressed: _copyImageToClipboard,
                          icon: const Icon(Icons.copy),
                          label: const Text('Copy to Clipboard'),
                        ),
                      ],
                    ),
                    if (_generateDebug?.decodeWarning case final warning?
                        when warning.isNotEmpty) ...[
                      const SizedBox(height: 12),
                      Padding(
                        padding: const EdgeInsets.symmetric(horizontal: 16.0),
                        child: Text(
                          warning,
                          textAlign: TextAlign.center,
                          style: TextStyle(
                            color: Theme.of(context).colorScheme.error,
                          ),
                        ),
                      ),
                    ],
                    const SizedBox(height: 16),
                    // Image preview with zoom and pan
                    Listener(
                      onPointerSignal: (event) {
                        // Registering here (as an ancestor) means this fires
                        // before the parent SingleChildScrollView's listener,
                        // so we can claim the event and prevent page scrolling.
                        // InteractiveViewer registers its own claim in the same
                        // frame and, being a descendant, wins the resolver race
                        // — so zooming still works normally.
                        if (event is PointerScrollEvent) {
                          GestureBinding.instance.pointerSignalResolver
                              .register(event, (_) {});
                        }
                      },
                      child: Container(
                        constraints: BoxConstraints(
                          minWidth: 300,
                          minHeight: 200,
                          maxWidth: max(
                            300.0,
                            MediaQuery.of(context).size.width - 32,
                          ),
                          maxHeight: max(200.0, 400),
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
                                child: CustomPaint(painter: _CheckerPainter()),
                              ),
                              InteractiveViewer(
                                transformationController:
                                    TransformationController(),
                                boundaryMargin: const EdgeInsets.all(20),
                                minScale: 0.5,
                                maxScale: 5.0,
                                clipBehavior: Clip.hardEdge,
                                child: Transform.scale(
                                  scale: _zoomLevel,
                                  child: Image.memory(
                                    _latestImage!,
                                    fit: BoxFit.contain,
                                  ),
                                ),
                              ),
                            ],
                          ),
                        ),
                      ),
                    ),
                    if (_imageWidth != null && _imageHeight != null) ...[
                      const SizedBox(height: 8),
                      Text(
                        '$_imageWidth × $_imageHeight px'
                        ' · triangles $_imageTriangleWidth px wide',
                        style: const TextStyle(
                          fontSize: 12,
                          color: Colors.grey,
                        ),
                      ),
                    ],
                    // Zoom controls - only show if image is bigger than container
                    if (_shouldShowZoomControls) ...[
                      const SizedBox(height: 8),
                      Row(
                        mainAxisAlignment: MainAxisAlignment.center,
                        children: [
                          IconButton(
                            icon: const Icon(Icons.zoom_out),
                            onPressed: () => setState(() {
                              _zoomLevel = (_zoomLevel - 0.2).clamp(0.5, 5.0);
                            }),
                          ),
                          SizedBox(
                            width: 150,
                            child: Slider(
                              value: _zoomLevel,
                              min: 0.5,
                              max: 5.0,
                              divisions: 45,
                              label:
                                  '${(_zoomLevel * 100).toStringAsFixed(0)}%',
                              onChanged: (value) => setState(() {
                                _zoomLevel = value;
                              }),
                            ),
                          ),
                          IconButton(
                            icon: const Icon(Icons.zoom_in),
                            onPressed: () => setState(() {
                              _zoomLevel = (_zoomLevel + 0.2).clamp(0.5, 5.0);
                            }),
                          ),
                          IconButton(
                            icon: const Icon(Icons.refresh),
                            onPressed: () => setState(() {
                              _zoomLevel = 1.0;
                            }),
                          ),
                        ],
                      ),
                    ],
                    if (_showDebugInfo && _generateDebug != null) ...[
                      const SizedBox(height: 24),
                      const Divider(),
                      const SizedBox(height: 8),
                      Padding(
                        padding: const EdgeInsets.symmetric(horizontal: 16.0),
                        child: const Align(
                          alignment: Alignment.centerLeft,
                          child: Text(
                            'Debug info',
                            style: TextStyle(
                              fontSize: 13,
                              fontWeight: FontWeight.bold,
                              color: Colors.grey,
                            ),
                          ),
                        ),
                      ),
                      const SizedBox(height: 8),
                      Padding(
                        padding: const EdgeInsets.symmetric(horizontal: 16.0),
                        child: _GenerateDebugTable(_generateDebug!),
                      ),
                    ],
                    const SizedBox(height: 20),
                  ],
                ],
              ),
            ),
          ],
        ),
      ),
    );
  }

  @override
  void dispose() {
    _tabController.dispose();
    _generatedImageSub?.cancel();
    _decodeResultSub?.cancel();
    _textController.dispose();
    _textFocusNode.dispose();
    _widthRatioController.dispose();
    _heightRatioController.dispose();
    _horizonSeedController.dispose();
    _seedDebounce?.cancel();
    _triangleWidthController.dispose();
    _triangleWidthDebounce?.cancel();
    _overlayTextController.dispose();
    _overlayTextDebounce?.cancel();
    super.dispose();
  }

  /// Font, style and colour of a text overlay.
  Widget _buildOverlayTextStyleOptions(BuildContext context) {
    void update(VoidCallback change) {
      setState(change);
      _regenerate();
    }

    return Column(
      children: [
        InputDecorator(
          decoration: const InputDecoration(
            labelText: 'Font',
            border: OutlineInputBorder(),
            contentPadding: EdgeInsets.symmetric(horizontal: 12, vertical: 4),
          ),
          child: DropdownButtonHideUnderline(
            // null = "Installed fonts…", which opens a picker.
            child: DropdownButton<OverlayFont?>(
              value: _overlayFont,
              isExpanded: true,
              items: [
                for (final font in [
                  ...overlayFonts,
                  // An installed font picked earlier.
                  if (!overlayFonts.contains(_overlayFont)) _overlayFont,
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
                  update(() => _overlayFont = font);
                  return;
                }
                final family = await showDialog<String>(
                  context: context,
                  builder: (_) => _InstalledFontPicker(
                    families: _installedFonts ??= installedFontFamilies(),
                  ),
                );
                if (family != null) {
                  update(() => _overlayFont = OverlayFont.installed(family));
                }
              },
            ),
          ),
        ),
        const SizedBox(height: 8),
        Wrap(
          alignment: WrapAlignment.center,
          crossAxisAlignment: WrapCrossAlignment.center,
          spacing: 8,
          runSpacing: 8,
          children: [
            FilterChip(
              label: const Text('Bold'),
              selected: _overlayBold,
              onSelected: (on) => update(() => _overlayBold = on),
            ),
            FilterChip(
              label: const Text('Italic'),
              selected: _overlayItalic,
              onSelected: (on) => update(() => _overlayItalic = on),
            ),
            FilterChip(
              label: const Text('Outline'),
              selected: _overlayOutline,
              onSelected: (on) => update(() => _overlayOutline = on),
            ),
            const SizedBox(width: 8),
            _ColorSwatch(
              label: 'Text',
              color: _overlayTextStyle.color,
              showAlpha: false,
              onChanged: (c) => update(() => _overlayTextColor = c),
            ),
            if (_overlayTextColor != null)
              TextButton(
                onPressed: () => update(() => _overlayTextColor = null),
                child: const Text('Match triangles'),
              ),
          ],
        ),
      ],
    );
  }

  /// The overlay drawn over the centre of the code: none, a line of text or an
  /// image, and how much of the code it covers.
  Widget _buildOverlayOptions(BuildContext context) {
    final maxPercent = overlayMaxShares[_eccLevel] * 100;
    return Padding(
      padding: const EdgeInsets.symmetric(horizontal: 16.0),
      child: InputDecorator(
        decoration: const InputDecoration(
          labelText: 'Centre overlay',
          border: OutlineInputBorder(),
        ),
        child: Column(
          children: [
            SegmentedButton<_OverlayMode>(
              segments: const [
                ButtonSegment(value: _OverlayMode.none, label: Text('None')),
                ButtonSegment(value: _OverlayMode.text, label: Text('Text')),
                ButtonSegment(value: _OverlayMode.image, label: Text('Image')),
              ],
              selected: {_overlayMode},
              onSelectionChanged: (selection) {
                setState(() => _overlayMode = selection.first);
                _regenerate();
              },
            ),
            if (_overlayMode == _OverlayMode.text) ...[
              const SizedBox(height: 12),
              TextField(
                controller: _overlayTextController,
                minLines: 1,
                maxLines: 3,
                decoration: const InputDecoration(
                  labelText: 'Overlay text',
                  border: OutlineInputBorder(),
                ),
                onChanged: (_) {
                  _overlayTextDebounce?.cancel();
                  _overlayTextDebounce = Timer(
                    const Duration(milliseconds: 500),
                    _regenerate,
                  );
                },
              ),
              const SizedBox(height: 12),
              _buildOverlayTextStyleOptions(context),
            ],
            if (_overlayMode == _OverlayMode.image) ...[
              const SizedBox(height: 12),
              Row(
                mainAxisAlignment: MainAxisAlignment.center,
                children: [
                  if (_overlayImage != null) ...[
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
                            CustomPaint(painter: _CheckerPainter()),
                            Image.memory(_overlayImage!, fit: BoxFit.contain),
                          ],
                        ),
                      ),
                    ),
                    IconButton(
                      tooltip: 'Remove image',
                      icon: const Icon(Icons.close),
                      onPressed: () {
                        setState(() => _overlayImage = null);
                        _regenerate();
                      },
                    ),
                    const SizedBox(width: 8),
                  ],
                  OutlinedButton.icon(
                    onPressed: _pickOverlayImage,
                    icon: const Icon(Icons.image),
                    label: const Text('Choose…'),
                  ),
                  const SizedBox(width: 8),
                  OutlinedButton.icon(
                    onPressed: _pasteOverlayImage,
                    icon: const Icon(Icons.content_paste),
                    label: const Text('Paste'),
                  ),
                ],
              ),
            ],
            if (_overlayMode != _OverlayMode.none) ...[
              const SizedBox(height: 8),
              Row(
                children: [
                  const Text('Size'),
                  Expanded(
                    child: Slider(
                      value: _overlaySize,
                      min: 0.2,
                      max: 1.0,
                      divisions: 8,
                      label: '${(_overlayShare * 100).toStringAsFixed(1)}%',
                      onChanged: (value) =>
                          setState(() => _overlaySize = value),
                      onChangeEnd: (_) => _regenerate(),
                    ),
                  ),
                  SizedBox(
                    width: 56,
                    child: Text(
                      '${(_overlayShare * 100).toStringAsFixed(1)}%',
                      textAlign: TextAlign.end,
                    ),
                  ),
                ],
              ),
              Text(
                'Up to ${maxPercent.toStringAsFixed(0)}% of the code at this '
                'error correction level; raise it for a larger overlay.',
                style: const TextStyle(fontSize: 12, color: Colors.grey),
                textAlign: TextAlign.center,
              ),
            ],
          ],
        ),
      ),
    );
  }
}

/// What is drawn over the centre of a generated code.
enum _OverlayMode { none, text, image }

/// A searchable list of the installed font families, each previewed in its own
/// font; pops with the chosen family.
class _InstalledFontPicker extends StatefulWidget {
  const _InstalledFontPicker({required this.families});

  final Future<List<String>> families;

  @override
  State<_InstalledFontPicker> createState() => _InstalledFontPickerState();
}

class _InstalledFontPickerState extends State<_InstalledFontPicker> {
  String _filter = '';

  @override
  Widget build(BuildContext context) {
    return AlertDialog(
      title: const Text('Installed fonts'),
      content: SizedBox(
        width: 420,
        height: 480,
        child: FutureBuilder<List<String>>(
          future: widget.families,
          builder: (context, snapshot) {
            if (snapshot.hasError) {
              return Center(
                child: Text('Could not list the fonts: ${snapshot.error}'),
              );
            }
            final families = snapshot.data;
            if (families == null) {
              return const Center(child: CircularProgressIndicator());
            }
            final query = _filter.toLowerCase();
            final shown = [
              for (final family in families)
                if (family.toLowerCase().contains(query)) family,
            ];
            return Column(
              children: [
                TextField(
                  autofocus: true,
                  decoration: InputDecoration(
                    prefixIcon: const Icon(Icons.search),
                    hintText: 'Search ${families.length} fonts',
                    border: const OutlineInputBorder(),
                  ),
                  onChanged: (text) => setState(() => _filter = text),
                ),
                const SizedBox(height: 8),
                Expanded(
                  child: shown.isEmpty
                      ? const Center(child: Text('No matching fonts'))
                      : ListView.builder(
                          itemCount: shown.length,
                          itemBuilder: (context, index) {
                            final family = shown[index];
                            return ListTile(
                              dense: true,
                              // The name in the app's font stays readable even
                              // for symbol fonts; the sample shows the font.
                              title: Text(family),
                              subtitle: Text(
                                'Tricode 123',
                                overflow: TextOverflow.ellipsis,
                                style: TextStyle(
                                  fontFamily: family,
                                  fontSize: 20,
                                ),
                              ),
                              onTap: () => Navigator.of(context).pop(family),
                            );
                          },
                        ),
                ),
              ],
            );
          },
        ),
      ),
      actions: [
        TextButton(
          onPressed: () => Navigator.of(context).pop(),
          child: const Text('Cancel'),
        ),
      ],
    );
  }
}

/// A ready-made pair of code colours offered above the colour pickers.
@visibleForTesting
class ColorPreset {
  const ColorPreset(this.name, this.foreground, this.background);

  final String name;
  final Color foreground;
  final Color background;

  bool matches(Color foreground, Color background) =>
      foreground.toARGB32() == this.foreground.toARGB32() &&
      background.toARGB32() == this.background.toARGB32();
}

/// Suggested the first time the Horizon style is picked.
@visibleForTesting
const zeroDawnPreset = ColorPreset(
  'Zero Dawn',
  Color(0xFF90ADD3),
  Color(0xFFC0CAD7),
);

/// The two colours of a preset must differ in brightness by at least
/// `MIN_LUMA_GAP` (native/tricode/src/lib.rs); either may be the lighter one.
@visibleForTesting
const colorPresets = [
  ColorPreset('Classic', Color(0xFF000000), Color(0xFFFFFFFF)),
  zeroDawnPreset,
  ColorPreset('Ink', Color(0xFF1B2A41), Color(0xFFF4EFE6)),
  ColorPreset('Forest', Color(0xFF1F4D3A), Color(0xFFE8F0E3)),
  ColorPreset('Terracotta', Color(0xFF7A2E1F), Color(0xFFF6E7D8)),
  ColorPreset('Plum', Color(0xFF4A1F4D), Color(0xFFF3E9F2)),
  ColorPreset('Night', Color(0xFFF2F4F5), Color(0xFF1E2A33)),
  ColorPreset('Blueprint', Color(0xFFFFFFFF), Color(0xFF1F4E79)),
];

/// A small triangle in the preset's triangle colour on its background.
class _PresetPreview extends StatelessWidget {
  const _PresetPreview({required this.preset});

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

class _ColorSwatch extends StatelessWidget {
  const _ColorSwatch({
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
                    child: CustomPaint(painter: _CheckerPainter()),
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

class _CheckerPainter extends CustomPainter {
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
  bool shouldRepaint(_CheckerPainter old) => false;
}

class _ContentTypeBadge extends StatelessWidget {
  const _ContentTypeBadge(this.contentType);
  final String contentType;

  @override
  Widget build(BuildContext context) {
    final (label, color) = switch (contentType) {
      'url' => ('URL', Colors.blue),
      'plain' => ('PLAIN', Colors.orange),
      _ => ('TEXT', Colors.green),
    };
    return Align(
      alignment: Alignment.centerLeft,
      child: Container(
        padding: const EdgeInsets.symmetric(horizontal: 8, vertical: 2),
        decoration: BoxDecoration(
          color: color.withAlpha(30),
          border: Border.all(color: color),
          borderRadius: BorderRadius.circular(4),
        ),
        child: Text(
          label,
          style: TextStyle(
            fontSize: 11,
            fontWeight: FontWeight.bold,
            color: color,
            letterSpacing: 0.8,
          ),
        ),
      ),
    );
  }
}

class _DebugTable extends StatelessWidget {
  const _DebugTable(this.result);
  final DecodeResult result;

  @override
  Widget build(BuildContext context) {
    final rows = [
      (
        'Decode method',
        result.decodeMethod.isEmpty ? '—' : result.decodeMethod,
      ),
      ('Image size', '${result.imageWidth} × ${result.imageHeight} px'),
      ('Triangles found', '${result.totalTriangles}'),
      ('Grid', '${result.rowCount} rows × ${result.columnCount} columns'),
      ('Data bits', '${result.bitCount}'),
      (
        'Error correction',
        result.eccLevel.isEmpty
            ? '—'
            : '${result.eccLevel}: ${result.correctedBytes} bytes corrected, '
                  '${result.erasedBytes} erased',
      ),
    ];
    return Table(
      columnWidths: const {0: IntrinsicColumnWidth(), 1: FlexColumnWidth()},
      children: rows.map((row) {
        return TableRow(
          children: [
            Padding(
              padding: const EdgeInsets.only(right: 16, bottom: 4),
              child: Text(
                row.$1,
                style: const TextStyle(fontSize: 12, color: Colors.grey),
              ),
            ),
            Padding(
              padding: const EdgeInsets.only(bottom: 4),
              child: Text(
                row.$2,
                style: const TextStyle(fontSize: 12, fontFamily: 'monospace'),
              ),
            ),
          ],
        );
      }).toList(),
    );
  }
}

class _GenerateDebugTable extends StatelessWidget {
  const _GenerateDebugTable(this.result);
  final GeneratedImage result;

  @override
  Widget build(BuildContext context) {
    final rows = [
      ('Grid', '${result.gridRows} rows × ${result.gridColumns} columns'),
      ('Encoded bytes', '${result.encodedBytes}'),
      ('Data bits used', '${result.dataBitsUsed}'),
      ('Padding bits', '${result.paddingBits}'),
      ('Mask', '${result.maskName} (#${result.maskIndex})'),
      (
        'Colour changes',
        '${result.colourTransitions} / ${result.sharedEdges} edges'
            ' (${result.sharedEdges == 0 ? 0 : (100 * result.colourTransitions / result.sharedEdges).toStringAsFixed(1)}%)',
      ),
      ('Decodes', result.decodeWarning.isEmpty ? 'yes' : 'no'),
    ];
    return Table(
      columnWidths: const {0: IntrinsicColumnWidth(), 1: FlexColumnWidth()},
      children: rows.map((row) {
        return TableRow(
          children: [
            Padding(
              padding: const EdgeInsets.only(right: 16, bottom: 4),
              child: Text(
                row.$1,
                style: const TextStyle(fontSize: 12, color: Colors.grey),
              ),
            ),
            Padding(
              padding: const EdgeInsets.only(bottom: 4),
              child: Text(
                row.$2,
                style: const TextStyle(fontSize: 12, fontFamily: 'monospace'),
              ),
            ),
          ],
        );
      }).toList(),
    );
  }
}

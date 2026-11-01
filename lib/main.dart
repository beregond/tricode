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
  runApp(const MyApp());
}

class MyApp extends StatelessWidget {
  const MyApp({super.key});

  // This widget is the root of your application.
  @override
  Widget build(BuildContext context) {
    return MaterialApp(
      title: 'Flutter Demo',
      theme: ThemeData(
        // This is the theme of your application.
        //
        // TRY THIS: Try running your application with "flutter run". You'll see
        // the application has a purple toolbar. Then, without quitting the app,
        // try changing the seedColor in the colorScheme below to Colors.green
        // and then invoke "hot reload" (save your changes or press the "hot
        // reload" button in a Flutter-supported IDE, or press "r" if you used
        // the command line to start the app).
        //
        // Notice that the counter didn't reset back to zero; the application
        // state is not lost during the reload. To reset the state, use hot
        // restart instead.
        //
        // This works for code too, not just values: Most code changes can be
        // tested with just a hot reload.
        colorScheme: .fromSeed(seedColor: Colors.deepPurple),
      ),
      home: const MyHomePage(),
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

  void _sendTextToRust() {
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
    ).sendSignalToRust();
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

      final reader = await clipboard.read();

      // Try PNG first
      Uint8List? imageBytes;
      if (reader.canProvide(Formats.png)) {
        final completer = Completer<Uint8List?>();
        reader.getFile(Formats.png, (file) async {
          try {
            final bytes = await file.readAll();
            completer.complete(bytes);
          } catch (e) {
            completer.complete(null);
          }
        });
        imageBytes = await completer.future;
      }

      // If no PNG, try JPEG
      if (imageBytes == null && reader.canProvide(Formats.jpeg)) {
        final completer = Completer<Uint8List?>();
        reader.getFile(Formats.jpeg, (file) async {
          try {
            final bytes = await file.readAll();
            completer.complete(bytes);
          } catch (e) {
            completer.complete(null);
          }
        });
        imageBytes = await completer.future;
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
          backgroundColor: Theme.of(context).colorScheme.inversePrimary,
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
                        content: CheckboxListTile(
                          title: const Text('Show debug info'),
                          value: _showDebugInfo,
                          onChanged: (value) {
                            setDialogState(() {});
                            final enabled = value ?? false;
                            setState(() {
                              _showDebugInfo = enabled;
                              if (!enabled && _selectedPayloadType == 'plain') {
                                _selectedPayloadType = 'text';
                              }
                              if (!enabled) {
                                _selectedMask = null;
                              }
                            });
                          },
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
                    ],
                  ),
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
    super.dispose();
  }
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

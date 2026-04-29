import 'dart:async';
import 'dart:io';
import 'dart:math';
import 'dart:ui' as ui;

import 'package:flutter/foundation.dart';

import 'package:flutter/material.dart';
import 'package:rinf/rinf.dart';
import 'package:file_selector/file_selector.dart';
import 'package:path_provider/path_provider.dart';
import 'package:url_launcher/url_launcher.dart';

import 'src/bindings/bindings.dart';

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
  final TextEditingController _widthRatioController = TextEditingController(
    text: '21',
  );
  final TextEditingController _heightRatioController = TextEditingController(
    text: '13',
  );
  StreamSubscription<RustSignalPack<GeneratedImage>>? _generatedImageSub;
  StreamSubscription<RustSignalPack<DecodeResult>>? _decodeResultSub;
  Uint8List? _latestImage;
  double _zoomLevel = 1.0;
  int? _imageWidth;
  int? _imageHeight;
  bool _isGenerating = false;
  bool _isDecoding = false;
  int _selectedStyle = 0; // 0 = Tiles, 1 = Stain
  bool? _decodeSuccess;
  String? _decodeText;
  String _decodeContentType = '';
  DecodeResult? _decodeDebug;
  GeneratedImage? _generateDebug;
  bool _showDebugInfo = false;
  String _selectedPayloadType = 'text';

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
        });
      }
    }

    _textController.addListener(clearPreviewIfChanged);
    _textController.addListener(() => setState(() {}));
    _widthRatioController.addListener(clearPreviewIfChanged);
    _heightRatioController.addListener(clearPreviewIfChanged);

    _generatedImageSub = GeneratedImage.rustSignalStream.listen((signal) async {
      if (!mounted) return;
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
    });

    SmallText(
      text: text,
      widthRatio: safeWidth,
      heightRatio: safeHeight,
      style: _selectedStyle,
      payloadType: _selectedPayloadType,
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

        if (mounted) {
          ScaffoldMessenger.of(context).showSnackBar(
            SnackBar(content: Text('Image saved to: ${filePath.path}')),
          );
        }
      }
    } catch (e) {
      if (mounted) {
        ScaffoldMessenger.of(
          context,
        ).showSnackBar(SnackBar(content: Text('Error saving image: $e')));
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
                      ElevatedButton.icon(
                        onPressed: _pickAndDecodeImage,
                        icon: const Icon(Icons.file_upload),
                        label: const Text('Select Image File'),
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
                                await launchUrl(uri, mode: LaunchMode.externalApplication);
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
                    SegmentedButton<String>(
                      segments: [
                        const ButtonSegment(value: 'text', label: Text('Text')),
                        const ButtonSegment(value: 'url', label: Text('URL')),
                        if (_showDebugInfo)
                          const ButtonSegment(value: 'plain', label: Text('Plain')),
                      ],
                      selected: {_selectedPayloadType},
                      onSelectionChanged: (selection) {
                        setState(() {
                          _selectedPayloadType = selection.first;
                        });
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
                    ] else if (_latestImage != null) ...[
                      const Divider(height: 40, thickness: 1),
                      ElevatedButton.icon(
                        onPressed: _downloadImage,
                        icon: const Icon(Icons.download),
                        label: const Text('Download Image'),
                      ),
                      const SizedBox(height: 16),
                      // Image preview with zoom and pan
                      Container(
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
                          child: InteractiveViewer(
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
                        ),
                      ),
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
    _widthRatioController.dispose();
    _heightRatioController.dispose();
    super.dispose();
  }
}

class _ContentTypeBadge extends StatelessWidget {
  const _ContentTypeBadge(this.contentType);
  final String contentType;

  @override
  Widget build(BuildContext context) {
    final (label, color) = switch (contentType) {
      'url'   => ('URL', Colors.blue),
      'plain' => ('PLAIN', Colors.orange),
      _       => ('TEXT', Colors.green),
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

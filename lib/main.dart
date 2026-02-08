import 'dart:async';
import 'dart:io';
import 'dart:math';
import 'dart:typed_data';
import 'dart:ui' as ui;

import 'package:flutter/material.dart';
import 'package:rinf/rinf.dart';
import 'package:file_selector/file_selector.dart';
import 'package:path_provider/path_provider.dart';

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

class _MyHomePageState extends State<MyHomePage> {
  final TextEditingController _textController = TextEditingController();
  final TextEditingController _widthRatioController = TextEditingController(
    text: '21',
  );
  final TextEditingController _heightRatioController = TextEditingController(
    text: '13',
  );
  StreamSubscription<RustSignalPack<GeneratedImage>>? _generatedImageSub;
  StreamSubscription<RustSignalPack<DecodedText>>? _decodedTextSub;
  Uint8List? _latestImage;
  double _zoomLevel = 1.0;
  int? _imageWidth;
  int? _imageHeight;
  bool _isGenerating = false;
  String? _decodedText;
  bool _isDecoding = false;

  bool get _shouldShowZoomControls {
    if (_imageWidth == null || _imageHeight == null) return false;
    final containerWidth = MediaQuery.of(context).size.width - 32;
    final containerHeight = 400.0;
    return _imageWidth! > containerWidth || _imageHeight! > containerHeight;
  }

  @override
  void initState() {
    super.initState();
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
      });
    });

    _decodedTextSub = DecodedText.rustSignalStream.listen((signal) {
      if (!mounted) return;
      setState(() {
        _decodedText = signal.message.text;
        _isDecoding = false;
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
        _decodedText = null;
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
          bottom: const TabBar(
            tabs: [
              Tab(text: 'Read'),
              Tab(text: 'Generate'),
            ],
          ),
        ),
        body: TabBarView(
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
                      ] else if (_decodedText != null) ...[
                        const Text(
                          'Decoded text:',
                          style: TextStyle(
                            fontSize: 16,
                            fontWeight: FontWeight.bold,
                          ),
                        ),
                        const SizedBox(height: 8),
                        Container(
                          width: double.infinity,
                          padding: const EdgeInsets.all(16.0),
                          decoration: BoxDecoration(
                            border: Border.all(color: Colors.grey),
                            borderRadius: BorderRadius.circular(8),
                          ),
                          child: SelectableText(
                            _decodedText!,
                            style: const TextStyle(fontSize: 14),
                          ),
                        ),
                      ],
                    ],
                  ),
                ),
              ),
            ),
            Center(
              child: SingleChildScrollView(
                child: Column(
                  mainAxisAlignment: MainAxisAlignment.center,
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
                    ElevatedButton(
                      onPressed: _sendTextToRust,
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
                      const Text('Generated image:'),
                      const SizedBox(height: 8),
                      // Zoom controls - only show if image is bigger than container
                      if (_shouldShowZoomControls) ...[
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
                        const SizedBox(height: 8),
                      ],
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
                      const SizedBox(height: 8),
                      ElevatedButton.icon(
                        onPressed: _downloadImage,
                        icon: const Icon(Icons.download),
                        label: const Text('Download Image'),
                      ),
                      const SizedBox(height: 20),
                    ],
                  ],
                ),
              ),
            ),
          ],
        ),
      ),
    );
  }

  @override
  void dispose() {
    _generatedImageSub?.cancel();
    _decodedTextSub?.cancel();
    _textController.dispose();
    _widthRatioController.dispose();
    _heightRatioController.dispose();
    super.dispose();
  }
}

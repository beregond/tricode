import 'dart:async';
import 'dart:io';
import 'dart:typed_data';

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
      home: const MyHomePage(title: 'Flutter Demo Home Page'),
    );
  }
}

class MyHomePage extends StatefulWidget {
  const MyHomePage({super.key, required this.title});

  // This widget is the home page of your application. It is stateful, meaning
  // that it has a State object (defined below) that contains fields that affect
  // how it looks.

  // This class is the configuration for the state. It holds the values (in this
  // case the title) provided by the parent (in this case the App widget) and
  // used by the build method of the State. Fields in a Widget subclass are
  // always marked "final".

  final String title;

  @override
  State<MyHomePage> createState() => _MyHomePageState();
}

class _MyHomePageState extends State<MyHomePage> {
  final TextEditingController _textController = TextEditingController();
  final TextEditingController _widthRatioController = TextEditingController(text: '21');
  final TextEditingController _heightRatioController = TextEditingController(text: '13');
  StreamSubscription<RustSignalPack<GeneratedImage>>? _generatedImageSub;
  Uint8List? _latestImage;
  double _zoomLevel = 1.0;

  @override
  void initState() {
    super.initState();
    _generatedImageSub = GeneratedImage.rustSignalStream.listen((signal) {
      if (!mounted) return;
      setState(() {
        _latestImage = Uint8List.fromList(signal.message.pngBytes);
        _zoomLevel = 1.0; // Reset zoom when new image is generated
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
    
    SmallText(
      text: text,
      widthRatio: safeWidth,
      heightRatio: safeHeight,
    ).sendSignalToRust();
  }

  Future<void> _downloadImage() async {
    if (_latestImage == null) return;

    try {
      // Generate a default filename with timestamp
      final timestamp = DateTime.now().toIso8601String().replaceAll(':', '-').split('.')[0];
      final defaultName = 'generated_image_$timestamp.png';

      if (Platform.isAndroid || Platform.isIOS) {
        // For mobile platforms, save to downloads directory
        final directory = Platform.isAndroid
            ? await getExternalStorageDirectory()
            : await getApplicationDocumentsDirectory();
        
        if (directory == null) {
          if (mounted) {
            ScaffoldMessenger.of(context).showSnackBar(
              const SnackBar(content: Text('Could not access storage directory')),
            );
          }
          return;
        }

        final filePath = '${directory.path}/$defaultName';
        final file = File(filePath);
        await file.writeAsBytes(_latestImage!);

        if (mounted) {
          ScaffoldMessenger.of(context).showSnackBar(
            SnackBar(content: Text('Image saved to: $filePath')),
          );
        }
      } else {
        // For desktop platforms, use file picker
        final filePath = await getSaveLocation(
          suggestedName: defaultName,
          acceptedTypeGroups: [
            const XTypeGroup(
              label: 'PNG Images',
              extensions: ['png'],
            ),
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
        ScaffoldMessenger.of(context).showSnackBar(
          SnackBar(content: Text('Error saving image: $e')),
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
          title: Text(widget.title),
          bottom: const TabBar(
            tabs: [
              Tab(text: 'Generate'),
              Tab(text: 'Read'),
            ],
          ),
        ),
        body: TabBarView(
          children: [
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
                            child: Text(':', style: TextStyle(fontSize: 24, fontWeight: FontWeight.bold)),
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
                    if (_latestImage != null) ...[
                      const Text('Generated image:'),
                      const SizedBox(height: 8),
                      // Zoom controls
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
                              label: '${(_zoomLevel * 100).toStringAsFixed(0)}%',
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
                      // Image preview with zoom and pan
                      Container(
                        constraints: BoxConstraints(
                          maxWidth: MediaQuery.of(context).size.width - 32,
                          maxHeight: 400,
                        ),
                        decoration: BoxDecoration(
                          border: Border.all(color: Colors.grey),
                          borderRadius: BorderRadius.circular(8),
                        ),
                        child: InteractiveViewer(
                          transformationController: TransformationController(),
                          boundaryMargin: const EdgeInsets.all(20),
                          minScale: 0.5,
                          maxScale: 5.0,
                          child: Transform.scale(
                            scale: _zoomLevel,
                            child: ClipRRect(
                              borderRadius: BorderRadius.circular(8),
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
            const Center(
              child: Padding(
                padding: EdgeInsets.all(24.0),
                child: Text(
                  'Lorem ipsum dolor sit amet',
                  textAlign: TextAlign.center,
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
    _textController.dispose();
    _widthRatioController.dispose();
    _heightRatioController.dispose();
    super.dispose();
  }
}

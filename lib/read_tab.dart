/// The Read tab: decode a code from an image file or the clipboard.
library;

import 'dart:async';
import 'dart:typed_data';

import 'package:flutter/material.dart';
import 'package:logging/logging.dart';
import 'package:rinf/rinf.dart';
import 'package:url_launcher/url_launcher.dart';

import 'debug_tables.dart';
import 'image_io.dart';
import 'src/bindings/bindings.dart';

final _log = Logger('read_tab');

/// Reads codes. Its result is cleared when the user leaves the tab, which
/// [tabController] tells it about; [tabIndex] is its own index there.
class ReadTab extends StatefulWidget {
  const ReadTab({
    super.key,
    required this.tabController,
    required this.tabIndex,
    required this.showDebugInfo,
  });

  final TabController tabController;
  final int tabIndex;
  final bool showDebugInfo;

  @override
  State<ReadTab> createState() => _ReadTabState();
}

class _ReadTabState extends State<ReadTab> with AutomaticKeepAliveClientMixin {
  StreamSubscription<RustSignalPack<DecodeResult>>? _decodeResultSub;
  bool _isDecoding = false;

  /// The latest result; null before the first decode and while decoding.
  DecodeResult? _result;

  @override
  bool get wantKeepAlive => true;

  @override
  void initState() {
    super.initState();
    widget.tabController.addListener(_onTabChanged);
    _decodeResultSub = DecodeResult.rustSignalStream.listen((signal) {
      if (!mounted) return;
      setState(() {
        _isDecoding = false;
        _result = signal.message;
      });
    });
  }

  @override
  void dispose() {
    widget.tabController.removeListener(_onTabChanged);
    _decodeResultSub?.cancel();
    super.dispose();
  }

  /// Clears the result when the user leaves this tab.
  void _onTabChanged() {
    final controller = widget.tabController;
    if (controller.indexIsChanging &&
        controller.previousIndex == widget.tabIndex) {
      setState(() {
        _result = null;
        _isDecoding = false;
      });
    }
  }

  Future<void> _pickAndDecodeImage() async {
    try {
      final bytes = await pickImageFile(const ['png', 'jpg', 'jpeg']);
      if (bytes != null) _decode(bytes);
    } catch (e, stack) {
      _log.warning('Could not read image file', e, stack);
      if (mounted) showMessage(context, 'Error reading file: $e');
    }
  }

  Future<void> _pasteAndDecodeImage() async {
    final bytes = await pasteImage(context);
    if (bytes != null) _decode(bytes);
  }

  void _decode(Uint8List bytes) {
    setState(() {
      _isDecoding = true;
      _result = null;
    });
    ImageBytes(bytes: bytes).sendSignalToRust();
  }

  @override
  Widget build(BuildContext context) {
    super.build(context);
    final result = _result;
    return Center(
      child: SingleChildScrollView(
        child: Padding(
          padding: const EdgeInsets.all(24.0),
          child: Column(
            mainAxisAlignment: MainAxisAlignment.center,
            children: [
              const Text(
                'Select an image to decode',
                style: TextStyle(fontSize: 18, fontWeight: FontWeight.bold),
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
              ] else if (result != null && result.success)
                _DecodedText(result)
              else if (result != null)
                const Text(
                  'Decoding failed',
                  style: TextStyle(
                    fontSize: 16,
                    fontWeight: FontWeight.bold,
                    color: Colors.red,
                  ),
                ),
              if (widget.showDebugInfo && result != null) ...[
                const SizedBox(height: 24),
                const Divider(),
                const SizedBox(height: 8),
                const DebugHeading(),
                const SizedBox(height: 8),
                DecodeDebugTable(result),
                if (result.annotatedImage.isNotEmpty) _AnnotatedImage(result),
              ],
            ],
          ),
        ),
      ),
    );
  }
}

/// A successfully decoded payload: its type, and its text (a link for URLs).
class _DecodedText extends StatelessWidget {
  const _DecodedText(this.result);

  final DecodeResult result;

  @override
  Widget build(BuildContext context) {
    final text = result.text;
    final contentType = result.contentType;
    return Column(
      children: [
        const Text(
          'Decoded successfully:',
          style: TextStyle(
            fontSize: 16,
            fontWeight: FontWeight.bold,
            color: Colors.green,
          ),
        ),
        const SizedBox(height: 8),
        _ContentTypeBadge(contentType),
        const SizedBox(height: 6),
        if (contentType == 'url')
          GestureDetector(
            onTap: () async {
              final uri = Uri.tryParse(text);
              if (uri != null && await canLaunchUrl(uri)) {
                await launchUrl(uri, mode: LaunchMode.externalApplication);
              }
            },
            child: _TextBox(
              borderColor: Colors.green,
              child: Text(
                text,
                style: const TextStyle(
                  fontSize: 14,
                  color: Colors.blue,
                  decoration: TextDecoration.underline,
                ),
              ),
            ),
          )
        else
          _TextBox(
            borderColor: contentType == 'plain' ? Colors.orange : Colors.green,
            child: SelectableText(text, style: const TextStyle(fontSize: 14)),
          ),
      ],
    );
  }
}

class _TextBox extends StatelessWidget {
  const _TextBox({required this.borderColor, required this.child});

  final Color borderColor;
  final Widget child;

  @override
  Widget build(BuildContext context) {
    return Container(
      width: double.infinity,
      padding: const EdgeInsets.all(16.0),
      decoration: BoxDecoration(
        border: Border.all(color: borderColor),
        borderRadius: BorderRadius.circular(8),
      ),
      child: child,
    );
  }
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

/// The input image with the detected triangles outlined, and buttons to save
/// or copy it.
class _AnnotatedImage extends StatelessWidget {
  const _AnnotatedImage(this.result);

  final DecodeResult result;

  @override
  Widget build(BuildContext context) {
    final png = Uint8List.fromList(result.annotatedImage);
    return Column(
      children: [
        const SizedBox(height: 12),
        const Align(
          alignment: Alignment.centerLeft,
          child: Text(
            'Annotated image:',
            style: TextStyle(fontSize: 13, color: Colors.grey),
          ),
        ),
        const SizedBox(height: 8),
        Row(
          mainAxisAlignment: MainAxisAlignment.center,
          children: [
            ElevatedButton.icon(
              onPressed: () => savePng(context, png, 'annotated_image'),
              icon: const Icon(Icons.download),
              label: const Text('Download'),
            ),
            const SizedBox(width: 12),
            ElevatedButton.icon(
              onPressed: () => copyPng(context, png),
              icon: const Icon(Icons.copy),
              label: const Text('Copy'),
            ),
          ],
        ),
        const SizedBox(height: 8),
        Container(
          constraints: const BoxConstraints(maxWidth: 400, maxHeight: 400),
          decoration: BoxDecoration(
            border: Border.all(color: Colors.grey),
            borderRadius: BorderRadius.circular(8),
          ),
          child: ClipRRect(
            borderRadius: BorderRadius.circular(7),
            child: Image.memory(png, fit: BoxFit.contain),
          ),
        ),
      ],
    );
  }
}

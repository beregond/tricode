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
import 'overlay.dart';
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

  Future<void> _takePhotoAndDecode() async {
    try {
      final bytes = await takePhoto();
      if (bytes != null) _decode(bytes);
    } catch (e, stack) {
      _log.warning('Could not take photo', e, stack);
      if (mounted) showMessage(context, 'Error taking photo: $e');
    }
  }

  /// Decodes an image in any format Flutter reads: Rust only reads PNG, so
  /// anything else (JPEG photos, mostly) is re-encoded first.
  Future<void> _decode(Uint8List bytes) async {
    setState(() {
      _isDecoding = true;
      _result = null;
    });
    try {
      final png = _isPng(bytes) ? bytes : await imageToPng(bytes);
      ImageBytes(bytes: png).sendSignalToRust();
    } catch (e, stack) {
      _log.warning('Could not convert image to PNG', e, stack);
      if (!mounted) return;
      setState(() => _isDecoding = false);
      showMessage(context, 'Could not read image: $e');
    }
  }

  static bool _isPng(Uint8List bytes) =>
      bytes.length >= 8 &&
      bytes[0] == 0x89 &&
      bytes[1] == 0x50 &&
      bytes[2] == 0x4E &&
      bytes[3] == 0x47;

  @override
  Widget build(BuildContext context) {
    super.build(context);
    final result = _result;
    return Center(
      child: SingleChildScrollView(
        padding: EdgeInsets.only(bottom: scrollEndPadding(context)),
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
              Wrap(
                alignment: WrapAlignment.center,
                spacing: 16,
                runSpacing: 12,
                children: [
                  if (canTakePhoto)
                    ElevatedButton.icon(
                      onPressed: _takePhotoAndDecode,
                      icon: const Icon(Icons.photo_camera),
                      label: const Text('Take Photo'),
                    ),
                  ElevatedButton.icon(
                    onPressed: _pickAndDecodeImage,
                    icon: const Icon(Icons.file_upload),
                    label: const Text('Select Image File'),
                  ),
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

/// A successfully decoded payload: its type, its text, and for URLs a button
/// that opens it.
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
        _TextBox(
          borderColor: contentType == 'plain' ? Colors.orange : Colors.green,
          child: SelectableText(text, style: const TextStyle(fontSize: 14)),
        ),
        if (contentType == 'url') ...[
          const SizedBox(height: 8),
          _OpenUrlButton(text),
        ],
      ],
    );
  }
}

/// Opens [url] in the browser, then stays disabled (showing a spinner) for
/// [_cooldown], so repeated clicks don't open a pile of tabs.
class _OpenUrlButton extends StatefulWidget {
  const _OpenUrlButton(this.url);

  final String url;

  @override
  State<_OpenUrlButton> createState() => _OpenUrlButtonState();
}

class _OpenUrlButtonState extends State<_OpenUrlButton> {
  static const _cooldown = Duration(seconds: 3);

  Timer? _cooldownTimer;

  @override
  void dispose() {
    _cooldownTimer?.cancel();
    super.dispose();
  }

  Future<void> _open() async {
    setState(() {
      _cooldownTimer = Timer(_cooldown, () {
        if (mounted) setState(() => _cooldownTimer = null);
      });
    });
    final uri = Uri.tryParse(widget.url);
    try {
      if (uri == null ||
          !await launchUrl(uri, mode: LaunchMode.externalApplication)) {
        _log.warning('Could not open URL (${widget.url.length} chars)');
      }
    } catch (e, stack) {
      _log.warning('Opening URL failed', e, stack);
    }
  }

  @override
  Widget build(BuildContext context) {
    final coolingDown = _cooldownTimer != null;
    return FilledButton.icon(
      onPressed: coolingDown ? null : _open,
      style: FilledButton.styleFrom(
        minimumSize: const Size(220, 56),
        padding: const EdgeInsets.symmetric(horizontal: 32, vertical: 16),
        textStyle: const TextStyle(fontSize: 18, fontWeight: FontWeight.w600),
        iconSize: 24,
      ),
      icon: coolingDown
          ? const SizedBox.square(
              dimension: 24,
              child: CircularProgressIndicator(strokeWidth: 2.5),
            )
          : const Icon(Icons.open_in_new),
      label: const Text('Open URL'),
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
              onPressed: () => savePng(context, png, 'tricode-annotated'),
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

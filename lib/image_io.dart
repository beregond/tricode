/// Saving, copying and pasting images, with a snack bar telling the user how
/// it went.
library;

import 'dart:io';

import 'package:file_selector/file_selector.dart';
import 'package:flutter/foundation.dart';
import 'package:flutter/material.dart';
import 'package:gal/gal.dart';
import 'package:image_picker/image_picker.dart';
import 'package:logging/logging.dart';
import 'package:super_clipboard/super_clipboard.dart';
import 'package:url_launcher/url_launcher.dart';

import 'overlay.dart';

final _log = Logger('image_io');

/// Shows [text] in a snack bar, if [context] is still mounted.
void showMessage(BuildContext context, String text) {
  if (!context.mounted) return;
  ScaffoldMessenger.of(context).showSnackBar(SnackBar(content: Text(text)));
}

/// Space below a tab's scrolling content, so its last buttons can be scrolled
/// clear of the system navigation bar and of snack bars on a phone.
double scrollEndPadding(BuildContext context) =>
    MediaQuery.paddingOf(context).bottom + 80;

/// Saves [png] as `{prefix}_{timestamp}.png`: on mobile into the photo
/// gallery, on desktop wherever the user picks.
Future<void> savePng(BuildContext context, Uint8List png, String prefix) async {
  final timestamp = DateTime.now()
      .toIso8601String()
      .replaceAll(':', '-')
      .split('.')[0];
  final name = '${prefix}_$timestamp';
  try {
    if (Platform.isAndroid || Platform.isIOS) {
      // The app's own storage directory is hidden from gallery and file
      // manager apps, so mobile saves go to the gallery instead.
      await Gal.putImageBytes(png, name: name);
      if (context.mounted) showMessage(context, 'Image saved to the gallery');
    } else {
      final location = await getSaveLocation(
        suggestedName: '$name.png',
        acceptedTypeGroups: [
          const XTypeGroup(label: 'PNG Images', extensions: ['png']),
        ],
      );
      if (location == null) return; // canceled
      await XFile.fromData(
        png,
        mimeType: 'image/png',
        name: '$name.png',
      ).saveTo(location.path);
      if (context.mounted) _showSavedSnackBar(context, location.path);
    }
  } on GalException catch (e, stack) {
    _log.warning('Saving image to the gallery failed', e, stack);
    if (context.mounted) {
      showMessage(context, 'Error saving image: ${e.type.message}');
    }
  } catch (e, stack) {
    _log.warning('Saving image failed', e, stack);
    if (context.mounted) showMessage(context, 'Error saving image: $e');
  }
}

/// Copies [png] to the system clipboard.
Future<void> copyPng(BuildContext context, Uint8List png) async {
  try {
    final clipboard = SystemClipboard.instance;
    if (clipboard == null) {
      showMessage(context, 'Clipboard not available on this platform');
      return;
    }
    final item = DataWriterItem()..add(Formats.png(png));
    await clipboard.write([item]);
    if (context.mounted) showMessage(context, 'Image copied to clipboard');
  } catch (e, stack) {
    _log.warning('Copying image failed', e, stack);
    if (context.mounted) showMessage(context, 'Error copying to clipboard: $e');
  }
}

/// The image on the clipboard (PNG or JPEG bytes), or null after telling the
/// user why there is none.
Future<Uint8List?> pasteImage(BuildContext context) async {
  try {
    final bytes = await readClipboardImage();
    if (bytes == null && context.mounted) {
      showMessage(context, 'No image found in clipboard');
    }
    return bytes;
  } on ClipboardUnavailable {
    if (context.mounted) {
      showMessage(context, 'Clipboard not available on this platform');
    }
  } catch (e, stack) {
    _log.warning('Clipboard paste failed', e, stack);
    if (context.mounted) {
      showMessage(context, 'Error pasting from clipboard: $e');
    }
  }
  return null;
}

/// Lets the user pick an image file; null when they cancel.
Future<Uint8List?> pickImageFile(List<String> extensions) async {
  final typeGroup = XTypeGroup(label: 'images', extensions: extensions);
  final file = await openFile(acceptedTypeGroups: [typeGroup]);
  return file?.readAsBytes();
}

/// Whether this device can take a photo with [takePhoto]: phones and tablets.
bool get canTakePhoto => !kIsWeb && (Platform.isAndroid || Platform.isIOS);

/// Longest side, in pixels, photos are scaled down to. Codes filling a good
/// part of the frame keep triangles far wider than the 37 px minimum, and
/// decoding a full-resolution photo would take several times longer.
const _photoMaxSide = 2048.0;

/// Lets the user take a photo with the camera; null when they cancel.
Future<Uint8List?> takePhoto() async {
  final photo = await ImagePicker().pickImage(
    source: ImageSource.camera,
    maxWidth: _photoMaxSide,
    maxHeight: _photoMaxSide,
  );
  return photo?.readAsBytes();
}

/// Tells the user where a file was saved, with an action that reveals it in
/// the system file manager.
void _showSavedSnackBar(BuildContext context, String path) {
  ScaffoldMessenger.of(context).showSnackBar(
    SnackBar(
      content: Text('Image saved to: $path'),
      duration: const Duration(seconds: 8),
      action: SnackBarAction(
        label: _revealLabel,
        onPressed: () async {
          if (await _revealInFileManager(path) || !context.mounted) return;
          showMessage(context, 'Could not open the file manager');
        },
      ),
    ),
  );
}

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

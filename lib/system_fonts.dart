/// Lists the font families installed on a desktop computer, so text overlays
/// can use them. Flutter draws an installed font given its family name, but has
/// no way to list them, so each desktop system is asked its own way.
library;

import 'dart:convert';
import 'dart:io';

import 'package:flutter/foundation.dart';
import 'package:flutter/services.dart';

/// Whether [installedFontFamilies] can list fonts here: desktops only, since
/// phones neither expose their fonts nor let apps use most of them by name.
bool get canListInstalledFonts =>
    !kIsWeb && (Platform.isLinux || Platform.isMacOS || Platform.isWindows);

/// Answered by macos/Runner/MainFlutterWindow.swift.
const _macChannel = MethodChannel('tricode/fonts');

/// The installed font families, sorted case-insensitively without duplicates.
/// Throws when the system can't be asked.
Future<List<String>> installedFontFamilies() async {
  final List<String> families;
  if (Platform.isLinux) {
    families = await _linuxFamilies();
  } else if (Platform.isMacOS) {
    families =
        await _macChannel.invokeListMethod<String>('families') ?? const [];
  } else if (Platform.isWindows) {
    families = await _windowsFamilies();
  } else {
    families = const [];
  }
  final unique = {
    for (final family in families)
      // macOS lists hidden system fonts with a leading dot.
      if (family.trim().isNotEmpty && !family.startsWith('.')) family.trim(),
  }.toList();
  unique.sort((a, b) => a.toLowerCase().compareTo(b.toLowerCase()));
  return unique;
}

/// fontconfig prints one line per font face: its family names separated by
/// commas (the first is the main one), with `\` escaping `-`, `,` and `:`.
Future<List<String>> _linuxFamilies() async {
  final result = await Process.run('fc-list', [
    ':',
    'family',
  ], stdoutEncoding: utf8);
  if (result.exitCode != 0) {
    throw ProcessException('fc-list', [], '${result.stderr}');
  }
  return [
    for (final line in (result.stdout as String).split('\n'))
      if (line.isNotEmpty) fcListFamily(line),
  ];
}

/// The main family name in a line of `fc-list : family` output.
@visibleForTesting
String fcListFamily(String line) {
  final name = StringBuffer();
  for (var i = 0; i < line.length; i++) {
    final char = line[i];
    if (char == '\\' && i + 1 < line.length) {
      name.write(line[++i]);
    } else if (char == ',') {
      break;
    } else {
      name.write(char);
    }
  }
  return name.toString();
}

/// Windows has no command for this, but PowerShell (part of every Windows
/// since 7) can ask .NET, which lists every installed family by name.
Future<List<String>> _windowsFamilies() async {
  const script =
      '[Console]::OutputEncoding = [Text.Encoding]::UTF8; '
      'Add-Type -AssemblyName System.Drawing; '
      '(New-Object System.Drawing.Text.InstalledFontCollection).Families '
      '| ForEach-Object { \$_.Name }';
  final result = await Process.run('powershell', [
    '-NoProfile',
    '-NonInteractive',
    '-Command',
    script,
  ], stdoutEncoding: utf8);
  if (result.exitCode != 0) {
    throw ProcessException('powershell', [], '${result.stderr}');
  }
  return const LineSplitter().convert(result.stdout as String);
}

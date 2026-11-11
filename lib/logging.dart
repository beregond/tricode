/// One log for the whole app: Dart code logs through `package:logging`, and
/// Rust's `tracing` events arrive as [RustLog] signals and join it under
/// `rust.<target>`. Records go to DevTools' Logging view, and to the console
/// in debug builds.
library;

import 'dart:developer' as developer;

import 'package:flutter/foundation.dart';
import 'package:logging/logging.dart';

import 'src/bindings/bindings.dart';

/// Which Rust events reach Dart, from `--dart-define=RUST_LOG=...`, e.g.
/// `info,tricode_core=trace`. Empty keeps Rust's default (hub and tricode-core at debug,
/// the rest at info). Release builds drop Rust's debug and trace events at
/// compile time.
const _rustLogFilter = String.fromEnvironment('RUST_LOG');

/// Rust `tracing` levels as `package:logging` levels.
const _rustLevels = {
  'TRACE': Level.FINEST,
  'DEBUG': Level.FINE,
  'INFO': Level.INFO,
  'WARN': Level.WARNING,
  'ERROR': Level.SEVERE,
};

/// Routes Dart and Rust records to the log and catches uncaught errors.
/// Call before `initializeRust`, so Rust's first events aren't missed.
void setUpLogging() {
  Logger.root.level = kDebugMode ? Level.ALL : Level.INFO;
  Logger.root.onRecord.listen(_emit);

  RustLog.rustSignalStream.listen((signal) {
    final log = signal.message;
    Logger(
      'rust.${log.target}',
    ).log(_rustLevels[log.level] ?? Level.INFO, log.message);
  });

  final flutterLog = Logger('flutter');
  FlutterError.onError = (details) {
    FlutterError.presentError(details);
    flutterLog.severe(
      details.exceptionAsString(),
      details.exception,
      details.stack,
    );
  };
  PlatformDispatcher.instance.onError = (error, stack) {
    Logger('app').severe('Uncaught error', error, stack);
    return true;
  };
}

/// Sends the `RUST_LOG` filter to Rust, if one was given. Call after
/// `initializeRust`.
void sendRustLogFilter() {
  if (_rustLogFilter.isEmpty) return;
  RustLogFilter(directives: _rustLogFilter).sendSignalToRust();
}

void _emit(LogRecord record) {
  developer.log(
    record.message,
    time: record.time,
    sequenceNumber: record.sequenceNumber,
    level: record.level.value,
    name: record.loggerName,
    error: record.error,
    stackTrace: record.stackTrace,
  );
  if (!kDebugMode) return;
  debugPrint(
    '${record.level.name.padRight(7)} ${record.loggerName}: ${record.message}'
    '${record.error == null ? '' : '\n${record.error}'}'
    '${record.stackTrace == null ? '' : '\n${record.stackTrace}'}',
  );
}

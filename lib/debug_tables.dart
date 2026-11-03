/// Tables of debug information about a decoded or generated code, shown when
/// "Debug Mode" is on.
library;

import 'package:flutter/material.dart';

import 'src/bindings/bindings.dart';

/// How a code was read.
class DecodeDebugTable extends StatelessWidget {
  const DecodeDebugTable(this.result, {super.key});

  final DecodeResult result;

  @override
  Widget build(BuildContext context) {
    return _InfoTable([
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
    ]);
  }
}

/// How a code was laid out.
class GenerateDebugTable extends StatelessWidget {
  const GenerateDebugTable(this.result, {super.key});

  final GeneratedImage result;

  @override
  Widget build(BuildContext context) {
    final changeShare = result.sharedEdges == 0
        ? '0'
        : (100 * result.colourTransitions / result.sharedEdges).toStringAsFixed(
            1,
          );
    return _InfoTable([
      ('Grid', '${result.gridRows} rows × ${result.gridColumns} columns'),
      ('Encoded bytes', '${result.encodedBytes}'),
      ('Data bits used', '${result.dataBitsUsed}'),
      ('Padding bits', '${result.paddingBits}'),
      ('Mask', '${result.maskName} (#${result.maskIndex})'),
      (
        'Colour changes',
        '${result.colourTransitions} / ${result.sharedEdges} edges'
            ' ($changeShare%)',
      ),
      ('Decodes', result.decodeWarning.isEmpty ? 'yes' : 'no'),
    ]);
  }
}

/// Labels in grey beside monospaced values.
class _InfoTable extends StatelessWidget {
  const _InfoTable(this.rows);

  final List<(String, String)> rows;

  @override
  Widget build(BuildContext context) {
    return Table(
      columnWidths: const {0: IntrinsicColumnWidth(), 1: FlexColumnWidth()},
      children: [
        for (final (label, value) in rows)
          TableRow(
            children: [
              Padding(
                padding: const EdgeInsets.only(right: 16, bottom: 4),
                child: Text(
                  label,
                  style: const TextStyle(fontSize: 12, color: Colors.grey),
                ),
              ),
              Padding(
                padding: const EdgeInsets.only(bottom: 4),
                child: Text(
                  value,
                  style: const TextStyle(fontSize: 12, fontFamily: 'monospace'),
                ),
              ),
            ],
          ),
      ],
    );
  }
}

/// The small grey "Debug info" heading above a debug table.
class DebugHeading extends StatelessWidget {
  const DebugHeading({super.key});

  @override
  Widget build(BuildContext context) {
    return const Align(
      alignment: Alignment.centerLeft,
      child: Text(
        'Debug info',
        style: TextStyle(
          fontSize: 13,
          fontWeight: FontWeight.bold,
          color: Colors.grey,
        ),
      ),
    );
  }
}

import 'dart:math';

import 'package:flutter/foundation.dart';
import 'package:flutter/material.dart';

import 'generate_tab.dart';
import 'read_tab.dart';

/// Light, dark or follow the system; picked in the settings dialog.
final themeMode = ValueNotifier(ThemeMode.system);

/// The app's only page: the Read and Generate tabs, and the settings.
class HomePage extends StatefulWidget {
  const HomePage({super.key});

  @override
  State<HomePage> createState() => _HomePageState();
}

class _HomePageState extends State<HomePage>
    with SingleTickerProviderStateMixin {
  static const _readTab = 0;
  static const _generateTab = 1;

  late final TabController _tabController = TabController(
    length: 2,
    vsync: this,
  );
  bool _showDebugInfo = false;

  @override
  void dispose() {
    _tabController.dispose();
    super.dispose();
  }

  @override
  Widget build(BuildContext context) {
    return Scaffold(
      appBar: AppBar(
        title: Row(
          children: [
            const Tooltip(message: 'Tricode', child: TricodeLogo(size: 28)),
            const SizedBox(width: 8),
            Expanded(
              child: TabBar(
                controller: _tabController,
                dividerColor: Colors.transparent,
                indicatorSize: TabBarIndicatorSize.tab,
                tabs: const [
                  Tab(text: 'Read'),
                  Tab(text: 'Generate'),
                ],
              ),
            ),
          ],
        ),
        actions: [
          Padding(
            // Clear of the debug banner.
            padding: EdgeInsets.only(right: kDebugMode ? 48.0 : 0.0),
            child: IconButton(
              icon: const Icon(Icons.settings),
              tooltip: 'Settings',
              onPressed: () => showDialog<void>(
                context: context,
                builder: (context) => _SettingsDialog(
                  showDebugInfo: _showDebugInfo,
                  onShowDebugInfoChanged: (enabled) =>
                      setState(() => _showDebugInfo = enabled),
                ),
              ),
            ),
          ),
        ],
      ),
      body: TabBarView(
        controller: _tabController,
        children: [
          ReadTab(
            tabController: _tabController,
            tabIndex: _readTab,
            showDebugInfo: _showDebugInfo,
          ),
          GenerateTab(
            tabController: _tabController,
            tabIndex: _generateTab,
            showDebugInfo: _showDebugInfo,
          ),
        ],
      ),
    );
  }
}

/// The app logo, as in `assets/logo/tricode.svg`: a triangle split at its edge
/// midpoints with the middle one cut out, drawn in the SVG's dark grey in both
/// themes.
class TricodeLogo extends StatelessWidget {
  const TricodeLogo({super.key, this.size = 24});

  /// The logo's fill in `assets/logo/tricode.svg`.
  static const color = Color(0xFF1A1A1A);

  final double size;

  @override
  Widget build(BuildContext context) {
    return CustomPaint(
      size: Size.square(size),
      painter: _TricodeLogoPainter(color),
    );
  }
}

class _TricodeLogoPainter extends CustomPainter {
  _TricodeLogoPainter(this.color);

  final Color color;

  @override
  void paint(Canvas canvas, Size size) {
    final w = size.width;
    final h = w * sqrt(3) / 2;
    final top = (size.height - h) / 2;
    final bottom = top + h;
    final middle = top + h / 2;
    final path = Path()
      ..fillType = PathFillType.evenOdd
      ..addPolygon([
        Offset(w / 2, top),
        Offset(w, bottom),
        Offset(0, bottom),
      ], true)
      ..addPolygon([
        Offset(w / 4, middle),
        Offset(w * 3 / 4, middle),
        Offset(w / 2, bottom),
      ], true);
    canvas.drawPath(path, Paint()..color = color);
  }

  @override
  bool shouldRepaint(_TricodeLogoPainter oldDelegate) =>
      color != oldDelegate.color;
}

/// The theme, and debug info on or off.
class _SettingsDialog extends StatefulWidget {
  const _SettingsDialog({
    required this.showDebugInfo,
    required this.onShowDebugInfoChanged,
  });

  final bool showDebugInfo;
  final ValueChanged<bool> onShowDebugInfoChanged;

  @override
  State<_SettingsDialog> createState() => _SettingsDialogState();
}

class _SettingsDialogState extends State<_SettingsDialog> {
  late bool _showDebugInfo = widget.showDebugInfo;

  @override
  Widget build(BuildContext context) {
    return AlertDialog(
      title: const Text('Settings'),
      content: Column(
        mainAxisSize: MainAxisSize.min,
        crossAxisAlignment: CrossAxisAlignment.start,
        children: [
          const Text('Theme'),
          const SizedBox(height: 8),
          ValueListenableBuilder(
            valueListenable: themeMode,
            builder: (context, mode, _) => SegmentedButton<ThemeMode>(
              segments: const [
                ButtonSegment(
                  value: ThemeMode.system,
                  icon: Icon(Icons.brightness_auto),
                  label: Text('System'),
                ),
                ButtonSegment(
                  value: ThemeMode.light,
                  icon: Icon(Icons.light_mode),
                  label: Text('Light'),
                ),
                ButtonSegment(
                  value: ThemeMode.dark,
                  icon: Icon(Icons.dark_mode),
                  label: Text('Dark'),
                ),
              ],
              selected: {mode},
              onSelectionChanged: (selection) =>
                  themeMode.value = selection.first,
            ),
          ),
          const SizedBox(height: 16),
          CheckboxListTile(
            title: const Text('Debug Mode'),
            value: _showDebugInfo,
            onChanged: (value) {
              setState(() => _showDebugInfo = value ?? false);
              widget.onShowDebugInfoChanged(_showDebugInfo);
            },
          ),
        ],
      ),
      actions: [
        TextButton(
          onPressed: () => Navigator.of(context).pop(),
          child: const Text('Close'),
        ),
      ],
    );
  }
}

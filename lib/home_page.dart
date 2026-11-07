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

/// The app logo: `assets/logo/tricode.png`, rendered from `assets/logo/tricode.svg`
/// by `tricode-regen-icons`. A bitmap scaled down with mipmaps keeps the edges
/// smooth where a painted path can come out jagged (Impeller on Linux draws
/// paths without anti-aliasing).
class TricodeLogo extends StatelessWidget {
  const TricodeLogo({super.key, this.size = 24});

  final double size;

  @override
  Widget build(BuildContext context) {
    return Image.asset(
      'assets/logo/tricode.png',
      width: size,
      height: size,
      filterQuality: FilterQuality.medium,
    );
  }
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
          // One row per mode, rather than segmented buttons, so it fits the
          // narrow dialog on a phone.
          ValueListenableBuilder(
            valueListenable: themeMode,
            builder: (context, mode, _) => RadioGroup<ThemeMode>(
              groupValue: mode,
              onChanged: (value) => themeMode.value = value ?? mode,
              child: const Column(
                mainAxisSize: MainAxisSize.min,
                children: [
                  _ThemeOption(
                    ThemeMode.system,
                    Icons.brightness_auto,
                    'System',
                  ),
                  _ThemeOption(ThemeMode.light, Icons.light_mode, 'Light'),
                  _ThemeOption(ThemeMode.dark, Icons.dark_mode, 'Dark'),
                ],
              ),
            ),
          ),
          const Divider(height: 24),
          CheckboxListTile(
            contentPadding: EdgeInsets.zero,
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

/// A theme mode's row in the settings dialog.
class _ThemeOption extends StatelessWidget {
  const _ThemeOption(this.mode, this.icon, this.label);

  final ThemeMode mode;
  final IconData icon;
  final String label;

  @override
  Widget build(BuildContext context) {
    return RadioListTile<ThemeMode>(
      value: mode,
      contentPadding: EdgeInsets.zero,
      dense: true,
      title: Text(label),
      secondary: Icon(icon),
    );
  }
}

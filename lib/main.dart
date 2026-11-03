import 'package:flutter/material.dart';
import 'package:rinf/rinf.dart';

import 'home_page.dart';
import 'logging.dart';
import 'overlay.dart';
import 'src/bindings/bindings.dart';

Future<void> main() async {
  setUpLogging();
  await initializeRust(assignRustSignal);
  sendRustLogFilter();
  registerOverlayFontLicenses();
  runApp(const MyApp());
}

/// The app's brand colour; both colour schemes are generated from it.
const _primaryColor = Color(0xFF4a97dd);

/// A colour scheme generated from [_primaryColor], keeping it as the exact primary colour.
ColorScheme _colorScheme(Brightness brightness) => ColorScheme.fromSeed(
  seedColor: _primaryColor,
  brightness: brightness,
  dynamicSchemeVariant: DynamicSchemeVariant.fidelity,
).copyWith(primary: _primaryColor, onPrimary: Colors.white);

ThemeData _theme(Brightness brightness) {
  final colorScheme = _colorScheme(brightness);
  return ThemeData(
    colorScheme: colorScheme,
    appBarTheme: AppBarTheme(
      backgroundColor: colorScheme.primary,
      foregroundColor: colorScheme.onPrimary,
    ),
    tabBarTheme: TabBarThemeData(
      labelColor: colorScheme.onPrimary,
      unselectedLabelColor: colorScheme.onPrimary.withValues(alpha: 0.7),
      indicatorColor: colorScheme.onPrimary,
    ),
  );
}

class MyApp extends StatelessWidget {
  const MyApp({super.key});

  @override
  Widget build(BuildContext context) {
    return ValueListenableBuilder(
      valueListenable: themeMode,
      builder: (context, mode, _) => MaterialApp(
        title: 'Tricode',
        theme: _theme(Brightness.light),
        darkTheme: _theme(Brightness.dark),
        themeMode: mode,
        home: const HomePage(),
      ),
    );
  }
}

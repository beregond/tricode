import 'package:flutter/material.dart';

/// A searchable list of the installed font families, each previewed in its own
/// font; pops with the chosen family.
class InstalledFontPicker extends StatefulWidget {
  const InstalledFontPicker({super.key, required this.families});

  final Future<List<String>> families;

  @override
  State<InstalledFontPicker> createState() => _InstalledFontPickerState();
}

class _InstalledFontPickerState extends State<InstalledFontPicker> {
  String _filter = '';

  @override
  Widget build(BuildContext context) {
    return AlertDialog(
      title: const Text('Installed fonts'),
      content: SizedBox(
        width: 420,
        height: 480,
        child: FutureBuilder<List<String>>(
          future: widget.families,
          builder: (context, snapshot) {
            if (snapshot.hasError) {
              return Center(
                child: Text('Could not list the fonts: ${snapshot.error}'),
              );
            }
            final families = snapshot.data;
            if (families == null) {
              return const Center(child: CircularProgressIndicator());
            }
            final query = _filter.toLowerCase();
            final shown = [
              for (final family in families)
                if (family.toLowerCase().contains(query)) family,
            ];
            return Column(
              children: [
                TextField(
                  autofocus: true,
                  decoration: InputDecoration(
                    prefixIcon: const Icon(Icons.search),
                    hintText: 'Search ${families.length} fonts',
                    border: const OutlineInputBorder(),
                  ),
                  onChanged: (text) => setState(() => _filter = text),
                ),
                const SizedBox(height: 8),
                Expanded(
                  child: shown.isEmpty
                      ? const Center(child: Text('No matching fonts'))
                      : ListView.builder(
                          itemCount: shown.length,
                          itemBuilder: (context, index) {
                            final family = shown[index];
                            return ListTile(
                              dense: true,
                              // The name in the app's font stays readable even
                              // for symbol fonts; the sample shows the font.
                              title: Text(family),
                              subtitle: Text(
                                'Tricode 123',
                                overflow: TextOverflow.ellipsis,
                                style: TextStyle(
                                  fontFamily: family,
                                  fontSize: 20,
                                ),
                              ),
                              onTap: () => Navigator.of(context).pop(family),
                            );
                          },
                        ),
                ),
              ],
            );
          },
        ),
      ),
      actions: [
        TextButton(
          onPressed: () => Navigator.of(context).pop(),
          child: const Text('Cancel'),
        ),
      ],
    );
  }
}

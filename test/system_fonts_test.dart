import 'package:flutter_test/flutter_test.dart';

import 'package:tricode_app/system_fonts.dart';

void main() {
  test('fc-list lines give their first family, unescaped', () {
    expect(fcListFamily('Noto Sans Arabic'), 'Noto Sans Arabic');
    expect(fcListFamily('DejaVu Sans,DejaVu Sans Condensed'), 'DejaVu Sans');
    expect(fcListFamily(r'Font\-Name\, Inc,Alias'), 'Font-Name, Inc');
  });
}

import Cocoa
import FlutterMacOS

class MainFlutterWindow: NSWindow {
  override func awakeFromNib() {
    let flutterViewController = FlutterViewController()
    let windowFrame = self.frame
    self.contentViewController = flutterViewController
    self.setFrame(windowFrame, display: true)

    RegisterGeneratedPlugins(registry: flutterViewController)

    // Lists installed font families for text overlays (lib/system_fonts.dart);
    // Flutter can draw them by name but has no way to list them.
    let fontChannel = FlutterMethodChannel(
      name: "tricode/fonts",
      binaryMessenger: flutterViewController.engine.binaryMessenger)
    fontChannel.setMethodCallHandler { call, result in
      if call.method == "families" {
        result(NSFontManager.shared.availableFontFamilies)
      } else {
        result(FlutterMethodNotImplemented)
      }
    }

    super.awakeFromNib()
  }
}

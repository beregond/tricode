{{flutter_js}}
{{flutter_build_config}}

// Replaces Flutter's default bootstrap, which registers its deprecated service worker; that
// worker unregisters itself on activation and would remove coi_service_worker.js with it.
(async () => {
  // The Rust module needs a cross-origin isolated page (see coi_service_worker.js). When the
  // server doesn't send the headers, a service worker adds them, from the next load on.
  if (!self.crossOriginIsolated && self.isSecureContext && 'serviceWorker' in navigator) {
    const reloadedKey = 'tricode-coi-reloaded';
    try {
      await navigator.serviceWorker.register('coi_service_worker.js');
      if (!sessionStorage.getItem(reloadedKey)) {
        sessionStorage.setItem(reloadedKey, '1');
        if (navigator.serviceWorker.controller) {
          location.reload();
        } else {
          navigator.serviceWorker.addEventListener('controllerchange', () => location.reload());
        }
        return;
      }
    } catch (e) {
      console.error('Could not register coi_service_worker.js', e);
    }
    // Reloaded once already and still not isolated: start anyway, so the app at least shows.
    console.error(
      'The page is not cross-origin isolated, so generating and reading codes will fail. ' +
        'Serve it with Cross-Origin-Opener-Policy: same-origin and ' +
        'Cross-Origin-Embedder-Policy: require-corp.',
    );
  }
  if (!self.crossOriginIsolated && !self.isSecureContext) {
    // Service workers need a secure context: HTTPS, or http on localhost or 127.0.0.1. Python's
    // http.server prints http://0.0.0.0:8000/, which isn't one.
    console.error(
      'The page is not cross-origin isolated, so generating and reading codes will fail: ' +
        `${location.origin} is not a secure context. Open it on http://localhost:${location.port} ` +
        'or over HTTPS, or serve it with Cross-Origin-Opener-Policy: same-origin and ' +
        'Cross-Origin-Embedder-Policy: require-corp.',
    );
  }
  try {
    sessionStorage.removeItem('tricode-coi-reloaded');
  } catch (_) {}
  _flutter.loader.load();
})();

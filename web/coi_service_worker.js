// Makes the page cross-origin isolated when the server doesn't, by adding the two headers to
// every same-origin response. The Rust module needs that isolation: it runs its work on web
// workers sharing memory with the page, and browsers only allow SharedArrayBuffer on isolated
// pages. Registered by flutter_bootstrap.js, which reloads the page once so this worker serves it.
'use strict';

self.addEventListener('install', () => self.skipWaiting());

self.addEventListener('activate', (event) => event.waitUntil(self.clients.claim()));

self.addEventListener('fetch', (event) => {
  const request = event.request;
  // Cross-origin resources keep their own headers (CanvasKit's CDN allows embedding).
  if (new URL(request.url).origin !== self.location.origin) return;
  // Chrome throws on these outside same-origin mode.
  if (request.cache === 'only-if-cached' && request.mode !== 'same-origin') return;
  event.respondWith(
    fetch(request).then((response) => {
      if (response.status === 0) return response; // opaque: headers can't be changed
      const headers = new Headers(response.headers);
      headers.set('Cross-Origin-Opener-Policy', 'same-origin');
      headers.set('Cross-Origin-Embedder-Policy', 'require-corp');
      return new Response(response.body, {
        status: response.status,
        statusText: response.statusText,
        headers,
      });
    }),
  );
});

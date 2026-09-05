# Windows native browser dispatch verification

Status: ready-for-human

2026-09-05, current debug Tauri shell with the current embedded web bundle.

```powershell
node .scratch/link-opening/probe-native-browser.mjs
```

Result: **PASS**. The probe launched its own `server/target/debug/laplus.exe` with fresh application data, an isolated WebView2 profile, a separate server port, and a dedicated CDP port. It attached to the actual Tauri WebView2 page, then clicked an HTTP anchor with `target="_blank"` and an HTTP anchor targeting the existing window. Both exercised shell callbacks and reached the OS default browser. A local HTTP fixture recorded the exact path and query, including `&` and `%20`.

The shell used Edge WebView2 (user agent ending `Edg/152.0.0.0`). Both external loads used the default Brave browser (Chromium user agent without the Edge suffix); the Windows HTTP association was a Brave ProgID. The shell retained its original origin after both clicks. Same-window navigation also generated an initial WebView2 request before the external browser request, so the probe explicitly waits for a different browser user agent instead of treating the first HTTP request as sufficient proof.

The isolated shell closed through its native Tauri window-close command, and the probe and local HTTP server exited. The installed application and its provider sessions were not stopped. Test data was retained under the probe's printed temporary directory. No pairing credentials or user conversations were logged.

This validates native HTTP dispatch, query preservation, new-window handling, and cross-origin navigation handling on this machine. It does not validate HTTPS content rendering, `mailto`, or other machines' browser associations. The CDP probe injects ordinary anchors into the real shell page; it does not fabricate a provider conversation or test Markdown parsing.

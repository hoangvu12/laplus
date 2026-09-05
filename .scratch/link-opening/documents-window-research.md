# External links sometimes open Documents

Status: ready-for-agent
Date: 2026-09-05
Type: research

Follow-up: native Windows dispatch is now implemented through `open` 5.4.3
(resolved in Cargo.lock). [Native shell verification](native-verification.md)
passed; the findings below describe the previous implementation.

## Finding

The Windows desktop link opener directly starts `explorer.exe` with the URL as its argument. This is confirmed in source and is the best next boundary to investigate. The user's intermittent Documents-window symptom has **not** been reproduced, and this research does not establish which URL or Windows state triggers it. No implementation or browser associations were changed and no links were launched.

The user subsequently confirmed that the affected input is an HTTPS link and the unwanted window is Windows Documents. The exact URL remains unavailable.

## Confirmed local evidence

- `apps/web/src/components/ChatMarkdown.tsx:1392` renders external Markdown links as `_blank` anchors. The context menu passes the original URL to `api.shell.openExternal`.
- `apps/web/src/localApi.ts:29` falls back to `window.open(url, "_blank", "noopener,noreferrer")` when there is no Electron bridge. That is the normal Tauri path; `server/docs/adr/0021-the-page-commands-the-shell-through-a-named-list.md` explains why laplus has no `desktopBridge`.
- `server/crates/laplus-shell/src/main.rs:233` handles new-window requests and line 244 handles external navigation. Both call `open_in_the_system_browser`.
- `main.rs:267` allows only `http`, `https`, and `mailto`; a `file://` link does not pass this opener gate.
- `main.rs:282` chooses `explorer.exe` on Windows and calls `Command::spawn`. It reports only failure to create that process. Successful process creation cannot prove that Windows opened the requested page, and the child result is never observed.
- Existing `external_opening_is_an_allowlist_of_schemes_the_os_may_carry` tests exercise URL scheme classification, not dispatch to a browser.
- A read-only PowerShell check of `HKCU:\Software\Microsoft\Windows\Shell\Associations\UrlAssociations\{http,https}\UserChoice` returned a BraveHTML ProgId for both protocols. That makes a simple association with Explorer less likely, but does not prove the browser registration is healthy.

## Supported alternative

Windows provides `ShellExecuteExW` for shell activation, including registered URL protocols. It returns success/failure and exposes errors such as missing associations. Microsoft recommends COM initialization, including STA where required by shell extensions. A wrapper must respect those thread requirements. [Microsoft API documentation](https://learn.microsoft.com/en-us/windows/win32/api/shellapi/nf-shellapi-shellexecuteexw)

Tauri's opener offers Rust URL opening as well as JavaScript APIs. Its standalone Rust `open_url` forwards to `open::that_detached` for the system default application; it does not require adding a partial Electron bridge. Consequently, the current comment that dismisses an opener crate as only an IPC facility is too broad. [Tauri documentation](https://tauri.app/plugin/opener/), [Tauri opener implementation](https://raw.githubusercontent.com/tauri-apps/plugins-workspace/v2/plugins/opener/src/open.rs)

The locally cached `tauri-plugin-opener-2.5.5/Cargo.toml` explicitly enables `open`'s `shellexecute-on-windows` feature. `open` 5.4.2 is cached too, but neither is currently in this repository's Cargo lockfile. If selecting the smaller direct dependency, explicitly enable that feature and verify the resolved Windows implementation; simply adding an opener dependency does not guarantee that it avoids command launchers. [open 5.4.2 API](https://docs.rs/open/5.4.2/open/fn.that_detached.html), [upstream Windows implementation](https://raw.githubusercontent.com/Byron/open-rs/main/src/windows.rs)

Concrete implementation choice: add a Windows-target dependency `open = { version = "5.4.2", features = ["shellexecute-on-windows"] }` and call `open::that_detached(url)` from the existing Tauri callback. The cached 5.4.2 implementation at `src/windows.rs:292` places the UTF-16 URL in `SHELLEXECUTEINFOW.lpFile` and calls `ShellExecuteExW` directly. Calling `open::that` or `open::commands` does not select the same path. Keep activation on the Tauri UI thread rather than moving it blindly to an uninitialized worker. `server/Cargo.toml:186` forbids unsafe code, making the maintained safe wrapper preferable to handwritten FFI.

## Proposed next work

1. Capture one failing link target and which control was clicked, with query tokens redacted. Compare that URL with a plain HTTPS URL through the same Tauri hook. Distinguish a Windows Documents window from laplus's integrated file panel.
2. Add a test seam at external dispatch: preserve the existing scheme gate, capture the exact URL sent to a fake OS opener, and propagate a simulated launcher error. Cover punctuation, percent encoding, Unicode, query/fragment, and mailto. Such tests validate dispatch fidelity; they cannot establish that Explorer produced Documents.
3. Replace Windows `explorer.exe` URL launching with supported shell activation. Preserve Rust-owned navigation hooks and existing scheme restrictions. Prefer a maintained implementation over new handwritten FFI, accounting for its size and thread requirements. Do not add command-string interpolation.
4. Drive both a rendered Markdown link and the context-menu action in the Windows shell. For the previously failing URL, verify the browser receives its full URL and no Documents window appears. Repeated clicks should produce only the expected browser action. Test launcher failure separately so it is observable.

## Verification limitation

Source tracing, local dependency inspection, protocol-association reads, and primary documentation review completed. A mock can verify launcher choice, but cannot reproduce the user's actual Explorer-window symptom. No red-capable reproduction of that symptom exists yet; a Windows desktop reproduction with a failing URL remains necessary before declaring this bug diagnosed or fixed. No new tests were added because a test asserting only that the current source names Explorer would give false confidence about the reported behavior.

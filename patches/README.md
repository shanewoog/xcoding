# XCoding Desktop Patches

This directory contains locally patched copies of upstream crates used by the
Tauri Desktop shell. Each crate is a snapshot from the cargo registry with
XCoding-specific modifications applied.

## Why patch?

XCoding needs `print_to_pdf` on the Tauri `Webview` to export browser content
as PDF using the native WebView2 `PrintToPdf` COM API. Upstream wry/tauri do
not expose this capability.

## Modified crates

| Crate | Version | Modification |
|-------|---------|-------------|
| wry | 0.55.1 | Added `print_to_pdf` to `InnerWebView` and `WebView` using `ICoreWebView2_7::PrintToPdf` |
| tauri-runtime | 2.11.3 | Added `print_to_pdf` to `WebviewDispatch` trait |
| tauri-runtime-wry | 2.11.4 | Added `WebviewMessage::PrintToPdf` variant, dispatcher impl, and event loop handler |
| tauri | 2.11.5 | Added `print_to_pdf` to public `Webview<R>` and mock runtime |

## How to refresh

After `cargo update` changes the upstream versions:

1. Run `scripts/copy-patched-crates.ps1` to copy the new versions from the cargo registry.
2. Re-apply the `print_to_pdf` modifications to the new sources.
3. Update the version numbers in `[patch.crates-io]` in `apps/desktop/src-tauri/Cargo.toml`.
4. Update this README with the new versions.

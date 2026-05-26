# Windows WebView2 Packaging

`WebView2Loader.dll` must be shipped with the Windows executable unless the loader is statically linked. For this Tauri v1 app, the reliable portable packaging rule is:

```text
echo.exe
WebView2Loader.dll
portable.json
```

The DLL has to be next to `echo.exe`. Do not extract it at application startup; Windows may need the DLL before Rust `main()` runs.

Build a portable Windows package with:

```powershell
.\scripts\package-windows-portable.ps1 -Build -Arch x64
```

The generated zip can be used as the Windows portable update package in the MinIO manifest. Every Windows update zip must include `WebView2Loader.dll` in the same directory as `echo.exe`.

This DLL is only the WebView2 loader. It helps the app locate the WebView2 Runtime. For native installers, configure Tauri's Windows `webviewInstallMode` if you also need to install or bundle the runtime:

- `downloadBootstrapper`: smallest installer, needs network.
- `embedBootstrapper`: includes the small bootstrapper, still needs network.
- `offlineInstaller`: includes the runtime installer, works offline, much larger.
- `fixedRuntime`: ships a fixed WebView2 Runtime folder with the app, largest but most self-contained.

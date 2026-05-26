# Echo Release Flow

This project uses MinIO as the update host. The app reads one built-in manifest URL and then downloads the matching package for the current distribution, platform, and CPU architecture.

## Files Required For A Release

Upload these files to MinIO:

```text
echo-updates/
  stable/
    latest.json
    0.2.0/
      Echo-0.2.0-windows-x64-portable.zip
      Echo-0.2.0-macos-aarch64-portable.zip
      Echo_0.2.0_x64-setup.exe
```

Only upload the files you actually support. The `latest.json` manifest must only reference files that exist.

## Version Locations

Update both version fields before building:

```text
src-tauri/Cargo.toml       package.version
src-tauri/tauri.conf.json  package.version
```

The updater compares versions as semver. Use values like `0.2.0`, not `0.2`.

## Built-In Manifest URL

The app has the manifest URL compiled into:

```text
src-tauri/src/updater.rs
```

Set `BUILT_IN_UPDATE_MANIFEST_URL` to the public or presigned MinIO URL for:

```text
echo-updates/stable/latest.json
```

You can also inject it at build time:

```powershell
$env:ECHO_BUILT_IN_UPDATE_MANIFEST_URL="https://minio.example.com/echo-updates/stable/latest.json"
```

## Windows Portable

Build on Windows:

```powershell
.\scripts\package-windows-portable.ps1 -Build -Arch x64
```

The generated zip includes:

```text
echo.exe
WebView2Loader.dll
portable.json
```

Keep `WebView2Loader.dll` next to `echo.exe`. Do not move it into a subfolder.

## macOS Portable

Build on macOS:

```bash
cd frontend
npm run build
cd ../src-tauri
cargo build --release
```

Create a version directory with the executable and `portable.json`:

```bash
VERSION=0.2.0
OUT="../../dist/portable/macos-aarch64/Echo-$VERSION-macos-aarch64-portable"
rm -rf "$OUT"
mkdir -p "$OUT"
cp target/release/echo "$OUT/echo"
cat > "$OUT/portable.json" <<EOF
{
  "version": "$VERSION",
  "executable": "echo"
}
EOF
cd "$(dirname "$OUT")"
zip -r "Echo-$VERSION-macos-aarch64-portable.zip" "$(basename "$OUT")"
```

## Windows Installer

Enable bundling in `src-tauri/tauri.conf.json` for installer builds:

```json
"bundle": {
  "active": true
}
```

Then build on Windows:

```powershell
cd frontend
npm run build
cd ..\src-tauri
cargo tauri build
```

The Windows config currently uses `embedBootstrapper`, so the installer contains the WebView2 bootstrapper. If the target machines have no internet, switch to `offlineInstaller` or `fixedRuntime`.

## Checksums

Generate SHA256 and file size for every package.

macOS/Linux:

```bash
shasum -a 256 Echo-0.2.0-macos-aarch64-portable.zip
wc -c Echo-0.2.0-macos-aarch64-portable.zip
```

Windows PowerShell:

```powershell
Get-FileHash .\Echo-0.2.0-windows-x64-portable.zip -Algorithm SHA256
(Get-Item .\Echo-0.2.0-windows-x64-portable.zip).Length
```

Fill those values into `latest.json`.

## Manifest Fields

The updater matches packages using these exact values:

```text
target   portable | installer
platform windows | macos | linux
arch     x64 | aarch64
```

Example manifest:

```text
updates/stable/latest.example.json
```

## Publish Order

1. Update both version files.
2. Build packages for each platform.
3. Verify each portable zip contains `portable.json` and the executable.
4. On Windows portable, verify `WebView2Loader.dll` is next to `echo.exe`.
5. Generate SHA256 and size for each package.
6. Upload packages to `echo-updates/stable/<version>/`.
7. Upload `latest.json` last.
8. Start an older Echo version and use `帮助 -> 检查更新`.

Uploading `latest.json` last prevents clients from seeing a new version before all packages are available.

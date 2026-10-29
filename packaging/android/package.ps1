<#
.SYNOPSIS
  Build, package and sign VectorCraft for Android (arm64-v8a APK).

.DESCRIPTION
  Produces, in $env:DIST (default: dist/release):
    vectorcraft-<version>-android-arm64.apk

  Compiles the Rust NativeActivity shared library (libvectorcraft.so) for aarch64-linux-android
  with Vulkan/GLES GPU acceleration and Android storage/media permissions, compiles
  VectorCraftActivity.java into classes.dex, packages resources and icons with aapt2, aligns
  with zipalign, and signs with apksigner.

.EXAMPLE
  pwsh -NoProfile -File packaging/android/package.ps1
  pwsh -NoProfile -File packaging/android/package.ps1 -SkipBuild
#>
param(
  [switch] $SkipBuild
)
$ErrorActionPreference = 'Stop'
$Root = (Resolve-Path (Join-Path $PSScriptRoot '..\..')).Path

function Invoke-Native([string] $What, [scriptblock] $Block) {
  Write-Output "==> $What"
  & $Block
  if ($LASTEXITCODE -ne 0) { throw "$What failed with exit code $LASTEXITCODE" }
}

# Read [workspace.package] version from root Cargo.toml.
$Version = $env:VECTORCRAFT_VERSION
if (-not $Version) {
  $inPkg = $false
  foreach ($line in Get-Content (Join-Path $Root 'Cargo.toml')) {
    if ($line -match '^\s*\[') { $inPkg = ($line.Trim() -eq '[workspace.package]'); continue }
    if ($inPkg -and $line -match '^\s*version\s*=\s*"([^"]+)"') { $Version = $Matches[1]; break }
  }
}
if (-not $Version) { throw 'could not read [workspace.package] version from Cargo.toml' }

$Target = 'aarch64-linux-android'
$Dist = if ($env:DIST) { $env:DIST } else { Join-Path $Root 'dist\release' }
$TargetDir = if ($env:CARGO_TARGET_DIR) { $env:CARGO_TARGET_DIR } else { Join-Path $Root 'target' }
New-Item -ItemType Directory -Force -Path $Dist | Out-Null

# Locate Android SDK, NDK, build-tools, platform android.jar, and JDK.
$Sdk = if ($env:ANDROID_HOME) { $env:ANDROID_HOME } elseif ($env:ANDROID_SDK_ROOT) { $env:ANDROID_SDK_ROOT } else { Join-Path $env:LOCALAPPDATA 'Android\Sdk' }
if (-not (Test-Path $Sdk)) { throw "Android SDK not found at $Sdk (set ANDROID_HOME)" }

$Ndk = if ($env:ANDROID_NDK_HOME -and (Test-Path $env:ANDROID_NDK_HOME)) {
  $env:ANDROID_NDK_HOME
} else {
  Get-ChildItem (Join-Path $Sdk 'ndk') -Directory | Sort-Object Name -Descending | Select-Object -First 1 -ExpandProperty FullName
}
if (-not $Ndk) { throw "Android NDK not found under $Sdk\ndk" }

$BuildTools = Get-ChildItem (Join-Path $Sdk 'build-tools') -Directory | Sort-Object Name -Descending | Select-Object -First 1 -ExpandProperty FullName
if (-not $BuildTools) { throw "Android build-tools not found under $Sdk\build-tools" }

$PlatformDir = Get-ChildItem (Join-Path $Sdk 'platforms') -Directory | Sort-Object Name -Descending | Select-Object -First 1 -ExpandProperty FullName
$AndroidJar = if ($PlatformDir) { Join-Path $PlatformDir 'android.jar' } else { $null }
if (-not $AndroidJar -or -not (Test-Path $AndroidJar)) { throw "android.jar not found under $Sdk\platforms" }

$JavaHome = $env:JAVA_HOME
if (-not $JavaHome -or -not (Test-Path (Join-Path $JavaHome 'bin\javac.exe'))) {
  $JdkCandidate = Get-ChildItem 'C:\Program Files\Eclipse Adoptium' -Directory -ErrorAction SilentlyContinue | Sort-Object Name -Descending | Select-Object -First 1 -ExpandProperty FullName
  if ($JdkCandidate) { $JavaHome = $JdkCandidate }
}
if ($JavaHome -and (Test-Path (Join-Path $JavaHome 'bin'))) {
  $env:JAVA_HOME = $JavaHome
  $env:PATH = "$(Join-Path $JavaHome 'bin');$env:PATH"
}

$Javac = if ($JavaHome) { Join-Path $JavaHome 'bin\javac.exe' } else { 'javac' }
$Keytool = if ($JavaHome) { Join-Path $JavaHome 'bin\keytool.exe' } else { 'keytool' }
$Aapt2 = Join-Path $BuildTools 'aapt2.exe'
$D8 = Join-Path $BuildTools 'd8.bat'
$ZipAlign = Join-Path $BuildTools 'zipalign.exe'
$ApkSigner = Join-Path $BuildTools 'apksigner.bat'

$LlvmBin = Join-Path $Ndk 'toolchains\llvm\prebuilt\windows-x86_64\bin'
if (-not (Test-Path $LlvmBin)) { throw "NDK LLVM toolchain not found at $LlvmBin" }

$HostTools = Join-Path $TargetDir 'host-tools'
New-Item -ItemType Directory -Force -Path $HostTools | Out-Null
$RustupSelfContained = Join-Path $env:USERPROFILE '.rustup\toolchains\stable-x86_64-pc-windows-gnu\lib\rustlib\x86_64-pc-windows-gnu\bin\self-contained'
if (Test-Path $RustupSelfContained) {
  Copy-Item (Join-Path $RustupSelfContained 'dlltool.exe') (Join-Path $HostTools 'dlltool.exe') -Force
  Copy-Item (Join-Path $RustupSelfContained 'libwinpthread-1.dll') (Join-Path $HostTools 'libwinpthread-1.dll') -Force
  $env:PATH = "$HostTools;$env:PATH"
  $AsExe = Join-Path $HostTools 'as.exe'
  if (-not (Test-Path $AsExe)) {
    $AsWrapRs = Join-Path $HostTools 'as_wrap.rs'
    $ClangEscaped = (Join-Path $LlvmBin 'clang.exe') -replace '\\', '\\'
    Set-Content -Path $AsWrapRs -Value @"
fn main() {
    let mut cmd = std::process::Command::new("$ClangEscaped");
    cmd.args(["--target=x86_64-w64-mingw32", "-c", "-x", "assembler"]);
    let mut args = std::env::args().skip(1);
    while let Some(a) = args.next() {
        match a.as_str() {
            "--64" => {}
            "-o" => { if let Some(out) = args.next() { cmd.arg("-o").arg(out); } }
            other => { cmd.arg(other); }
        }
    }
    let status = cmd.status().unwrap();
    std::process::exit(status.code().unwrap_or(1));
}
"@
    rustc -O $AsWrapRs -o $AsExe
  }
} else {
  $env:PATH = "$LlvmBin;$env:PATH"
}

Write-Output "VectorCraft $Version for Android arm64 ($Target)"
Write-Output "  SDK:         $Sdk"
Write-Output "  NDK:         $Ndk"
Write-Output "  Build-Tools: $BuildTools"
Write-Output "  Platform:    $AndroidJar"

$ApkCargoManifest = Join-Path $PSScriptRoot 'apk\Cargo.toml'
$ApkCargoLock = Join-Path $PSScriptRoot 'apk\Cargo.lock'

if (-not $SkipBuild) {
  $env:ANDROID_NDK_HOME = $Ndk
  $env:ANDROID_NDK_ROOT = $Ndk
  $env:CC_aarch64_linux_android = Join-Path $LlvmBin 'aarch64-linux-android26-clang.cmd'
  $env:CXX_aarch64_linux_android = Join-Path $LlvmBin 'aarch64-linux-android26-clang++.cmd'
  $env:AR_aarch64_linux_android = Join-Path $LlvmBin 'llvm-ar.exe'
  $env:CARGO_TARGET_AARCH64_LINUX_ANDROID_LINKER = Join-Path $LlvmBin 'aarch64-linux-android26-clang.cmd'
  $env:CARGO_TARGET_DIR = $TargetDir

  Copy-Item (Join-Path $Root 'Cargo.lock') $ApkCargoLock -Force
  try {
    Invoke-Native "cargo build ($Target)" {
      cargo build --manifest-path $ApkCargoManifest --target $Target --release
    }
  } finally {
    Remove-Item -Force $ApkCargoLock -ErrorAction SilentlyContinue
  }
}

$SoPath = Join-Path $TargetDir "$Target\release\libvectorcraft.so"
if (-not (Test-Path $SoPath)) { throw "missing shared library $SoPath" }

# Verify ELF header (0x7F 'E' 'L' 'F', 64-bit little-endian, EM_AARCH64 = 0xB7).
$elfBytes = [System.IO.File]::ReadAllBytes($SoPath)
$machine = [BitConverter]::ToUInt16($elfBytes, 18)
if ($elfBytes[0] -ne 0x7F -or $elfBytes[1] -ne 0x45 -or $elfBytes[2] -ne 0x4C -or $elfBytes[3] -ne 0x46 -or $machine -ne 0xB7) {
  throw "libvectorcraft.so is not a valid AArch64 ELF shared library (machine=0x$('{0:X}' -f $machine))"
}
Write-Output "ok libvectorcraft.so: AArch64 ELF ($([Math]::Round($elfBytes.Length / 1MB, 2)) MB)"

$Stage = Join-Path $TargetDir 'android-package'
Remove-Item -Recurse -Force $Stage -ErrorAction SilentlyContinue
New-Item -ItemType Directory -Force -Path $Stage | Out-Null

# Stage Android Adaptive Icon resources (API 26+ <adaptive-icon> 108x108dp foreground/monochrome/background + legacy & round mipmaps)
$ResDir = Join-Path $Stage 'res'
$ValuesDir = Join-Path $ResDir 'values'
$AnyDpiDir = Join-Path $ResDir 'mipmap-anydpi-v26'
New-Item -ItemType Directory -Force -Path $ValuesDir | Out-Null
New-Item -ItemType Directory -Force -Path $AnyDpiDir | Out-Null

$ColorsXml = @'
<?xml version="1.0" encoding="utf-8"?>
<resources>
    <color name="ic_launcher_background">#E8573F</color>
</resources>
'@
[System.IO.File]::WriteAllText((Join-Path $ValuesDir 'colors.xml'), $ColorsXml, [System.Text.UTF8Encoding]::new($false))

$AdaptiveXml = @'
<?xml version="1.0" encoding="utf-8"?>
<adaptive-icon xmlns:android="http://schemas.android.com/apk/res/android">
    <background android:drawable="@color/ic_launcher_background" />
    <foreground android:drawable="@mipmap/ic_launcher_foreground" />
    <monochrome android:drawable="@mipmap/ic_launcher_monochrome" />
</adaptive-icon>
'@
[System.IO.File]::WriteAllText((Join-Path $AnyDpiDir 'ic_launcher.xml'), $AdaptiveXml, [System.Text.UTF8Encoding]::new($false))
[System.IO.File]::WriteAllText((Join-Path $AnyDpiDir 'ic_launcher_round.xml'), $AdaptiveXml, [System.Text.UTF8Encoding]::new($false))

$AdaptiveIconCs = @'
using System;
using System.IO;
using System.Drawing;
using System.Drawing.Drawing2D;
using System.Drawing.Imaging;

public static class VectorCraftAdaptiveIcon {
    public static void GenerateAll(string srcIconPath, string resDir) {
        using (Bitmap src = new Bitmap(srcIconPath)) {
            int W = src.Width;
            int H = src.Height;
            Bitmap fgSrc = new Bitmap(W, H, PixelFormat.Format32bppArgb);
            for (int y = 0; y < H; y++) {
                for (int x = 0; x < W; x++) {
                    Color c = src.GetPixel(x, y);
                    if (c.A < 16) {
                        fgSrc.SetPixel(x, y, Color.FromArgb(0, 0, 0, 0));
                        continue;
                    }
                    double dr = c.R - 232.0;
                    double dg = c.G - 87.0;
                    double db = c.B - 63.0;
                    double dist = Math.Sqrt(dr * dr + dg * dg + db * db);
                    if (dist < 28.0) {
                        fgSrc.SetPixel(x, y, Color.FromArgb(0, 0, 0, 0));
                    } else if (dist < 55.0) {
                        int a = (int)Math.Min(255.0, Math.Max(0.0, (dist - 28.0) / (55.0 - 28.0) * c.A));
                        fgSrc.SetPixel(x, y, Color.FromArgb(a, c.R, c.G, c.B));
                    } else {
                        fgSrc.SetPixel(x, y, Color.FromArgb(255, c.R, c.G, c.B));
                    }
                }
            }

            // Fill bottom-right rx=112 corner clip in fgSrc so squircle/rounded-square masks have continuous wing bleed
            double cx = 776.0, cy = 776.0, rInner = 212.0;
            for (int y = 776; y < H; y++) {
                for (int x = 776; x < W; x++) {
                    double dx = x - cx, dy = y - cy;
                    double r = Math.Sqrt(dx * dx + dy * dy);
                    if (r > rInner) {
                        double offset = (r - rInner) % 44.0;
                        double rSample = rInner - 6.0 - offset;
                        int sx = Math.Max(0, Math.Min(W - 1, (int)Math.Round(cx + dx * (rSample / r))));
                        int sy = Math.Max(0, Math.Min(H - 1, (int)Math.Round(cy + dy * (rSample / r))));
                        fgSrc.SetPixel(x, y, fgSrc.GetPixel(sx, sy));
                    }
                }
            }
            for (int y = 692; y < 776; y++) {
                for (int x = 996; x < W; x++) {
                    fgSrc.SetPixel(x, y, fgSrc.GetPixel(994 - ((x - 996) % 32), y));
                }
            }
            for (int y = 996; y < H; y++) {
                for (int x = 172; x < 776; x++) {
                    fgSrc.SetPixel(x, y, fgSrc.GetPixel(x, 994 - ((y - 996) % 32)));
                }
            }

            // Build master 432x432 (xxxhdpi 108x108dp) foreground layer with safe-zone framing (66-72dp) and parallax bleed
            int masterSize = 432;
            using (Bitmap fgMaster = new Bitmap(masterSize, masterSize, PixelFormat.Format32bppArgb))
            using (Bitmap monoMaster = new Bitmap(masterSize, masterSize, PixelFormat.Format32bppArgb)) {
                using (Graphics g = Graphics.FromImage(fgMaster)) {
                    g.SmoothingMode = SmoothingMode.HighQuality;
                    g.InterpolationMode = InterpolationMode.HighQualityBicubic;
                    g.PixelOffsetMode = PixelOffsetMode.HighQuality;
                    g.Clear(Color.FromArgb(0, 0, 0, 0));
                    g.DrawImage(fgSrc, new RectangleF(54f, 82f, 310f, 310f), new RectangleF(0, 0, W, H), GraphicsUnit.Pixel);
                }
                for (int y = 292; y < 392; y++) {
                    for (int x = 362; x < 416; x++) {
                        fgMaster.SetPixel(x, y, fgMaster.GetPixel(361 - ((x - 362) % 16), y));
                    }
                }
                for (int y = 390; y < 416; y++) {
                    for (int x = 104; x < 416; x++) {
                        fgMaster.SetPixel(x, y, fgMaster.GetPixel(x, 389 - ((y - 390) % 16)));
                    }
                }

                // Build monochrome layer (warm paper #efe9dc -> white alpha mask for Android 13+ themed icons)
                for (int y = 0; y < masterSize; y++) {
                    for (int x = 0; x < masterSize; x++) {
                        Color c = fgMaster.GetPixel(x, y);
                        if (c.A < 32) {
                            monoMaster.SetPixel(x, y, Color.FromArgb(0, 255, 255, 255));
                        } else {
                            int lum = (c.R * 299 + c.G * 587 + c.B * 114) / 1000;
                            int alpha = (c.A * lum) / 255;
                            monoMaster.SetPixel(x, y, Color.FromArgb(alpha, 255, 255, 255));
                        }
                    }
                }

                string[] densities = new string[] { "mipmap-mdpi", "mipmap-hdpi", "mipmap-xhdpi", "mipmap-xxhdpi", "mipmap-xxxhdpi" };
                int[] fgSizes = new int[] { 108, 162, 216, 324, 432 };
                int[] legacySizes = new int[] { 48, 72, 96, 144, 192 };

                for (int i = 0; i < densities.Length; i++) {
                    string dir = Path.Combine(resDir, densities[i]);
                    Directory.CreateDirectory(dir);
                    SaveScaled(fgMaster, fgSizes[i], Path.Combine(dir, "ic_launcher_foreground.png"));
                    SaveScaled(monoMaster, fgSizes[i], Path.Combine(dir, "ic_launcher_monochrome.png"));
                    SaveLegacy(fgMaster, legacySizes[i], false, Path.Combine(dir, "ic_launcher.png"));
                    SaveLegacy(fgMaster, legacySizes[i], true, Path.Combine(dir, "ic_launcher_round.png"));
                }
            }
            fgSrc.Dispose();
        }
    }

    private static void SaveScaled(Bitmap master, int size, string outPath) {
        using (Bitmap bmp = new Bitmap(size, size, PixelFormat.Format32bppArgb))
        using (Graphics g = Graphics.FromImage(bmp)) {
            g.SmoothingMode = SmoothingMode.HighQuality;
            g.InterpolationMode = InterpolationMode.HighQualityBicubic;
            g.PixelOffsetMode = PixelOffsetMode.HighQuality;
            g.Clear(Color.FromArgb(0, 0, 0, 0));
            g.DrawImage(master, new RectangleF(0, 0, size, size), new RectangleF(0, 0, master.Width, master.Height), GraphicsUnit.Pixel);
            bmp.Save(outPath, ImageFormat.Png);
        }
    }

    private static void SaveLegacy(Bitmap fgMaster, int size, bool round, string outPath) {
        using (Bitmap bmp = new Bitmap(size, size, PixelFormat.Format32bppArgb))
        using (Graphics g = Graphics.FromImage(bmp)) {
            g.SmoothingMode = SmoothingMode.AntiAlias;
            g.InterpolationMode = InterpolationMode.HighQualityBicubic;
            g.PixelOffsetMode = PixelOffsetMode.HighQuality;
            g.Clear(Color.FromArgb(0, 0, 0, 0));
            float inset = size * (2f / 48f);
            float box = size - inset * 2f;
            using (GraphicsPath mask = new GraphicsPath()) {
                if (round) {
                    mask.AddEllipse(inset, inset, box, box);
                } else {
                    float rx = box * 0.42f;
                    mask.AddArc(inset, inset, rx, rx, 180, 90);
                    mask.AddArc(inset + box - rx, inset, rx, rx, 270, 90);
                    mask.AddArc(inset + box - rx, inset + box - rx, rx, rx, 0, 90);
                    mask.AddArc(inset, inset + box - rx, rx, rx, 90, 90);
                    mask.CloseFigure();
                }
                g.SetClip(mask);
                using (SolidBrush bg = new SolidBrush(Color.FromArgb(255, 232, 87, 63))) {
                    g.FillRectangle(bg, 0, 0, size, size);
                }
                // Map the 72x72dp viewport (72..360 in 432px master) into the legacy icon box
                g.DrawImage(fgMaster, new RectangleF(inset, inset, box, box), new RectangleF(72f, 72f, 288f, 288f), GraphicsUnit.Pixel);
            }
            bmp.Save(outPath, ImageFormat.Png);
        }
    }
}
'@
if (-not ('VectorCraftAdaptiveIcon' -as [type])) {
  Add-Type -TypeDefinition $AdaptiveIconCs -ReferencedAssemblies System.Drawing
}
$SrcIcon1024 = Join-Path $Root 'assets\app-icon\vectorcraft-1024.png'
[VectorCraftAdaptiveIcon]::GenerateAll($SrcIcon1024, $ResDir)

$ResZip = Join-Path $Stage 'res.zip'
Invoke-Native 'aapt2 compile' {
  & $Aapt2 compile --dir $ResDir -o $ResZip
}

$GenJava = Join-Path $Stage 'gen'
New-Item -ItemType Directory -Force -Path $GenJava | Out-Null
$BaseApk = Join-Path $Stage 'base.apk'
$Manifest = Join-Path $PSScriptRoot 'AndroidManifest.xml'
Invoke-Native 'aapt2 link' {
  & $Aapt2 link -o $BaseApk -I $AndroidJar --manifest $Manifest --java $GenJava `
    --version-name $Version --auto-add-overlay $ResZip
}

# Compile VectorCraftActivity.java + generated R.java and convert to classes.dex.
$ClassesDir = Join-Path $Stage 'classes'
New-Item -ItemType Directory -Force -Path $ClassesDir | Out-Null
$ActivityJava = Join-Path $PSScriptRoot 'java\ai\storyteller\vectorcraft\VectorCraftActivity.java'
$RJava = Join-Path $GenJava 'ai\storyteller\vectorcraft\R.java'
Invoke-Native 'javac VectorCraftActivity.java' {
  & $Javac --release 11 -encoding UTF-8 -cp $AndroidJar -d $ClassesDir $ActivityJava $RJava
}

$DexDir = Join-Path $Stage 'dex'
New-Item -ItemType Directory -Force -Path $DexDir | Out-Null
$ClassFiles = Get-ChildItem (Join-Path $ClassesDir 'ai\storyteller\vectorcraft') -Filter '*.class' | Select-Object -ExpandProperty FullName
Invoke-Native 'd8 classes.dex' {
  & $D8 --min-api 26 --lib $AndroidJar --output $DexDir $ClassFiles
}

# Assemble unaligned APK with classes.dex and lib/arm64-v8a/libvectorcraft.so.
Add-Type -AssemblyName System.IO.Compression
Add-Type -AssemblyName System.IO.Compression.FileSystem
$UnalignedApk = Join-Path $Stage 'unaligned.apk'
Copy-Item $BaseApk $UnalignedApk -Force

$zip = [System.IO.Compression.ZipFile]::Open($UnalignedApk, [System.IO.Compression.ZipArchiveMode]::Update)
try {
  $dexFile = Join-Path $DexDir 'classes.dex'
  [System.IO.Compression.ZipFileExtensions]::CreateEntryFromFile($zip, $dexFile, 'classes.dex', [System.IO.Compression.CompressionLevel]::Optimal) | Out-Null
  [System.IO.Compression.ZipFileExtensions]::CreateEntryFromFile($zip, $SoPath, 'lib/arm64-v8a/libvectorcraft.so', [System.IO.Compression.CompressionLevel]::Optimal) | Out-Null

  # Include libc++_shared.so if present and needed by native dependencies.
  $LibCppShared = Join-Path $Ndk 'toolchains\llvm\prebuilt\windows-x86_64\sysroot\usr\lib\aarch64-linux-android\libc++_shared.so'
  if (Test-Path $LibCppShared) {
    [System.IO.Compression.ZipFileExtensions]::CreateEntryFromFile($zip, $LibCppShared, 'lib/arm64-v8a/libc++_shared.so', [System.IO.Compression.CompressionLevel]::Optimal) | Out-Null
  }
} finally {
  $zip.Dispose()
}

$AlignedApk = Join-Path $Stage 'aligned.apk'
Invoke-Native 'zipalign' {
  & $ZipAlign -f -p 4 $UnalignedApk $AlignedApk
}

# Sign the APK (uses ANDROID_KEYSTORE if configured, otherwise generates a local debug keystore in target/).
$Keystore = if ($env:ANDROID_KEYSTORE) { $env:ANDROID_KEYSTORE } else { Join-Path $Stage 'debug.keystore' }
$KeyAlias = if ($env:ANDROID_KEY_ALIAS) { $env:ANDROID_KEY_ALIAS } else { 'androiddebugkey' }
$KeyPass = if ($env:ANDROID_KEYSTORE_PASS) { $env:ANDROID_KEYSTORE_PASS } else { 'android' }
if (-not (Test-Path $Keystore)) {
  Invoke-Native 'keytool generate keystore' {
    & $Keytool -genkeypair -v -keystore $Keystore -storepass $KeyPass -alias $KeyAlias `
      -keypass $KeyPass -keyalg RSA -keysize 2048 -validity 10000 `
      -dname 'CN=VectorCraft Android,O=Storyteller,C=US'
  }
}

$FinalApk = Join-Path $Dist "vectorcraft-$Version-android-arm64.apk"
Invoke-Native 'apksigner sign' {
  & $ApkSigner sign --ks $Keystore --ks-key-alias $KeyAlias --ks-pass "pass:$KeyPass" --key-pass "pass:$KeyPass" --out $FinalApk $AlignedApk
}
Invoke-Native 'apksigner verify' {
  & $ApkSigner verify --verbose $FinalApk
}

$apkSizeMb = [Math]::Round((Get-Item $FinalApk).Length / 1MB, 2)
Write-Output "==> Built and signed APK: $FinalApk ($apkSizeMb MB)"

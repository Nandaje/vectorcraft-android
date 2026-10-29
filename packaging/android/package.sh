#!/usr/bin/env bash
# Build, package and sign VectorCraft for Android (arm64-v8a APK) on Linux / macOS.
#
# Produces, in ${DIST:-dist/release}:
#   vectorcraft-<version>-android-arm64.apk
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
VERSION="${VECTORCRAFT_VERSION:-$(awk '/^\[workspace\.package\]/{f=1;next} /^\[/{f=0} f && /^version[[:space:]]*=/{gsub(/.*"|"[^"]*$/,"");print;exit}' "$ROOT/Cargo.toml")}"
TARGET="aarch64-linux-android"
DIST="${DIST:-$ROOT/dist/release}"
TARGET_DIR="${CARGO_TARGET_DIR:-$ROOT/target}"
mkdir -p "$DIST"

SDK="${ANDROID_HOME:-${ANDROID_SDK_ROOT:-$HOME/Android/Sdk}}"
NDK="${ANDROID_NDK_HOME:-$(ls -d "$SDK/ndk/"* 2>/dev/null | sort -V | tail -n 1)}"
BUILD_TOOLS="$(ls -d "$SDK/build-tools/"* 2>/dev/null | sort -V | tail -n 1)"
PLATFORM_DIR="$(ls -d "$SDK/platforms/android-"* 2>/dev/null | sort -V | tail -n 1)"
ANDROID_JAR="$PLATFORM_DIR/android.jar"

HOST_TAG="linux-x86_64"
if [[ "$(uname -s)" == "Darwin" ]]; then
  HOST_TAG="darwin-x86_64"
fi
LLVM_BIN="$NDK/toolchains/llvm/prebuilt/$HOST_TAG/bin"

export ANDROID_NDK_HOME="$NDK"
export ANDROID_NDK_ROOT="$NDK"
export CC_aarch64_linux_android="$LLVM_BIN/aarch64-linux-android26-clang"
export CXX_aarch64_linux_android="$LLVM_BIN/aarch64-linux-android26-clang++"
export AR_aarch64_linux_android="$LLVM_BIN/llvm-ar"
export CARGO_TARGET_AARCH64_LINUX_ANDROID_LINKER="$LLVM_BIN/aarch64-linux-android26-clang"
export CARGO_TARGET_DIR="$TARGET_DIR"

APK_MANIFEST="$ROOT/packaging/android/apk/Cargo.toml"
APK_LOCK="$ROOT/packaging/android/apk/Cargo.lock"
cp "$ROOT/Cargo.lock" "$APK_LOCK"
trap 'rm -f "$APK_LOCK"' EXIT

cargo build --manifest-path "$APK_MANIFEST" --target "$TARGET" --release

SO_PATH="$TARGET_DIR/$TARGET/release/libvectorcraft.so"
STAGE="$TARGET_DIR/android-package"
rm -rf "$STAGE"
mkdir -p "$STAGE/res/values" "$STAGE/res/mipmap-anydpi-v26" "$STAGE/res/mipmap-mdpi" "$STAGE/res/mipmap-hdpi" "$STAGE/res/mipmap-xhdpi" "$STAGE/res/mipmap-xxhdpi" "$STAGE/res/mipmap-xxxhdpi" "$STAGE/gen" "$STAGE/classes" "$STAGE/dex" "$STAGE/apk/lib/arm64-v8a"

cat > "$STAGE/res/values/colors.xml" <<'EOF'
<?xml version="1.0" encoding="utf-8"?>
<resources>
    <color name="ic_launcher_background">#E8573F</color>
</resources>
EOF

cat > "$STAGE/res/mipmap-anydpi-v26/ic_launcher.xml" <<'EOF'
<?xml version="1.0" encoding="utf-8"?>
<adaptive-icon xmlns:android="http://schemas.android.com/apk/res/android">
    <background android:drawable="@color/ic_launcher_background" />
    <foreground android:drawable="@mipmap/ic_launcher_foreground" />
    <monochrome android:drawable="@mipmap/ic_launcher_monochrome" />
</adaptive-icon>
EOF
cp "$STAGE/res/mipmap-anydpi-v26/ic_launcher.xml" "$STAGE/res/mipmap-anydpi-v26/ic_launcher_round.xml"

python3 - "$ROOT/assets/app-icon/vectorcraft-1024.png" "$STAGE/res" <<'PY'
import math, os, sys
from PIL import Image, ImageDraw

src = Image.open(sys.argv[1]).convert("RGBA")
res_dir = sys.argv[2]
w, h = src.size
px = src.load()
fg_src = Image.new("RGBA", (w, h), (0, 0, 0, 0))
fg_px = fg_src.load()
for y in range(h):
    for x in range(w):
        r, g, b, a = px[x, y]
        if a < 16:
            continue
        dist = math.sqrt((r - 232.0) ** 2 + (g - 87.0) ** 2 + (b - 63.0) ** 2)
        if dist >= 55.0:
            fg_px[x, y] = (r, g, b, 255)
        elif dist >= 28.0:
            fg_px[x, y] = (r, g, b, int(min(255.0, max(0.0, (dist - 28.0) / 27.0 * a))))

cx, cy, r_inner = 776.0, 776.0, 212.0
for y in range(776, h):
    for x in range(776, w):
        dx, dy = x - cx, y - cy
        rad = math.sqrt(dx * dx + dy * dy)
        if rad > r_inner:
            r_sample = r_inner - 6.0 - ((rad - r_inner) % 44.0)
            sx = max(0, min(w - 1, int(round(cx + dx * (r_sample / rad)))))
            sy = max(0, min(h - 1, int(round(cy + dy * (r_sample / rad)))))
            fg_px[x, y] = fg_px[sx, sy]
for y in range(692, 776):
    for x in range(996, w):
        fg_px[x, y] = fg_px[994 - ((x - 996) % 32), y]
for y in range(996, h):
    for x in range(172, 776):
        fg_px[x, y] = fg_px[x, 994 - ((y - 996) % 32)]

master = Image.new("RGBA", (432, 432), (0, 0, 0, 0))
scaled = fg_src.resize((310, 310), Image.Resampling.LANCZOS)
master.paste(scaled, (54, 82), scaled)
m_px = master.load()
for y in range(292, 392):
    for x in range(362, 416):
        m_px[x, y] = m_px[361 - ((x - 362) % 16), y]
for y in range(390, 416):
    for x in range(104, 416):
        m_px[x, y] = m_px[x, 389 - ((y - 390) % 16)]

mono = Image.new("RGBA", (432, 432), (255, 255, 255, 0))
mo_px = mono.load()
for y in range(432):
    for x in range(432):
        r, g, b, a = m_px[x, y]
        if a >= 32:
            lum = (r * 299 + g * 587 + b * 114) // 1000
            mo_px[x, y] = (255, 255, 255, (a * lum) // 255)

crop_72 = master.crop((72, 72, 360, 360))
for density, fg_sz, leg_sz in [
    ("mipmap-mdpi", 108, 48),
    ("mipmap-hdpi", 162, 72),
    ("mipmap-xhdpi", 216, 96),
    ("mipmap-xxhdpi", 324, 144),
    ("mipmap-xxxhdpi", 432, 192),
]:
    d = os.path.join(res_dir, density)
    os.makedirs(d, exist_ok=True)
    master.resize((fg_sz, fg_sz), Image.Resampling.LANCZOS).save(os.path.join(d, "ic_launcher_foreground.png"))
    mono.resize((fg_sz, fg_sz), Image.Resampling.LANCZOS).save(os.path.join(d, "ic_launcher_monochrome.png"))
    for is_round, name in [(False, "ic_launcher.png"), (True, "ic_launcher_round.png")]:
        tile = Image.new("RGBA", (leg_sz, leg_sz), (232, 87, 63, 255))
        fg_leg = crop_72.resize((leg_sz, leg_sz), Image.Resampling.LANCZOS)
        tile.alpha_composite(fg_leg)
        mask = Image.new("L", (leg_sz, leg_sz), 0)
        draw = ImageDraw.Draw(mask)
        inset = max(1, int(round(leg_sz * (2.0 / 48.0))))
        if is_round:
            draw.ellipse((inset, inset, leg_sz - inset - 1, leg_sz - inset - 1), fill=255)
        else:
            rad = int(round((leg_sz - 2 * inset) * 0.22))
            draw.rounded_rectangle((inset, inset, leg_sz - inset - 1, leg_sz - inset - 1), radius=rad, fill=255)
        out = Image.new("RGBA", (leg_sz, leg_sz), (0, 0, 0, 0))
        out.paste(tile, (0, 0), mask)
        out.save(os.path.join(d, name))
PY

"$BUILD_TOOLS/aapt2" compile --dir "$STAGE/res" -o "$STAGE/res.zip"
"$BUILD_TOOLS/aapt2" link -o "$STAGE/base.apk" -I "$ANDROID_JAR" \
  --manifest "$ROOT/packaging/android/AndroidManifest.xml" \
  --java "$STAGE/gen" --version-name "$VERSION" --auto-add-overlay "$STAGE/res.zip"

javac --release 11 -encoding UTF-8 -cp "$ANDROID_JAR" -d "$STAGE/classes" \
  "$ROOT/packaging/android/java/ai/storyteller/vectorcraft/VectorCraftActivity.java" \
  "$STAGE/gen/ai/storyteller/vectorcraft/R.java"

"$BUILD_TOOLS/d8" --min-api 26 --lib "$ANDROID_JAR" --output "$STAGE/dex" \
  "$STAGE/classes/ai/storyteller/vectorcraft/"*.class

cp "$STAGE/base.apk" "$STAGE/unaligned.apk"
cp "$STAGE/dex/classes.dex" "$STAGE/apk/classes.dex"
cp "$SO_PATH" "$STAGE/apk/lib/arm64-v8a/libvectorcraft.so"
LIBCPP="$LLVM_BIN/../sysroot/usr/lib/aarch64-linux-android/libc++_shared.so"
if [[ -f "$LIBCPP" ]]; then
  cp "$LIBCPP" "$STAGE/apk/lib/arm64-v8a/libc++_shared.so"
fi
(cd "$STAGE/apk" && zip -q -r "$STAGE/unaligned.apk" classes.dex lib/arm64-v8a)

"$BUILD_TOOLS/zipalign" -f -p 4 "$STAGE/unaligned.apk" "$STAGE/aligned.apk"

KEYSTORE="${ANDROID_KEYSTORE:-$STAGE/debug.keystore}"
KEY_ALIAS="${ANDROID_KEY_ALIAS:-androiddebugkey}"
KEY_PASS="${ANDROID_KEYSTORE_PASS:-android}"
if [[ ! -f "$KEYSTORE" ]]; then
  keytool -genkeypair -v -keystore "$KEYSTORE" -storepass "$KEY_PASS" -alias "$KEY_ALIAS" \
    -keypass "$KEY_PASS" -keyalg RSA -keysize 2048 -validity 10000 \
    -dname "CN=VectorCraft Android,O=Storyteller,C=US"
fi

FINAL_APK="$DIST/vectorcraft-$VERSION-android-arm64.apk"
"$BUILD_TOOLS/apksigner" sign --ks "$KEYSTORE" --ks-key-alias "$KEY_ALIAS" \
  --ks-pass "pass:$KEY_PASS" --key-pass "pass:$KEY_PASS" --out "$FINAL_APK" "$STAGE/aligned.apk"
"$BUILD_TOOLS/apksigner" verify --verbose "$FINAL_APK"
echo "==> Built and signed APK: $FINAL_APK"

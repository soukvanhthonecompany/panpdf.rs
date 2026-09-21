#!/usr/bin/env bash
#
# Build the shippable packages.
#
#   packaging/build.sh                 everything this machine can make
#   packaging/build.sh deb windows     only these
#   packaging/build.sh --list          what each target needs, and whether
#                                      this machine has it
#
# Targets:
#
#   tarball    panpdf-VERSION-linux-x86_64.tar.gz    a directory you unpack
#   deb        panpdf_VERSION_amd64.deb              Debian, Ubuntu, Mint
#   rpm        panpdf-VERSION.x86_64.rpm             Fedora, openSUSE, RHEL
#   appimage   PanPDF-VERSION-x86_64.AppImage        one file, any Linux
#   windows    panpdf-VERSION-windows-x86_64.zip     panpdf.exe, cross-built
#              panpdf-VERSION-windows-x86_64-setup.exe  the same, as an installer
#   macos      panpdf-VERSION-macos.tar.gz           PanPDF.app; macOS only
#
# Everything lands in `dist/`, with `dist/SHA256SUMS` beside it.
#
# A target whose tools are missing is skipped with a line saying which tool and
# how to get it; it is not an error, because no one machine makes all six. The
# release workflow in `.github/workflows/` runs the three hosts that do.
#
# The program looks for its substitution faces in a `fonts/` directory beside
# the executable, so every package carries `fonts/manifest.json` and the faces
# `fonts/vendor.py` unpacked. Without them the program still runs and falls
# back to the host's fonts; a document that names a font nobody has will draw
# differently on two machines.

set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
dist="$root/dist"
version="$(grep -m1 '^version = ' "$root/Cargo.toml" | cut -d'"' -f2)"
[[ -n "$version" ]] || { echo "no version in Cargo.toml" >&2; exit 1; }

say() { printf '  %s\n' "$*"; }
skip() { printf '\n== %s: skipped\n  %s\n' "$1" "$2"; }
have() { command -v "$1" >/dev/null 2>&1; }

# ---------------------------------------------------------------------------
# Pieces every package needs.
# ---------------------------------------------------------------------------

# The two binaries, for one target triple. An empty triple means this machine.
compile() {
    local triple="${1:-}"
    if [[ -n "$triple" ]]; then
        ( cd "$root" && cargo build --release --target "$triple" -p pdf-desktop -p pdf-cli )
    else
        ( cd "$root" && cargo build --release -p pdf-desktop -p pdf-cli )
    fi
}

# Where `compile` put them. The desktop crate's binary is still called
# `pdf-app`, because every probe and script in this tree names that file; it is
# installed as `panpdf` and only the installed name is a promise.
built() {
    local triple="${1:-}"
    if [[ -n "$triple" ]]; then printf '%s/target/%s/release' "$root" "$triple"
    else printf '%s/target/release' "$root"; fi
}

# The faces, into a payload directory. Absent is a warning, not a failure.
stage_fonts() {
    local into="$1"
    if [[ ! -f "$root/fonts/packaged/DejaVuSans.ttf" ]]; then
        say "no fonts/packaged -- run: python3 fonts/vendor.py"
        return 0
    fi
    mkdir -p "$into/fonts"
    cp "$root/fonts/manifest.json" "$into/fonts/"
    cp -r "$root/fonts/packaged" "$into/fonts/"
    say "fonts: $(ls "$into/fonts/packaged" | wc -l) files"
}

# The icon at one size, as PNG. Prints nothing and returns 1 if this machine
# cannot rasterise SVG.
icon_png() {
    local size="$1" out="$2" svg="$root/packaging/panpdf.svg"
    # Order matters. ImageMagick has no SVG renderer of its own worth the name:
    # asked for this logo it produces a nearly transparent dark smudge, which is
    # how a black square reached a real Windows taskbar once. It stays last.
    if have rsvg-convert; then rsvg-convert -w "$size" -h "$size" -o "$out" "$svg"
    elif have inkscape; then inkscape "$svg" -w "$size" -h "$size" -o "$out" >/dev/null 2>&1
    elif have magick; then magick -background none "$svg" -resize "${size}x${size}" "$out"
    elif have convert; then convert -background none "$svg" -resize "${size}x${size}" "$out"
    else return 1
    fi
}

desktop_entry() {
    cat <<'ENTRY'
[Desktop Entry]
Type=Application
Name=PanPDF
GenericName=PDF Editor
Comment=Read and edit PDF documents
Exec=panpdf %f
TryExec=panpdf
Icon=panpdf
Terminal=false
Categories=Office;Graphics;Viewer;
MimeType=application/pdf;
StartupWMClass=panpdf
ENTRY
}

# The icon as a multi-size Windows .ico. `packaging/panpdf.ico` is committed,
# because a Windows or macOS build machine has no SVG rasteriser and the crate's
# build script must find it; this rebuilds it from the SVG when the SVG changes.
make_ico() {
    local out="$root/packaging/panpdf.ico"
    have rsvg-convert || have magick || have convert || have inkscape \
        || { say "no SVG rasteriser: keeping the committed packaging/panpdf.ico"; return 0; }
    local work size pngs=()
    work="$(mktemp -d)"
    for size in 16 24 32 48 64 128 256; do
        icon_png "$size" "$work/$size.png" || { rm -rf "$work"; return 0; }
        pngs+=("$work/$size.png")
    done
    python3 "$root/packaging/make_icon.py" "$out" "${pngs[@]}" >/dev/null
    rm -rf "$work"
    say "icon: $(basename "$out")"
}

# The installer: a Win32 wizard of our own, in `packaging/installer/`, built
# with the same mingw toolchain that cross-compiles the program, carrying the
# staged payload appended to it. Nothing else is needed and nothing is fetched.
#
# Before it is built, the decompressor inside it is compiled for this machine
# and run against known answers, because an installer that cannot unpack what it
# carries is worse than no installer.
build_installer() {
    local stage="$1" out="$2" src="$root/packaging/installer" work
    have x86_64-w64-mingw32-gcc || { say "no mingw gcc: no installer"; return 0; }
    have python3 || { say "no python3: no installer"; return 0; }

    work="$(mktemp -d)"
    python3 "$src/pack.py" --self-test "$work/cases.bin" >/dev/null
    if have cc || have gcc; then
        "$(command -v cc || command -v gcc)" -O2 -o "$work/inflate_test" \
            "$src/inflate_test.c" "$src/inflate.c" -I"$src"
        "$work/inflate_test" "$work/cases.bin" || { rm -rf "$work"; echo "the installer's decompressor is wrong" >&2; exit 1; }
    else
        say "no host C compiler: the decompressor ships unproven"
    fi

    cat > "$work/setup.rc" <<RC
1 ICON "$root/packaging/panpdf.ico"
1 24 "$src/installer.manifest"
1 VERSIONINFO
FILEVERSION ${version//./,},0
PRODUCTVERSION ${version//./,},0
FILEOS 0x4
FILETYPE 0x1
BEGIN
  BLOCK "StringFileInfo"
  BEGIN
    BLOCK "040904b0"
    BEGIN
      VALUE "CompanyName", "Soukvanhthone Company"
      VALUE "FileDescription", "PanPDF Setup"
      VALUE "FileVersion", "$version"
      VALUE "InternalName", "panpdf-setup"
      VALUE "LegalCopyright", "AGPL-3.0-only"
      VALUE "OriginalFilename", "$(basename "$out")"
      VALUE "ProductName", "PanPDF"
      VALUE "ProductVersion", "$version"
    END
  END
  BLOCK "VarFileInfo"
  BEGIN
    VALUE "Translation", 0x409, 1200
  END
END
RC
    x86_64-w64-mingw32-windres -O coff "$work/setup.rc" "$work/setup.res.o"

    local flags=(-O2 -Wall -Wextra -municode -mwindows -I"$src")
    local libs=(-lcomctl32 -lole32 -luuid -lshell32 -lshlwapi -ladvapi32 -lgdi32)
    x86_64-w64-mingw32-gcc "${flags[@]}" -o "$stage/uninstall.exe" \
        "$src/uninstall.c" "$src/records.c" "$work/setup.res.o" "${libs[@]}"
    x86_64-w64-mingw32-gcc "${flags[@]}" "-DPANPDF_VERSION=L\"$version\"" -o "$work/stub.exe" \
        "$src/installer.c" "$src/inflate.c" "$src/records.c" "$work/setup.res.o" "${libs[@]}"
    have x86_64-w64-mingw32-strip && x86_64-w64-mingw32-strip "$stage/uninstall.exe" "$work/stub.exe"

    python3 "$src/pack.py" archive "$stage" "$work/payload.bin"
    python3 "$src/pack.py" append "$work/stub.exe" "$work/payload.bin" "$out"
    python3 "$src/pack.py" list "$out" | tail -1
    rm -rf "$work"
}

# ---------------------------------------------------------------------------
# Linux: a directory you unpack anywhere.
# ---------------------------------------------------------------------------

make_tarball() {
    printf '\n== tarball\n'
    compile
    local name="panpdf-$version-linux-x86_64"
    local stage="$dist/stage/$name"
    rm -rf "$stage"; mkdir -p "$stage"
    cp "$(built)/pdf-app" "$stage/panpdf"
    cp "$(built)/pdf-cli" "$stage/panpdf-cli"
    strip "$stage/panpdf" "$stage/panpdf-cli" 2>/dev/null || true
    cp "$root/LICENSE" "$root/README.md" "$stage/"
    desktop_entry > "$stage/panpdf.desktop"
    icon_png 256 "$stage/panpdf.png" || say "no SVG rasteriser; packaged without an icon"
    # The scalable icon travels inside the tarball, because the RPM is built
    # from the tarball and nothing else: a spec that reached back out into the
    # source tree got the path wrong, and no one found out until a release
    # runner tried it (run 35582914417).
    cp "$root/packaging/panpdf.svg" "$stage/panpdf.svg"
    stage_fonts "$stage"
    find "$stage" -type d -exec chmod 755 {} +
    find "$stage" -type f -exec chmod 644 {} +
    chmod 755 "$stage/panpdf" "$stage/panpdf-cli"
    ( cd "$dist/stage" && tar czf "$dist/$name.tar.gz" --owner=0 --group=0 "$name" )
    say "dist/$name.tar.gz"
}

# ---------------------------------------------------------------------------
# Debian and its descendants.
#
# The binary goes to /usr/lib/panpdf so that the `fonts/` it looks for beside
# itself can live there too; /usr/bin/panpdf is a symlink, and Linux resolves
# /proc/self/exe through it, so the lookup still lands in /usr/lib/panpdf.
#
# Nothing but libc is linked: the window opens X11, Wayland and GL at run time
# through dlopen, which is why those are Depends rather than shared-library
# dependencies dpkg-shlibdeps could find on its own.
# ---------------------------------------------------------------------------

make_deb() {
    printf '\n== deb\n'
    have dpkg-deb || { skip deb "dpkg-deb is missing -- apt install dpkg-dev"; return 0; }
    compile
    local stage="$dist/stage/deb"
    rm -rf "$stage"
    mkdir -p "$stage/DEBIAN" "$stage/usr/lib/panpdf" "$stage/usr/bin" \
             "$stage/usr/share/applications" "$stage/usr/share/doc/panpdf" \
             "$stage/usr/share/icons/hicolor/scalable/apps"
    cp "$(built)/pdf-app" "$stage/usr/lib/panpdf/panpdf"
    cp "$(built)/pdf-cli" "$stage/usr/lib/panpdf/panpdf-cli"
    strip "$stage/usr/lib/panpdf/panpdf" "$stage/usr/lib/panpdf/panpdf-cli" 2>/dev/null || true
    ln -s ../lib/panpdf/panpdf "$stage/usr/bin/panpdf"
    ln -s ../lib/panpdf/panpdf-cli "$stage/usr/bin/panpdf-cli"
    stage_fonts "$stage/usr/lib/panpdf"
    desktop_entry > "$stage/usr/share/applications/panpdf.desktop"
    cp "$root/packaging/panpdf.svg" "$stage/usr/share/icons/hicolor/scalable/apps/panpdf.svg"
    for size in 48 64 128 256; do
        mkdir -p "$stage/usr/share/icons/hicolor/${size}x${size}/apps"
        icon_png "$size" "$stage/usr/share/icons/hicolor/${size}x${size}/apps/panpdf.png" \
            || { rmdir "$stage/usr/share/icons/hicolor/${size}x${size}/apps" \
                       "$stage/usr/share/icons/hicolor/${size}x${size}" 2>/dev/null; true; }
    done
    cp "$root/LICENSE" "$stage/usr/share/doc/panpdf/copyright"
    find "$stage" -type d -exec chmod 755 {} +
    find "$stage" -type f -exec chmod 644 {} +
    chmod 755 "$stage/usr/lib/panpdf/panpdf" "$stage/usr/lib/panpdf/panpdf-cli"
    gzip -9n -c "$root/README.md" > "$stage/usr/share/doc/panpdf/README.md.gz"
    cat > "$stage/DEBIAN/control" <<CONTROL
Package: panpdf
Version: $version
Architecture: amd64
Maintainer: Soukvanhthone Company <panpdf@users.noreply.github.com>
Section: graphics
Priority: optional
Homepage: https://github.com/soukvanhthonecompany/panpdf.rs
Depends: libc6 (>= 2.34), libgcc-s1, libgl1, libx11-6, libxkbcommon0, libxcursor1, libxi6, libxrandr2, libwayland-client0
Recommends: fonts-noto, zenity
Suggests: tesseract-ocr, cups-client
Description: PDF editor that edits the file
 PanPDF reads the instructions a PDF already contains, keeps every value
 attached to the bytes it came from, and writes an edit back as a change to
 those instructions rather than rebuilding the page from a model of its own.
 What it cannot do exactly it refuses by name instead of approximating.
 .
 The parser, filters, encryption, font programs, shaper, rasteriser and image
 decoders are all its own; no PDF library is linked.
CONTROL
    local out="$dist/panpdf_${version}_amd64.deb"
    dpkg-deb --root-owner-group --build "$stage" "$out" >/dev/null
    say "$(basename "$out")"
    have lintian && lintian --no-tag-display-limit "$out" 2>&1 | head -20 || true
}

# ---------------------------------------------------------------------------
# Fedora, openSUSE, RHEL. Same layout, rpmbuild's own tree.
# ---------------------------------------------------------------------------

make_rpm() {
    printf '\n== rpm\n'
    have rpmbuild || { skip rpm "rpmbuild is missing -- dnf install rpm-build"; return 0; }
    local tar="$dist/panpdf-$version-linux-x86_64.tar.gz"
    [[ -f "$tar" ]] || make_tarball
    local top="$dist/stage/rpm"
    rm -rf "$top"; mkdir -p "$top"/{BUILD,RPMS,SOURCES,SPECS}
    cp "$tar" "$top/SOURCES/"
    cat > "$top/SPECS/panpdf.spec" <<SPEC
Name:           panpdf
Version:        $version
Release:        1
Summary:        PDF editor that edits the file
License:        AGPL-3.0-or-later
URL:            https://github.com/soukvanhthonecompany/panpdf.rs
Source0:        panpdf-%{version}-linux-x86_64.tar.gz
Requires:       mesa-libGL libX11 libxkbcommon libXcursor libXi libXrandr
Recommends:     google-noto-sans-fonts zenity
Suggests:       tesseract
%global debug_package %{nil}

%description
PanPDF reads the instructions a PDF already contains, keeps every value
attached to the bytes it came from, and writes an edit back as a change to
those instructions rather than rebuilding the page from a model of its own.
The parser, filters, encryption, font programs, shaper, rasteriser and image
decoders are all its own; no PDF library is linked.

%prep
%setup -q -n panpdf-%{version}-linux-x86_64

%install
mkdir -p %{buildroot}/usr/lib/panpdf %{buildroot}/usr/bin
mkdir -p %{buildroot}/usr/share/applications
mkdir -p %{buildroot}/usr/share/icons/hicolor/scalable/apps
cp panpdf panpdf-cli %{buildroot}/usr/lib/panpdf/
[ -d fonts ] && cp -r fonts %{buildroot}/usr/lib/panpdf/ || :
ln -s ../lib/panpdf/panpdf %{buildroot}/usr/bin/panpdf
ln -s ../lib/panpdf/panpdf-cli %{buildroot}/usr/bin/panpdf-cli
install -m644 panpdf.desktop %{buildroot}/usr/share/applications/panpdf.desktop
install -m644 panpdf.svg %{buildroot}/usr/share/icons/hicolor/scalable/apps/panpdf.svg

%files
/usr/lib/panpdf
/usr/bin/panpdf
/usr/bin/panpdf-cli
/usr/share/applications/panpdf.desktop
/usr/share/icons/hicolor/scalable/apps/panpdf.svg
%license LICENSE
SPEC
    rpmbuild --define "_topdir $top" -bb "$top/SPECS/panpdf.spec" >/dev/null
    find "$top/RPMS" -name '*.rpm' -exec cp {} "$dist/" \;
    say "$(cd "$dist" && ls panpdf-$version*.rpm 2>/dev/null | tr '\n' ' ')"
}

# ---------------------------------------------------------------------------
# One file that runs on any Linux old enough to have FUSE.
# ---------------------------------------------------------------------------

make_appimage() {
    printf '\n== appimage\n'
    local tool
    tool="$(command -v appimagetool || command -v appimagetool-x86_64.AppImage || true)"
    [[ -n "$tool" ]] || { skip appimage "appimagetool is missing -- https://github.com/AppImage/appimagetool/releases"; return 0; }
    compile
    local app="$dist/stage/PanPDF.AppDir"
    rm -rf "$app"; mkdir -p "$app/usr/bin"
    cp "$(built)/pdf-app" "$app/usr/bin/panpdf"
    cp "$(built)/pdf-cli" "$app/usr/bin/panpdf-cli"
    strip "$app/usr/bin/panpdf" "$app/usr/bin/panpdf-cli" 2>/dev/null || true
    stage_fonts "$app/usr/bin"
    desktop_entry > "$app/panpdf.desktop"
    cp "$root/packaging/panpdf.svg" "$app/panpdf.svg"
    icon_png 256 "$app/panpdf.png" || true
    cat > "$app/AppRun" <<'APPRUN'
#!/bin/sh
here="$(dirname "$(readlink -f "$0")")"
exec "$here/usr/bin/panpdf" "$@"
APPRUN
    chmod +x "$app/AppRun"
    local out="$dist/PanPDF-$version-x86_64.AppImage"
    ARCH=x86_64 "$tool" "$app" "$out" >/dev/null 2>&1
    say "$(basename "$out")"
}

# ---------------------------------------------------------------------------
# Windows, cross-built from Linux with the GNU toolchain, or native on Windows.
# ---------------------------------------------------------------------------

make_windows() {
    printf '\n== windows\n'
    local triple=x86_64-pc-windows-gnu
    if [[ "${OS:-}" == Windows_NT ]]; then
        triple=""
    else
        rustup target list --installed 2>/dev/null | grep -qx "$triple" \
            || { skip windows "rustup target add $triple"; return 0; }
        have x86_64-w64-mingw32-gcc \
            || { skip windows "the mingw-w64 linker is missing -- apt install gcc-mingw-w64-x86-64"; return 0; }
    fi
    compile "$triple"
    local name="panpdf-$version-windows-x86_64"
    local stage="$dist/stage/$name"
    rm -rf "$stage"; mkdir -p "$stage"
    cp "$(built "$triple")/pdf-app.exe" "$stage/panpdf.exe"
    cp "$(built "$triple")/pdf-cli.exe" "$stage/panpdf-cli.exe"
    if have x86_64-w64-mingw32-strip; then
        x86_64-w64-mingw32-strip "$stage/panpdf.exe" "$stage/panpdf-cli.exe" || true
    elif [[ "${OS:-}" == Windows_NT ]] && have strip; then
        strip "$stage/panpdf.exe" "$stage/panpdf-cli.exe" || true
    fi
    cp "$root/LICENSE" "$stage/LICENSE.txt"
    cp "$root/README.md" "$stage/README.md"
    cp "$root/packaging/panpdf.svg" "$stage/panpdf.svg"
    stage_fonts "$stage"
    if have zip; then ( cd "$dist/stage" && zip -qr "$dist/$name.zip" "$name" )
    else ( cd "$dist/stage" && tar czf "$dist/$name.tar.gz" "$name" ); fi
    # The installer is built from the same staged directory, after the archive,
    # so the two carry exactly the same files -- plus `uninstall.exe`, which
    # only the installer needs.
    build_installer "$stage" "$dist/$name-setup.exe"
    say "$(cd "$dist" && ls "$name".* "$name-setup.exe" 2>/dev/null | tr '\n' ' ')"
}

# ---------------------------------------------------------------------------
# macOS. Only on macOS: Apple's SDK is not redistributable, so no other host
# can link against it. Both architectures when both targets are installed.
# ---------------------------------------------------------------------------

make_macos() {
    printf '\n== macos\n'
    [[ "$(uname -s)" == Darwin ]] || { skip macos "this is not macOS -- the release workflow builds it on a macOS runner"; return 0; }
    local arches=() triple
    for triple in aarch64-apple-darwin x86_64-apple-darwin; do
        rustup target list --installed 2>/dev/null | grep -qx "$triple" && arches+=("$triple")
    done
    [[ ${#arches[@]} -gt 0 ]] || { skip macos "no apple-darwin target installed"; return 0; }
    for triple in "${arches[@]}"; do compile "$triple"; done
    local app="$dist/stage/PanPDF.app"
    rm -rf "$app"; mkdir -p "$app/Contents/MacOS" "$app/Contents/Resources"
    local binary
    for binary in pdf-app:panpdf pdf-cli:panpdf-cli; do
        local from="${binary%%:*}" to="${binary##*:}" parts=()
        for triple in "${arches[@]}"; do parts+=("$(built "$triple")/$from"); done
        if [[ ${#parts[@]} -gt 1 ]]; then lipo -create -output "$app/Contents/MacOS/$to" "${parts[@]}"
        else cp "${parts[0]}" "$app/Contents/MacOS/$to"; fi
    done
    stage_fonts "$app/Contents/MacOS"
    cp "$root/LICENSE" "$app/Contents/Resources/LICENSE"
    if have rsvg-convert || have magick || have convert || have inkscape; then
        local set="$dist/stage/panpdf.iconset"
        rm -rf "$set"; mkdir -p "$set"
        local size
        for size in 16 32 64 128 256 512; do
            icon_png "$size" "$set/icon_${size}x${size}.png" || true
            icon_png "$((size * 2))" "$set/icon_${size}x${size}@2x.png" || true
        done
        have iconutil && iconutil -c icns "$set" -o "$app/Contents/Resources/panpdf.icns" || true
    fi
    cat > "$app/Contents/Info.plist" <<PLIST
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>CFBundleName</key><string>PanPDF</string>
  <key>CFBundleDisplayName</key><string>PanPDF</string>
  <key>CFBundleIdentifier</key><string>com.soukvanhthone.panpdf</string>
  <key>CFBundleVersion</key><string>$version</string>
  <key>CFBundleShortVersionString</key><string>$version</string>
  <key>CFBundleExecutable</key><string>panpdf</string>
  <key>CFBundleIconFile</key><string>panpdf</string>
  <key>CFBundlePackageType</key><string>APPL</string>
  <key>LSMinimumSystemVersion</key><string>11.0</string>
  <key>NSHighResolutionCapable</key><true/>
  <key>CFBundleDocumentTypes</key>
  <array><dict>
    <key>CFBundleTypeName</key><string>PDF document</string>
    <key>CFBundleTypeRole</key><string>Editor</string>
    <key>LSItemContentTypes</key><array><string>com.adobe.pdf</string></array>
  </dict></array>
</dict>
</plist>
PLIST
    local name="panpdf-$version-macos"
    ( cd "$dist/stage" && tar czf "$dist/$name.tar.gz" PanPDF.app )
    say "$name.tar.gz"
    if have hdiutil; then
        rm -f "$dist/$name.dmg"
        hdiutil create -quiet -volname PanPDF -srcfolder "$app" -ov -format UDZO "$dist/$name.dmg"
        say "$name.dmg"
    fi
}

# ---------------------------------------------------------------------------

list_targets() {
    printf 'panpdf %s\n\n' "$version"
    local line
    for line in \
        "tarball:cargo:always" \
        "deb:dpkg-deb:apt install dpkg-dev" \
        "rpm:rpmbuild:dnf install rpm-build" \
        "appimage:appimagetool:github.com/AppImage/appimagetool/releases" \
        "windows:x86_64-w64-mingw32-gcc:apt install gcc-mingw-w64-x86-64, rustup target add x86_64-pc-windows-gnu" \
        "macos:iconutil:macOS only"
    do
        local target="${line%%:*}" rest="${line#*:}"
        local tool="${rest%%:*}" how="${rest#*:}"
        if have "$tool"; then printf '  %-9s yes\n' "$target"
        else printf '  %-9s no    %s\n' "$target" "$how"; fi
    done
    if ! (have rsvg-convert || have magick || have convert || have inkscape); then
        printf '\n  no SVG rasteriser: packages will have no PNG icon\n'
        printf '  apt install librsvg2-bin\n'
    fi
    [[ -f "$root/fonts/packaged/DejaVuSans.ttf" ]] \
        || printf '\n  no fonts/packaged: run python3 fonts/vendor.py first\n'
}

targets=("$@")
if [[ ${#targets[@]} -eq 1 && ( "${targets[0]}" == "--list" || "${targets[0]}" == "-l" ) ]]; then
    list_targets
    exit 0
fi
if [[ ${#targets[@]} -eq 0 ]]; then
    if [[ "$(uname -s)" == Darwin ]]; then targets=(macos)
    else targets=(tarball deb rpm appimage windows); fi
fi

mkdir -p "$dist"
printf '\n== icon\n'
make_ico
for target in "${targets[@]}"; do
    case "$target" in
        tarball|deb|rpm|appimage|windows|macos) "make_$target" ;;
        *) echo "unknown target: $target" >&2; exit 1 ;;
    esac
done

rm -rf "$dist/stage"

# `find -printf`, `xargs -r` and `sha256sum` are all GNU; macOS has none of
# them, and this is the last line of a macOS release, so the packages were
# built and then thrown away (run 35582914417). Whichever hashing program this
# machine has, read the names one line at a time so that a name with a space in
# it stays one name, and let `SHA256SUMS` exist before `find` runs so that
# `find` excludes it instead of hashing it.
if command -v sha256sum >/dev/null 2>&1; then
    hash_them() { sha256sum "$@"; }
else
    hash_them() { shasum -a 256 "$@"; }
fi
( cd "$dist" && find . -maxdepth 1 -type f -not -name SHA256SUMS \
    | sed 's|^\./||' | sort \
    | while IFS= read -r file; do hash_them "$file"; done > SHA256SUMS )

printf '\n== dist\n'
sed 's/^/  /' "$dist/SHA256SUMS"

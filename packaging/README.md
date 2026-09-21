# Packaging

```sh
packaging/build.sh --list        # what this machine can build, and what is missing
packaging/build.sh               # everything it can
packaging/build.sh deb windows   # or just these
```

Everything lands in `dist/`, with `dist/SHA256SUMS` beside it.

| Target | File | Needs |
| --- | --- | --- |
| `tarball` | `panpdf-VERSION-linux-x86_64.tar.gz` | cargo |
| `deb` | `panpdf_VERSION_amd64.deb` | `dpkg-deb` |
| `rpm` | `panpdf-VERSION.x86_64.rpm` | `rpmbuild` |
| `appimage` | `PanPDF-VERSION-x86_64.AppImage` | `appimagetool` |
| `windows` | `panpdf-VERSION-windows-x86_64.zip` and `-setup.exe` | mingw-w64, the `x86_64-pc-windows-gnu` target |
| `macos` | `panpdf-VERSION-macos.tar.gz`, `.dmg` | macOS |

A target whose tools are not installed is skipped with a line saying which tool
and where to get it. No one machine makes all six: Apple's SDK is not
redistributable, so nothing but macOS can link a macOS binary, and the Debian
and Fedora tools are not on macOS. `release.yml` runs the two hosts that
between them cover everything, and is exported as
`.github/workflows/release.yml`.

On Debian or Ubuntu, one line gets you five of the six:

```sh
sudo apt install dpkg-dev rpm librsvg2-bin gcc-mingw-w64-x86-64 zip
rustup target add x86_64-pc-windows-gnu
```

## What goes in a package

The two binaries, the licence, and `fonts/`.

The desktop binary is called `pdf-app` in `target/release`, because every
script in this tree names that file. It is installed as `panpdf`, and that is
the name a package promises.

`fonts/` matters: the program looks for `fonts/manifest.json` beside the
executable, one directory above it, or in the working directory, and draws a
font the document names and nobody has from the faces listed there. Without it
the program still runs and falls back to the host's fonts, and the same
document then draws differently on two machines. Run `python3 fonts/vendor.py`
before building packages; `build.sh` says so and carries on if you did not.

In the `.deb` and the `.rpm` the program is installed at
`/usr/lib/panpdf/panpdf` so that `fonts/` can sit beside it, and `/usr/bin/panpdf`
is a symlink. Linux resolves `/proc/self/exe` through the symlink, so the
lookup still lands in `/usr/lib/panpdf`.

## Dependencies a package declares

Only libc is linked. The window opens X11, Wayland and OpenGL at run time
through `dlopen`, which is why they are declared by hand rather than found by
`dpkg-shlibdeps`: `libgl1`, `libx11-6`, `libxkbcommon0`, `libxcursor1`,
`libxi6`, `libxrandr2`, `libwayland-client0`.

Recommended, not required: `zenity` for the system file dialog -- there is a
built-in one if it is absent. Suggested: `tesseract-ocr` to read scanned
pages, `cups-client` to print.

## The icon

`panpdf.svg` is the source. `build.sh` rasterises it with whichever of
`rsvg-convert`, `inkscape`, `magick` or `convert` is present, and packages
without a PNG icon if none is -- the SVG is installed either way, and that is
what a modern desktop prefers.

That order is not alphabetical. Asked for this logo, ImageMagick's own SVG
reader returns a nearly transparent dark smudge rather than an error, and a
black square reached a real Windows taskbar that way once. `make_icon.py`
refuses an image that is less than a twentieth opaque, so it cannot happen
again silently.

`packaging/panpdf.ico` is committed, because the crate's build script needs it
on a machine with no rasteriser at all. `build.sh` rewrites it from the SVG
whenever it can, and `make_icon.py` builds it with nothing but Python's
standard library: sixteen to sixty-four pixels as DIB entries, and the two
large ones as the PNGs themselves.

## What the Windows executable carries

`crates/pdf-desktop/build.rs` writes a resource script into `OUT_DIR` and
compiles it with `windres`, which the mingw toolchain already provides, so the
`.exe` carries its icon, its version information -- taken from the workspace
version, never typed twice -- and a manifest. The manifest says what is true:
`asInvoker` (this program never wants an administrator), per-monitor v2 DPI
awareness, UTF-8 as the active code page, and Windows 10 and 11 as the versions
anyone has run it on. None of this happens for a Linux or macOS build: the
build script returns before reading a file if the target is not Windows, and a
missing `windres` is a warning, not a failure.

`crates/pdf-cli/build.rs` is three lines that call the same `emit` with its own
description and filename, so both executables in a package carry the same three
resources and cannot drift apart in what they say they are.

## The Windows installer

`packaging/installer/` is a Win32 installer of our own, roughly a thousand
lines of C, cross-compiled by the same mingw toolchain. Nothing has to be
installed to build it that building the program did not already need, and
nothing is fetched.

- `installer.c` is the wizard: a welcome, the folder (with what the install
  needs and what that drive has free), a progress bar naming each file, then
  shortcuts, the Add/Remove Programs entry and a closing page.
- `uninstall.c` is shipped inside the payload and removes exactly the paths
  `installed-files.txt` records, then the registry key, then itself.
- `records.c` reads and undoes an `installed-files.txt`, and is shared because
  the installer and the uninstaller must agree, to the file, on what the
  installed version put there. When they disagree an update either leaves junk
  behind or deletes something it did not install.
- `inflate.c` is RFC 1951 decoding, so the payload can be deflate-compressed
  with Python's `zlib` and still need no library on either side. `build.sh`
  compiles `inflate_test.c` for the build machine and runs it against the
  known answers `pack.py --self-test` writes -- including corrupt input that
  must be refused -- and refuses to build an installer if any of them is wrong.
- `pack.py` packs the staged directory, appends it to the stub and can read a
  finished installer back. Its comment is the only description of the format.

It installs for one person: files under `%LOCALAPPDATA%\Programs\PanPDF` by
default and anywhere else the person types or browses to, shortcuts in that
person's own Start Menu and desktop, and one key under `HKEY_CURRENT_USER`. So
there is no elevation prompt at any point, which matches the program's own
`asInvoker` manifest. **It writes nothing to `%APPDATA%`**, and so does nothing
at all for the empty "Recent" list, which is `DEBT-51`.

**Installing over an existing installation is an update, not a second copy.**
The entry under `HKEY_CURRENT_USER` says which version is installed and where,
so the installer offers that folder rather than the default, says whether this
is an update, a reinstall or a step back to an older version, and deletes every
path the old version recorded before writing the new ones -- so a file that
stopped shipping does not live forever, while a file the person put in that
folder is not recorded, not deleted, and keeps the folder alive after an
uninstall. Downgrading is allowed and says so, because after a bad update it is
the right answer. If any of our executables is open, Windows will not let it be
replaced, so the installer says so and asks rather than half-writing over it.

The `.zip` is still built and still works for anyone who wants no installer.
What the installer cannot do -- a silent install, an all-users install, an MSI,
a file association -- is `DEBT-58`.

## Not made here

A signing certificate. The `.exe`, the installer and the `.app` are all
unsigned, and `DEBT-46` records what Windows shows a person because of it --
measured, not guessed.

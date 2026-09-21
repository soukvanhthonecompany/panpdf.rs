#!/usr/bin/env python3
"""Downloads the packaged substitution faces and verifies every byte.

`docs/design/RENDER-REPAIR-BATCH.md` F2 asks for a font set that reproduces on
a machine with no fonts installed, named by version and hash rather than by
whatever the developer happened to have. This is that procedure.

It is **explicit**: nothing here runs when a user opens a PDF. The renderer
reads `fonts/packaged/` if it is there and reports that it is not if it is not.

    python3 fonts/vendor.py            # download, verify, unpack
    python3 fonts/vendor.py --check    # verify what is already unpacked
    python3 fonts/vendor.py --print-hashes   # recompute, for a version bump

Every archive is checked against the SHA-256 in `MANIFEST` before it is opened,
and every extracted face is checked against its own SHA-256 afterwards. A
mismatch stops the run: a face whose bytes are not the ones this project
measured is not the face this project measured.
"""

from __future__ import annotations

import argparse
import hashlib
import io
import json
import pathlib
import sys
import tarfile
import urllib.request
import zipfile

ROOT = pathlib.Path(__file__).resolve().parent
PACKAGED = ROOT / "packaged"
MANIFEST_PATH = ROOT / "manifest.json"


def sha256(data: bytes) -> str:
    return hashlib.sha256(data).hexdigest()


def fetch(url: str) -> bytes:
    print(f"  fetching {url}", file=sys.stderr)
    request = urllib.request.Request(url, headers={"User-Agent": "panpdf-vendor"})
    with urllib.request.urlopen(request, timeout=300) as response:
        return response.read()


def members(archive: str, data: bytes) -> dict[str, bytes]:
    """Every regular file in the archive, keyed by its path inside it."""
    out: dict[str, bytes] = {}
    if archive.endswith((".tar.gz", ".tgz", ".tar.bz2")):
        mode = "r:bz2" if archive.endswith(".tar.bz2") else "r:gz"
        with tarfile.open(fileobj=io.BytesIO(data), mode=mode) as tar:
            for member in tar.getmembers():
                if not member.isfile():
                    continue
                handle = tar.extractfile(member)
                if handle is not None:
                    out[member.name] = handle.read()
    elif archive.endswith(".zip"):
        with zipfile.ZipFile(io.BytesIO(data)) as zip_file:
            for name in zip_file.namelist():
                if name.endswith("/"):
                    continue
                out[name] = zip_file.read(name)
    elif archive.endswith((".otf", ".ttf")):
        out[archive] = data
    else:
        raise SystemExit(f"unknown archive kind: {archive}")
    return out


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--check", action="store_true", help="verify what is unpacked")
    parser.add_argument("--print-hashes", action="store_true", help="recompute hashes")
    options = parser.parse_args()

    manifest = json.loads(MANIFEST_PATH.read_text())
    PACKAGED.mkdir(exist_ok=True)
    problems: list[str] = []
    computed: dict[str, dict[str, str]] = {}

    for source in manifest["sources"]:
        name = source["name"]
        wanted = [face for face in manifest["faces"] if face["source"] == name]
        if options.check:
            for face in wanted:
                path = PACKAGED / face["file"]
                if not path.is_file():
                    problems.append(f"{face['file']}: not vendored")
                    continue
                found = sha256(path.read_bytes())
                if found != face["sha256"]:
                    problems.append(
                        f"{face['file']}: manifest {face['sha256']}, found {found}"
                    )
            for notice in source.get("notices", []):
                path = PACKAGED / notice["file"]
                if not path.is_file() or sha256(path.read_bytes()) != notice["sha256"]:
                    problems.append(f"{notice['file']}: missing or hash mismatch")
            continue

        print(f"{name} {source['version']}", file=sys.stderr)
        data = fetch(source["url"])
        found = sha256(data)
        if options.print_hashes:
            computed.setdefault(name, {})["archive"] = found
        elif found != source["sha256"]:
            problems.append(
                f"{source['archive']}: manifest {source['sha256']}, downloaded {found}"
            )
            continue
        entries = members(source["archive"], data)
        for face in wanted:
            member = entries.get(face["member"])
            if member is None:
                problems.append(f"{face['member']}: not in {source['archive']}")
                continue
            found = sha256(member)
            if options.print_hashes:
                computed.setdefault(name, {})[face["file"]] = found
                continue
            if found != face["sha256"]:
                problems.append(
                    f"{face['file']}: manifest {face['sha256']}, extracted {found}"
                )
                continue
            (PACKAGED / face["file"]).write_bytes(member)
            print(f"  {face['file']} {found[:16]}…", file=sys.stderr)
        for notice in source.get("notices", []):
            member = fetch(notice["url"]) if "url" in notice else entries.get(notice["member"])
            if member is None or sha256(member) != notice["sha256"]:
                problems.append(f"{notice['file']}: missing or hash mismatch")
            else:
                (PACKAGED / notice["file"]).write_bytes(member)

    if options.print_hashes:
        print(json.dumps(computed, indent=1))
        return 0
    if problems:
        print("\n".join(problems), file=sys.stderr)
        return 1
    print("every vendored face matches the manifest", file=sys.stderr)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())

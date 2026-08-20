#!/usr/bin/env python3
"""Build the Tauri updater manifest (`latest.json`) from the collected bundles.

We do not build through `tauri-action`, so nobody hands us the per-platform
`*.json` manifests it emits — what CI has is the release assets themselves plus
the `.sig` file Tauri writes next to each of them when
`bundle.createUpdaterArtifacts` is on. This script turns that directory into the
one manifest the updater plugin fetches from
`releases/latest/download/latest.json`.

Usage:
    build-updater-manifest.py <dist-dir> <tag> [notes] [--require-all]
                              [--config <tauri.conf.json>]

`tag` is the git tag (`gui-v0.2.1`); the version in the manifest comes from it,
not from tauri.conf.json — the tag is what the release is actually named, and a
manifest promising a version the release does not carry sends every client into
a download loop.

Recognised bundles, and the platform keys they answer to:

    *.app.tar.gz  -> darwin-aarch64, darwin-x86_64  (one universal bundle)
    *.AppImage    -> linux-x86_64
    *-setup.exe   -> windows-x86_64

`--require-all` demands all four platform keys and fails without them. CI turns
it on when every matrix build succeeded: there, a missing key means the bundle
or its signature quietly did not appear, and shipping that manifest would tell
every user of that platform "you are up to date" forever. When a build did fail,
the flag is off — this workflow publishes partial releases on purpose — and the
platforms left out are named in the release body instead.

`--config` cross-checks the tag against `version` in `tauri.conf.json` and fails
on a mismatch. The manifest's version comes from the tag, the bundle's from the
config: when they disagree the client installs a bundle that still reports the
old version, finds the same update waiting, and reinstalls it forever.

A bundle whose `.sig` is missing is skipped loudly: an entry without a signature
is rejected by the client at install time, which is a worse failure than an
absent entry. If nothing at all can be assembled the script exits non-zero, so
the workflow stops before publishing a release the app cannot update from —
that exact silent hole cost project Pane four broken releases in a row, because
the installers kept building just fine.

Stdlib only: it has to run on a bare `ubuntu-latest`.
"""

import json
import os
import sys
from urllib.parse import quote
from datetime import datetime, timezone
from pathlib import Path

# Suffix -> platform keys. Order matters: the longest/most specific suffix that
# matches wins, so `-setup.exe` is tested before a bare `.exe` would be.
RULES: list[tuple[str, list[str]]] = [
    (".app.tar.gz", ["darwin-aarch64", "darwin-x86_64"]),
    (".AppImage", ["linux-x86_64"]),
    ("-setup.exe", ["windows-x86_64"]),
]

ASSET_URL = "https://github.com/thothlab/my-git/releases/download/{tag}/{name}"


def version_from_tag(tag: str) -> str:
    """`gui-v0.2.1` -> `0.2.1`. Anything else is passed through unchanged."""
    return tag[len("gui-v"):] if tag.startswith("gui-v") else tag.lstrip("v")


ALL_KEYS = ("darwin-aarch64", "darwin-x86_64", "linux-x86_64", "windows-x86_64")


def main() -> int:
    argv = sys.argv[1:]
    require_all = "--require-all" in argv
    config: Path | None = None
    if "--config" in argv:
        i = argv.index("--config")
        if i + 1 >= len(argv):
            print(__doc__, file=sys.stderr)
            return 2
        config = Path(argv[i + 1])
        del argv[i : i + 2]
    args = [a for a in argv if a != "--require-all"]
    if not 2 <= len(args) <= 3:
        print(__doc__, file=sys.stderr)
        return 2
    dist = Path(args[0])
    tag = args[1]
    notes = args[2] if len(args) == 3 else f"Graft {tag}"
    version = version_from_tag(tag)

    if config is not None:
        bundled = json.loads(config.read_text()).get("version")
        if bundled != version:
            print(
                f"::error::tag {tag} says version {version} but {config} says "
                f"{bundled} - the manifest would advertise a version the bundle "
                "does not report, and every client would reinstall it forever",
                file=sys.stderr,
            )
            return 1
    if not dist.is_dir():
        print(f"not a directory: {dist}", file=sys.stderr)
        return 1

    platforms: dict[str, dict[str, str]] = {}
    for f in sorted(dist.iterdir()):
        if not f.is_file() or f.name.endswith(".sig"):
            continue
        keys = next((k for suffix, k in RULES if f.name.endswith(suffix)), None)
        if keys is None:
            continue
        sig = f.with_name(f.name + ".sig")
        if not sig.is_file():
            print(f"WARNING: no signature for {f.name} - skipped", file=sys.stderr)
            continue
        entry = {
            "signature": sig.read_text().strip(),
            "url": ASSET_URL.format(tag=tag, name=quote(f.name)),
        }
        for key in keys:
            platforms[key] = entry

    if not platforms:
        print(
            "no updater bundles with signatures found in "
            f"{dist} - refusing to publish a release nobody can update from "
            "(is createUpdaterArtifacts on and are the signing secrets set?)",
            file=sys.stderr,
        )
        return 1

    missing = [k for k in ALL_KEYS if k not in platforms]
    if missing and require_all:
        print(
            f"::error::no updater bundle for {', '.join(missing)} although every "
            "build job succeeded - the release would silently never update those "
            "platforms",
            file=sys.stderr,
        )
        return 1
    if missing:
        # A build job failed, and this workflow ships partial releases on
        # purpose. Not fatal, but not silent either: the caller writes the gap
        # into the release body.
        print(f"WARNING: no updater bundle for {', '.join(missing)}", file=sys.stderr)
    # Machine-readable gap for the workflow, so the release body can name it
    # without keeping its own copy of the platform list.
    out = os.environ.get("GITHUB_OUTPUT")
    if out:
        with open(out, "a", encoding="utf-8") as fh:
            fh.write(f"missing={','.join(missing)}\n")

    manifest = {
        "version": version,
        "notes": notes,
        # RFC 3339 with an explicit offset - the plugin refuses to parse
        # anything else.
        "pub_date": datetime.now(timezone.utc).strftime("%Y-%m-%dT%H:%M:%SZ"),
        "platforms": platforms,
    }
    manifest_path = dist / "latest.json"
    manifest_path.write_text(json.dumps(manifest, indent=2, sort_keys=True) + "\n")
    print(f"wrote {manifest_path} for {', '.join(sorted(platforms))}", file=sys.stderr)
    return 0


if __name__ == "__main__":
    sys.exit(main())

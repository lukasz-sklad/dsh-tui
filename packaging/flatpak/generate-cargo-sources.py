#!/usr/bin/env python3
"""Generate Flathub-compatible cargo-sources.json from Cargo.lock."""

import json
import pathlib
import tomllib

REPO_ROOT = pathlib.Path(__file__).resolve().parents[2]
LOCK_PATH = REPO_ROOT / "Cargo.lock"
OUT_PATH = pathlib.Path(__file__).resolve().parent / "cargo-sources.json"


def main() -> None:
    lock_data = tomllib.loads(LOCK_PATH.read_text(encoding="utf-8"))
    sources = []

    for pkg in lock_data.get("package", []):
        source = pkg.get("source", "")
        if not source.startswith("registry+"):
            continue
        name = pkg["name"]
        version = pkg["version"]
        checksum = pkg["checksum"]
        dest = f"cargo/vendor/{name}-{version}"

        sources.append(
            {
                "type": "archive",
                "archive-type": "tar-gzip",
                "url": f"https://static.crates.io/crates/{name}/{name}-{version}.crate",
                "sha256": checksum,
                "dest": dest,
            }
        )
        sources.append(
            {
                "type": "inline",
                "contents": json.dumps({"package": checksum, "files": {}}),
                "dest": dest,
                "dest-filename": ".cargo-checksum.json",
            }
        )

    sources.append(
        {
            "type": "inline",
            "contents": (
                "[source.crates-io]\n"
                'replace-with = "vendored-sources"\n\n'
                "[source.vendored-sources]\n"
                'directory = "cargo/vendor"\n'
            ),
            "dest": "cargo",
            "dest-filename": "config.toml",
        }
    )

    OUT_PATH.write_text(json.dumps(sources, indent=2) + "\n", encoding="utf-8")
    print(f"Generated {OUT_PATH} with {len(sources)} entries.")


if __name__ == "__main__":
    main()

"""Write notices for the dependencies in Cargo's macOS package graph."""

import json
from pathlib import Path
import sys


metadata = json.load(sys.stdin)
packaging = Path(__file__).resolve().parent
dependency_ids = {node["id"] for node in metadata["resolve"]["nodes"]}
dependency_ids.remove(metadata["resolve"]["root"])
with Path(sys.argv[1]).open("w", encoding="utf-8") as output:
    for package in metadata["packages"]:
        if package["id"] not in dependency_ids:
            continue
        name = package["name"]
        source = Path(package["manifest_path"]).parent
        files = {
            path
            for path in source.iterdir()
            if path.is_file()
            and path.name.lower().startswith(("license", "licence", "copying", "notice", "copyright"))
        }
        if package["license_file"]:
            files.add(source / package["license_file"])
        if not files and package["repository"] == "https://github.com/madsmtm/objc2":
            # Some objc2 crates reference the license at the upstream workspace root.
            files.add(packaging / "objc2-MIT.txt")
        if not files:
            sys.exit(f"No license notice found for {name} {package['version']}")
        print(f"{name} {package['version']} ({package['license']})", file=output)
        if package["repository"]:
            print(package["repository"], file=output)
        for path in sorted(files):
            print(f"\n{path.name}\n{path.read_text(encoding='utf-8').rstrip()}\n", file=output)

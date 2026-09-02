#!/usr/bin/env python3

import json
import pathlib
import tomllib


ROOT = pathlib.Path(__file__).resolve().parents[2]


def version_tuple(value: str) -> tuple[int, int, int]:
    parts = value.split(".")
    if len(parts) != 3 or any(not part.isdigit() for part in parts):
        raise SystemExit(f"expected a stable semantic version, got {value!r}")
    return tuple(int(part) for part in parts)


def validate_state(manifest_version: str, package_version: str) -> str:
    manifest_semver = version_tuple(manifest_version)
    package_semver = version_tuple(package_version)
    if manifest_semver > package_semver:
        raise SystemExit(
            f"release manifest {manifest_version} is ahead of package {package_version}"
        )
    if manifest_semver == package_semver:
        return "post-merge"
    return "pre-release"


def find_key(value: object, key: str, path: str = "$") -> list[str]:
    matches: list[str] = []
    if isinstance(value, dict):
        for child_key, child_value in value.items():
            child_path = f"{path}.{child_key}"
            if child_key == key:
                matches.append(child_path)
            matches.extend(find_key(child_value, key, child_path))
    elif isinstance(value, list):
        for index, child_value in enumerate(value):
            matches.extend(find_key(child_value, key, f"{path}[{index}]"))
    return matches


config = json.loads((ROOT / "release-please-config.json").read_text())
manifest = json.loads((ROOT / ".release-please-manifest.json").read_text())
with (ROOT / "Cargo.toml").open("rb") as cargo_file:
    package_version = tomllib.load(cargo_file)["package"]["version"]

release_as_paths = find_key(config, "release-as")
if release_as_paths:
    raise SystemExit(f"persistent release-as is forbidden: {release_as_paths}")

package_config = config["packages"]["."]
if package_config.get("release-type") != "rust":
    raise SystemExit("Release Please package must use the rust release strategy")
if package_config.get("bump-minor-pre-major") is not True:
    raise SystemExit("bump-minor-pre-major must remain enabled")

last_published = manifest.get(".")
if not isinstance(last_published, str):
    raise SystemExit("release manifest must contain a string version for package '.'")

state = validate_state(last_published, package_version)
validate_state(package_version, package_version)

major, minor, patch = version_tuple(package_version)
future_versions = (
    f"{major}.{minor}.{patch + 1}",
    f"{major}.{minor + 1}.0",
    f"{major + 1}.0.0",
)
for future_version in future_versions:
    if validate_state(package_version, future_version) != "pre-release":
        raise SystemExit(f"future release {future_version} was not recognized")
    if validate_state(future_version, future_version) != "post-merge":
        raise SystemExit(f"post-merge release {future_version} was not recognized")

try:
    validate_state(future_versions[0], package_version)
except SystemExit:
    pass
else:
    raise SystemExit("backward semantic-version movement was accepted")

print(
    f"Release Please ({state}): manifest {last_published}, package {package_version}, "
    f"validated future patch/minor/major progression"
)

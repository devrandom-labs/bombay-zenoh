"""Audit actual release archives; local verification does not prove publication."""

import copy
import hashlib
import sys
import tarfile
import tomllib
from pathlib import Path, PurePosixPath

CONTROLLED_REPOSITORY = "https://github.com/devrandom-labs/bombay-zenoh"
ZENOH_REVISION = "1211779c3647f5a96713dade452c546a07823580"
LICENSE_SHA256 = "01a44774f7b1a453595c7c6d7f7308284ba6a1059dc49e14dad6647e1d44a338"
ZENOH_OWNERS = {
    "zenoh": "zenoh",
    "zenoh-config": "commons/zenoh-config",
    "zenoh-link-commons": "io/zenoh-link-commons",
    "zenoh-link-tls": "io/zenoh-links/zenoh-link-tls",
    "zenoh-link": "io/zenoh-link",
    "zenoh-plugin-trait": "plugins/zenoh-plugin-trait",
    "zenoh-transport": "io/zenoh-transport",
}
CONFIGURATION_OWNERS = {
    "validated_struct": ("validated-struct", "9cd5eda2d2a90e84cf47f3e0cdfb21e1df9d6729"),
    "validated_struct_macros": (
        "validated-struct-macros", "3eebb3c2e9147ac6c7c8cde987d89b22c9ab098a"
    ),
}
EDGES = {
    "zenoh": {"zenoh-config", "zenoh-link", "zenoh-link-commons",
              "zenoh-plugin-trait", "zenoh-transport"},
    "zenoh-config": {"validated_struct"},
    "zenoh-link-commons": set(),
    "zenoh-link-tls": {"zenoh-config", "zenoh-link-commons"},
    "zenoh-link": {"zenoh-config", "zenoh-link-commons", "zenoh-link-tls"},
    "zenoh-plugin-trait": {"zenoh-config"},
    "zenoh-transport": {"zenoh-config", "zenoh-link", "zenoh-link-commons"},
    "validated_struct": {"validated_struct_macros"},
    "validated_struct_macros": set(),
}
FEATURES = {
    "zenoh": {"default", "transport_tls", "internal", "internal_config", "test", "unstable"},
    "zenoh-config": {"internal", "unstable"},
    "zenoh-link-commons": {"tls"},
    "zenoh-link-tls": set(),
    "zenoh-link": {"transport_tls", "zenoh-link-tls"},
    "zenoh-plugin-trait": set(),
    "zenoh-transport": {"default", "test", "transport_tls", "unstable"},
    "validated_struct": {"json_get", "json5", "serde_json"},
    "validated_struct_macros": {"serde", "serde_json", "dot_separator"},
}


def require(condition, description):
    if not condition:
        raise ValueError(description)


def identity(owner):
    return "bombay-" + owner.replace("_", "-")


def version(owner):
    return "1.10.1" if owner in ZENOH_OWNERS else "2.2.0"


def dependency_tables(manifest):
    for section in (manifest, *manifest.get("target", {}).values()):
        for category in ("dependencies", "build-dependencies", "dev-dependencies"):
            yield from section.get(category, {}).items()


def verify_manifest(owner, manifest):
    package = manifest["package"]
    require(package["name"] == identity(owner), f"{owner}: package identity")
    require(package["version"] == version(owner), f"{owner}: package version")
    require(manifest["lib"]["name"] == owner.replace("-", "_"), f"{owner}: library identity")
    require(package["repository"] == CONTROLLED_REPOSITORY, f"{owner}: repository")
    require(package["license"] == "EPL-2.0 OR Apache-2.0", f"{owner}: license expression")
    if owner in {"zenoh-link-commons", "zenoh-link-tls"}:
        require(manifest["dependencies"]["rustls"]["version"] == "0.23.45", f"{owner}: Rustls minimum")
    provenance = package["metadata"]["bombay"]
    if owner in ZENOH_OWNERS:
        revision, repository, profile = ZENOH_REVISION, "https://github.com/eclipse-zenoh/zenoh", "tls-only"
    else:
        upstream, revision = CONFIGURATION_OWNERS[owner]
        repository = "https://github.com/p-avital/" + upstream + "-rs"
        profile = "configuration-owner"
    for key, expected in {"upstream-revision": revision, "upstream-repository": repository,
                          "supported-profile": profile, "controlled-repository": CONTROLLED_REPOSITORY}.items():
        require(provenance[key] == expected, f"{owner}: provenance {key}")
    affected = {}
    for alias, specification in dependency_tables(manifest):
        require(not {"path", "git"}.intersection(specification), f"{owner}: nonregistry edge {alias}")
        selected = specification.get("package", alias)
        require(selected not in EDGES, f"{owner}: original dependency edge {alias} -> {selected}")
        if selected.startswith("bombay-"):
            require(alias in EDGES[owner], f"{owner}: unexpected corrected edge {alias}")
            expected_version = "=1.10.1" if alias in ZENOH_OWNERS else "2.2.0"
            require(selected == identity(alias), f"{owner}: corrected dependency identity {alias}")
            require(specification["version"] == expected_version, f"{owner}: edge version {alias}")
            affected[alias] = selected
    require(set(affected) == EDGES[owner], f"{owner}: incomplete corrected edges")
    features = manifest.get("features", {})
    feature_names = set(features)
    references = {reference for values in features.values() for reference in values}
    for reference in references:
        alias, separator, forwarded = reference.partition("/")
        if separator and alias.rstrip("?") in FEATURES:
            require(forwarded in FEATURES[alias.rstrip("?")], f"{owner}: excluded feature forwarding {reference}")
    for alias, specification in manifest.get("dependencies", {}).items():
        if specification.get("optional") and "dep:" + alias not in references:
            feature_names.add(alias)
    require(feature_names == FEATURES[owner], f"{owner}: unsupported or missing features {feature_names}")
    if owner in ZENOH_OWNERS:
        expected_default = ["transport_tls"] if owner == "zenoh" else []
        require(features.get("default", []) == expected_default, f"{owner}: default profile")


def read_archive(archive, prefix):
    contents = {}
    with tarfile.open(archive, "r:gz") as packaged:
        for member in packaged.getmembers():
            path = PurePosixPath(member.name)
            require(path.parts[0] == prefix and ".." not in path.parts, "archive path escape")
            if member.isdir():
                continue
            require(member.isfile(), f"archive nonregular entry: {member.name}")
            relative = path.relative_to(prefix).as_posix()
            require(relative not in contents, f"duplicate archive entry: {relative}")
            contents[relative] = packaged.extractfile(member).read()
    return contents


def verify_archive(root, archives, owner):
    prefix = identity(owner) + "-" + version(owner)
    contents = read_archive(archives / (prefix + ".crate"), prefix)
    manifest = tomllib.loads(contents["Cargo.toml"].decode())
    verify_manifest(owner, manifest)
    directory = root / (ZENOH_OWNERS[owner] if owner in ZENOH_OWNERS else
                        "dependencies/" + CONFIGURATION_OWNERS[owner][0])
    require(contents["Cargo.toml.orig"] == (directory / "Cargo.toml").read_bytes(), f"{owner}: active manifest custody")
    readme = manifest["package"].get("readme")
    if owner in ZENOH_OWNERS:
        require(readme == "README.md", f"{owner}: owning profile readme")
        require(contents[readme] == (directory / readme).read_bytes(), f"{owner}: profile readme bytes")
    expected = {path.relative_to(directory).as_posix(): path.read_bytes()
                for path in directory.rglob("*.rs")
                if "target" not in path.relative_to(directory).parts}
    actual = {path: payload for path, payload in contents.items() if path.endswith(".rs")}
    require(actual == expected, f"{owner}: Rust source inventory or bytes differ")
    license_path = manifest["package"].get("license-file", "LICENSE")
    require(contents[license_path] == (root / "LICENSE").read_bytes(), f"{owner}: license bytes")
    require(hashlib.sha256(contents[license_path]).hexdigest() == LICENSE_SHA256, f"{owner}: upstream license")
    for custody in ("UPSTREAM_CARGO.toml", "UPSTREAM_VCS.json"):
        if (directory / custody).exists():
            require(contents[custody] == (directory / custody).read_bytes(), f"{owner}: {custody}")
    print(f"verified {prefix}: {len(actual)} Rust files; archive sha256="
          + hashlib.sha256((archives / (prefix + ".crate")).read_bytes()).hexdigest())
    return manifest


def main():
    root, archives = map(Path, sys.argv[1:])
    manifests = {owner: verify_archive(root, archives, owner) for owner in EDGES}
    for owner, defect in (("zenoh-config", "dependency"), ("zenoh", "feature")):
        mutant = copy.deepcopy(manifests[owner])
        if defect == "dependency":
            mutant["dependencies"]["validated_struct"]["package"] = "validated_struct"
            expected_diagnostic = "original dependency edge validated_struct"
        else:
            mutant["features"]["transport_tcp"] = ["zenoh-transport/transport_tcp"]
            expected_diagnostic = "excluded feature forwarding zenoh-transport/transport_tcp"
        try:
            verify_manifest(owner, mutant)
        except ValueError as refused:
            require(expected_diagnostic in str(refused), f"wrong inversion diagnostic: {refused}")
            print(f"intended {defect} inversion refused: {refused}")
        else:
            raise ValueError(f"{defect} inversion unexpectedly accepted")
        verify_archive(root, archives, owner)
    print("Nine actual archives and restored archive controls passed; publication remains unproved.")


if __name__ == "__main__":
    main()

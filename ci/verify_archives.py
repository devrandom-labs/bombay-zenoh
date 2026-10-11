"""Audit actual release archives; local verification does not prove publication."""

import argparse
import copy
import enum
import hashlib
import json
import subprocess
import tarfile
import tomllib
from io import BytesIO
from pathlib import Path, PurePosixPath
from tempfile import TemporaryDirectory
from urllib.request import Request, urlopen

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


class ReleaseKind(enum.Enum):
    INITIAL_COHORT = "initial-cohort"
    ZENOH_SUCCESSOR = "zenoh-successor"


PUBLISHED_REVISION = "e82481825e313ff14e5ea3a1d6e842040945372a"
PUBLISHED_DEPENDENCIES = {
    "zenoh-config": "84f764e6cc4f3c122b87f5ad715460c54e09fc95b690919740defe4e264ed482",
    "zenoh-link-commons": "ef9224e682a1cf8f57bb4fc88f98ac6a1f51036fc6f82b7b1c05dfcbafa8e37c",
    "zenoh-link-tls": "29075698e973b6d270f1d7e170209b70b4a2d14eac7aca61b559a29861c0246c",
    "zenoh-link": "40ff7d86f5e5da473ca7e05ca452c2782971ef3eb45ee1ac8c80a7b7fd9a5c91",
    "zenoh-plugin-trait": "7f706fdfff554904c5c87ffee53ceeb5c16c81d720444e4add2877753af09b64",
    "zenoh-transport": "3adae45298b79e80fc6c3494a4e26b84b5f347ee71aa83d5db85df9db8f6b627",
    "validated_struct": "b1b9764c93caf9628a9c391a92132f1bc84a8816b01ff486cc18b08101e2c3c7",
    "validated_struct_macros": "602d3538d65be50613f90d69dd21c504d4e894ae2ae13c41ba89163fd4b33b55",
}


def require(condition, description):
    if not condition:
        raise ValueError(description)


def identity(owner):
    return "bombay-" + owner.replace("_", "-")


def version(owner, release_kind=ReleaseKind.INITIAL_COHORT):
    if owner == "zenoh" and release_kind is ReleaseKind.ZENOH_SUCCESSOR:
        return "1.10.4"
    return "1.10.1" if owner in ZENOH_OWNERS else "2.2.0"


def dependency_tables(manifest):
    for section in (manifest, *manifest.get("target", {}).values()):
        for category in ("dependencies", "build-dependencies", "dev-dependencies"):
            yield from section.get(category, {}).items()


def verify_manifest(owner, manifest, release_kind=ReleaseKind.INITIAL_COHORT):
    package = manifest["package"]
    require(package["name"] == identity(owner), f"{owner}: package identity")
    require(package["version"] == version(owner, release_kind), f"{owner}: package version")
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


def read_archive(archive, prefix, expected_checksum=None):
    archive_bytes = archive.read_bytes()
    if expected_checksum is not None:
        require(hashlib.sha256(archive_bytes).hexdigest() == expected_checksum,
                f"{prefix}: immutable archive checksum")
    contents = {}
    with tarfile.open(fileobj=BytesIO(archive_bytes), mode="r:gz") as packaged:
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


def verify_archive(root, archives, owner, revision, release_kind=ReleaseKind.INITIAL_COHORT,
                   expected_checksum=None):
    prefix = identity(owner) + "-" + version(owner, release_kind)
    archive = archives / (prefix + ".crate")
    if release_kind is ReleaseKind.ZENOH_SUCCESSOR and owner != "zenoh":
        expected_checksum = PUBLISHED_DEPENDENCIES[owner]
    contents = read_archive(archive, prefix, expected_checksum)
    manifest = tomllib.loads(contents["Cargo.toml"].decode())
    verify_manifest(owner, manifest, release_kind)
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
    vcs = json.loads(contents[".cargo_vcs_info.json"])
    require(vcs["git"]["sha1"] == revision, f"{owner}: archive source commit")
    require(not vcs["git"].get("dirty", False), f"{owner}: dirty archive source")
    require(vcs["path_in_vcs"] == directory.relative_to(root).as_posix(), f"{owner}: archive source owner")
    license_path = manifest["package"].get("license-file", "LICENSE")
    require(contents[license_path] == (root / "LICENSE").read_bytes(), f"{owner}: license bytes")
    require(hashlib.sha256(contents[license_path]).hexdigest() == LICENSE_SHA256, f"{owner}: upstream license")
    for custody in ("UPSTREAM_CARGO.toml", "UPSTREAM_VCS.json"):
        if (directory / custody).exists():
            require(contents[custody] == (directory / custody).read_bytes(), f"{owner}: {custody}")
    print(f"verified {prefix}: {len(actual)} Rust files; archive sha256="
          + (expected_checksum or hashlib.sha256(archive.read_bytes()).hexdigest()))
    return manifest


def verify_lock(owner, lock, checksums, release_kind=ReleaseKind.INITIAL_COHORT):
    source_names = [package["name"] for package in lock["package"] if "source" not in package]
    require(source_names == [identity(owner)], f"{owner}: locked owning source node")
    controlled_names = {identity(dependency): dependency for dependency in EDGES}
    edges = []
    for package in lock["package"]:
        name = package["name"]
        require(name not in EDGES, f"{owner}: original locked dependency {name}")
        if not name.startswith("bombay-"):
            continue
        controlled = controlled_names.get(name)
        require(controlled in EDGES, f"{owner}: unexpected locked package {name}")
        require(package["version"] == version(controlled, release_kind), f"{owner}: locked version {name}")
        if "source" not in package:
            require(name == identity(owner), f"{owner}: nonregistry locked dependency {name}")
            continue
        require(package["source"] == "registry+https://github.com/rust-lang/crates.io-index",
                f"{owner}: locked registry {name}")
        require(package.get("checksum") == checksums[name], f"{owner}: locked checksum {name}")
        require(name != identity(owner), f"{owner}: registry self edge")
        edges.append(name)
    expected = set(EDGES[owner])
    remaining = list(expected)
    while remaining:
        dependency = remaining.pop()
        for descendant in EDGES[dependency] - expected:
            expected.add(descendant)
            remaining.append(descendant)
    require(set(edges) == {identity(dependency) for dependency in expected},
            f"{owner}: controlled lock closure")
    require(len(edges) == len(expected), f"{owner}: duplicate controlled locked package")
    controlled_nodes = {package["name"]: package for package in lock["package"]
                        if package["name"].startswith("bombay-")}
    for name, package in controlled_nodes.items():
        references = set()
        for reference in package.get("dependencies", []):
            parts = reference.split()
            require(parts and parts[0] not in EDGES, f"{owner}: locked dependency edges")
            if not parts[0].startswith("bombay-"):
                continue
            target = controlled_nodes.get(parts[0])
            require(target is not None and len(parts) <= 3 and parts[0] not in references
                    and (len(parts) < 2 or parts[1] == target["version"])
                    and (len(parts) < 3 or parts[2] == "(" + target.get("source", "") + ")"),
                    f"{owner}: locked dependency edges")
            references.add(parts[0])
        require(name in controlled_names, f"{owner}: unexpected locked package {name}")
        require(references == {identity(dependency) for dependency in EDGES[controlled_names[name]]},
                f"{owner}: locked dependency edges")
    return edges


def acquire_published_dependencies(archives):
    receipts = {}
    require(set(PUBLISHED_DEPENDENCIES) == set(EDGES) - {"zenoh"}, "published dependency inventory")
    for owner, checksum in PUBLISHED_DEPENDENCIES.items():
        package, selected_version = identity(owner), version(owner)
        prefix = package + "-" + selected_version
        headers = {"User-Agent": "Bombay controlled dependency release"}
        with urlopen(Request(f"https://crates.io/api/v1/crates/{package}/{selected_version}",
                             headers=headers), timeout=30) as response:
            published = json.load(response)["version"]
        require(published["crate"] == package and published["num"] == selected_version
                and published["checksum"] == checksum
                and not published["yanked"], f"{owner}: published dependency receipt")
        archive = archives / (prefix + ".crate")
        with urlopen(Request(f"https://static.crates.io/crates/{package}/{prefix}.crate",
                             headers=headers), timeout=30) as response:
            archive.write_bytes(response.read())
        require(hashlib.sha256(archive.read_bytes()).hexdigest() == checksum,
                f"{owner}: immutable published archive checksum")
        receipts[package] = {"version": selected_version, "checksum": checksum,
                             "expected_source": PUBLISHED_REVISION}
        (archives / "published-dependencies.json").write_text(
            json.dumps(receipts, indent=2, sort_keys=True) + "\n")


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("root", type=Path)
    parser.add_argument("archives", type=Path)
    parser.add_argument("ledger", type=Path, nargs="?")
    parser.add_argument("--release-kind", choices=[kind.value for kind in ReleaseKind],
                        default=ReleaseKind.INITIAL_COHORT.value)
    arguments = parser.parse_args()
    root, archives = arguments.root, arguments.archives
    release_kind = ReleaseKind(arguments.release_kind)
    if release_kind is ReleaseKind.ZENOH_SUCCESSOR:
        acquire_published_dependencies(archives)
    revision = subprocess.check_output(["git", "-C", str(root), "rev-parse", "HEAD"], text=True).strip()
    checksums = {identity(owner): hashlib.sha256(
        (archives / (identity(owner) + "-" + version(owner, release_kind) + ".crate")).read_bytes()).hexdigest()
        for owner in EDGES}
    manifests = {owner: verify_archive(
        root, archives, owner,
        PUBLISHED_REVISION if release_kind is ReleaseKind.ZENOH_SUCCESSOR and owner != "zenoh" else revision,
        release_kind, checksums[identity(owner)]) for owner in EDGES}
    locks = {owner: tomllib.loads(read_archive(
        archives / (identity(owner) + "-" + version(owner, release_kind) + ".crate"),
        identity(owner) + "-" + version(owner, release_kind), checksums[identity(owner)])["Cargo.lock"].decode()) for owner in EDGES}
    edge_count = sum(len(verify_lock(owner, lock, checksums, release_kind)) for owner, lock in locks.items())
    require(edge_count == 29, "controlled archived checksum edge inventory changed")
    if release_kind is ReleaseKind.ZENOH_SUCCESSOR:
        # Changing gzip metadata preserves source members but violates published byte custody.
        with TemporaryDirectory(prefix="bombay-published-archive-") as temporary:
            owner = "zenoh-config"
            archive_name = identity(owner) + "-" + version(owner) + ".crate"
            original = (archives / archive_name).read_bytes()
            changed = bytearray(original)
            changed[4] ^= 1
            mutant_archives = Path(temporary)
            (mutant_archives / archive_name).write_bytes(changed)
            try:
                verify_archive(root, mutant_archives, owner, PUBLISHED_REVISION, release_kind)
            except ValueError as refused:
                require("immutable archive checksum" in str(refused),
                        f"wrong published archive inversion diagnostic: {refused}")
                print(f"intended published archive inversion refused: {refused}")
            else:
                raise ValueError("changed immutable published archive unexpectedly accepted")
            verify_archive(root, archives, owner, PUBLISHED_REVISION, release_kind)
    for defect in ("checksum", "identity", "closure", "reference"):
        mutant = copy.deepcopy(locks["zenoh-config"])
        dependency = next(package for package in mutant["package"]
                          if package["name"] == "bombay-validated-struct")
        if defect == "checksum":
            dependency["checksum"] = "0" * 64
            diagnostic = "locked checksum bombay-validated-struct"
        elif defect == "identity":
            dependency["name"] = "validated_struct"
            diagnostic = "original locked dependency validated_struct"
        elif defect == "closure":
            mutant["package"].remove(dependency)
            diagnostic = "controlled lock closure"
        else:
            source = next(package for package in mutant["package"]
                          if package["name"] == identity("zenoh-config"))
            source["dependencies"].remove(identity("validated_struct"))
            diagnostic = "locked dependency edges"
        try:
            verify_lock("zenoh-config", mutant, checksums, release_kind)
        except ValueError as refused:
            require(diagnostic in str(refused), f"wrong lock inversion diagnostic: {refused}")
            print(f"intended lock {defect} inversion refused: {refused}")
        else:
            raise ValueError(f"lock {defect} inversion unexpectedly accepted")
        verify_lock("zenoh-config", locks["zenoh-config"], checksums, release_kind)
    print(f"Verified {edge_count} controlled archived registry checksum edges.")
    for owner, defect in (("zenoh-config", "dependency"), ("zenoh", "feature")):
        mutant = copy.deepcopy(manifests[owner])
        if defect == "dependency":
            mutant["dependencies"]["validated_struct"]["package"] = "validated_struct"
            expected_diagnostic = "original dependency edge validated_struct"
        else:
            mutant["features"]["transport_tcp"] = ["zenoh-transport/transport_tcp"]
            expected_diagnostic = "excluded feature forwarding zenoh-transport/transport_tcp"
        try:
            verify_manifest(owner, mutant, release_kind)
        except ValueError as refused:
            require(expected_diagnostic in str(refused), f"wrong inversion diagnostic: {refused}")
            print(f"intended {defect} inversion refused: {refused}")
        else:
            raise ValueError(f"{defect} inversion unexpectedly accepted")
        verify_archive(root, archives, owner,
                       PUBLISHED_REVISION if release_kind is ReleaseKind.ZENOH_SUCCESSOR and owner != "zenoh" else revision,
                       release_kind, checksums[identity(owner)])
    if arguments.ledger is not None:
        cohort = {identity(owner): {"version": version(owner, release_kind), "checksum": checksums[identity(owner)]}
                  for owner in EDGES if release_kind is ReleaseKind.INITIAL_COHORT or owner == "zenoh"}
        arguments.ledger.write_text(json.dumps(cohort, sort_keys=True) + "\n")
    print(f"Nine actual {release_kind.value} archives and restored archive controls passed; publication remains unproved.")


if __name__ == "__main__":
    main()

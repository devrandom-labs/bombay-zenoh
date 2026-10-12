"""Invert custody laws on the six actual successors and three immutable archives."""
import copy
import hashlib
import json
from pathlib import Path
import tomllib
import unittest

from verify_archives import (
    CONFIGURATION_CANDIDATES, EDGES, PUBLISHED_DEPENDENCIES, ReleaseKind,
    identity, published_owners, read_archive, verify_lock, verify_manifest, version,
)


class ConfigurationCohortEvidence(unittest.TestCase):
    def test_actual_successor_closure_and_custody_denials(self):
        kind = ReleaseKind.CONFIGURATION_SUCCESSOR
        immutable = {"zenoh-link-commons", "validated_struct", "validated_struct_macros"}
        self.assertEqual(published_owners(kind), immutable)
        self.assertEqual(CONFIGURATION_CANDIDATES, {
            "zenoh-config", "zenoh-link-tls", "zenoh-link", "zenoh-plugin-trait",
            "zenoh-transport", "zenoh",
        })
        self.assertEqual(published_owners(ReleaseKind.INITIAL_COHORT), set())
        self.assertEqual(published_owners(ReleaseKind.ZENOH_SUCCESSOR), set(PUBLISHED_DEPENDENCIES))
        self.assertEqual(version("zenoh", ReleaseKind.ZENOH_SUCCESSOR), "1.10.7")
        self.assertEqual(version("zenoh-config", ReleaseKind.INITIAL_COHORT), "1.10.1")
        archives = Path("target/package/tmp-crate")
        ledger = json.loads(Path("target/package/audited-cohort.json").read_text())
        self.assertEqual(set(ledger), {identity(owner) for owner in CONFIGURATION_CANDIDATES})
        contents, checksums = {}, {}
        for owner in EDGES:
            prefix = identity(owner) + "-" + version(owner, kind)
            archive = archives / (prefix + ".crate")
            checksum = PUBLISHED_DEPENDENCIES[owner] if owner in immutable else ledger[identity(owner)]["checksum"]
            self.assertEqual(hashlib.sha256(archive.read_bytes()).hexdigest(), checksum)
            contents[owner] = read_archive(archive, prefix, checksum)
            checksums[identity(owner)] = checksum
        manifest = tomllib.loads(contents["zenoh"]["Cargo.toml"].decode())
        for owner in CONFIGURATION_CANDIDATES:
            successor = tomllib.loads(contents[owner]["Cargo.toml"].decode())
            verify_manifest(owner, successor, kind)
            for floor in (None, "1.75.0"):
                changed = copy.deepcopy(successor)
                if floor is None:
                    changed["package"].pop("rust-version")
                else:
                    changed["package"]["rust-version"] = floor
                with self.subTest(owner=owner, floor=floor), self.assertRaisesRegex(ValueError, "verified compiler floor"):
                    verify_manifest(owner, changed, kind)
            verify_manifest(owner, successor, kind)
        lock = tomllib.loads(contents["zenoh"]["Cargo.lock"].decode())
        verify_manifest("zenoh", manifest, kind)
        verify_lock("zenoh", lock, checksums, kind)
        wrong_version = copy.deepcopy(manifest)
        wrong_version["dependencies"]["zenoh-config"]["version"] = "=1.10.1"
        with self.assertRaisesRegex(ValueError, "edge version zenoh-config"):
            verify_manifest("zenoh", wrong_version, kind)
        extra_feature = copy.deepcopy(manifest)
        extra_feature["features"]["compression"] = []
        with self.assertRaisesRegex(ValueError, "unsupported or missing features"):
            verify_manifest("zenoh", extra_feature, kind)
        for defect in ("checksum", "duplicate", "version"):
            changed = copy.deepcopy(lock)
            config = next(package for package in changed["package"] if package["name"] == "bombay-zenoh-config")
            if defect == "checksum":
                config["checksum"] = "0" * 64
                diagnostic = "locked checksum bombay-zenoh-config"
            elif defect == "duplicate":
                changed["package"].append(copy.deepcopy(config))
                diagnostic = "duplicate controlled locked package"
            else:
                config["version"] = "1.10.1"
                diagnostic = "locked version bombay-zenoh-config"
            with self.subTest(defect=defect), self.assertRaisesRegex(ValueError, diagnostic):
                verify_lock("zenoh", changed, checksums, kind)
            verify_lock("zenoh", lock, checksums, kind)
        verify_manifest("zenoh", manifest, kind)


if __name__ == "__main__":
    unittest.main()

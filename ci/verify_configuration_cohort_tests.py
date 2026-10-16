"""Invert custody laws on the exact selected successors and immutable archives."""
import copy
import hashlib
import json
import os
from pathlib import Path
import tomllib
import unittest

from verify_archives import (
    CONFIGURATION_CANDIDATES, TLS_SECURITY_CANDIDATES, EDGES, PUBLISHED_DEPENDENCIES, ReleaseKind,
    identity, published_dependencies, published_owners, read_archive, verify_lock, verify_manifest, version,
)


class ConfigurationCohortEvidence(unittest.TestCase):
    def test_ingress_successor_preserves_exact_candidate_versions(self):
        if os.environ.get("BOMBAY_RELEASE_KIND") != "tls-ingress-successor":
            self.skipTest("requires the incoming TLS successor selection")
        ledger = json.loads(Path("target/package/audited-cohort.json").read_text())
        versions = {name: archive["version"] for name, archive in ledger.items()}
        self.assertEqual(versions, {
            "bombay-zenoh-link-tls": "1.10.4",
            "bombay-zenoh-link": "1.10.4",
            "bombay-zenoh-transport": "1.10.4",
            "bombay-zenoh": "1.10.10",
        })

    def test_actual_successor_closure_and_custody_denials(self):
        kind = ReleaseKind(os.environ.get("BOMBAY_RELEASE_KIND", "configuration-successor"))
        candidates = TLS_SECURITY_CANDIDATES if kind is ReleaseKind.TLS_SECURITY_SUCCESSOR else CONFIGURATION_CANDIDATES
        immutable = {"zenoh-config", "zenoh-plugin-trait", "validated_struct", "validated_struct_macros"} if kind is ReleaseKind.TLS_SECURITY_SUCCESSOR else {"zenoh-link-commons", "validated_struct", "validated_struct_macros"}
        if kind is ReleaseKind.TLS_INGRESS_SUCCESSOR:
            immutable = {"zenoh-config", "zenoh-plugin-trait", "zenoh-link-commons", "validated_struct", "validated_struct_macros"}
            candidates = set(EDGES) - immutable
        self.assertEqual(published_owners(kind), immutable)
        self.assertEqual(TLS_SECURITY_CANDIDATES, {"zenoh-link-commons", "zenoh-link-tls", "zenoh-link", "zenoh-transport", "zenoh"})
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
        self.assertEqual(set(ledger), {identity(owner) for owner in candidates})
        contents, checksums = {}, {}
        for owner in EDGES:
            prefix = identity(owner) + "-" + version(owner, kind)
            archive = archives / (prefix + ".crate")
            checksum = published_dependencies(kind)[owner] if owner in immutable else ledger[identity(owner)]["checksum"]
            self.assertEqual(hashlib.sha256(archive.read_bytes()).hexdigest(), checksum)
            contents[owner] = read_archive(archive, prefix, checksum)
            with self.subTest(owner=owner), self.assertRaisesRegex(ValueError, "required archive checksum"):
                read_archive(archive, prefix, None)
            restored = read_archive(archive, prefix, checksum)
            self.assertEqual(restored, contents[owner])
            checksums[identity(owner)] = checksum
        manifest = tomllib.loads(contents["zenoh"]["Cargo.toml"].decode())
        for owner in candidates:
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
        if kind in {ReleaseKind.TLS_SECURITY_SUCCESSOR, ReleaseKind.TLS_INGRESS_SUCCESSOR}:
            for owner in ("zenoh-link-commons", "zenoh-link-tls"):
                successor = tomllib.loads(contents[owner]["Cargo.toml"].decode())
                for minimum in (None, "0.23.45"):
                    changed = copy.deepcopy(successor)
                    if minimum is None:
                        changed["dependencies"]["rustls"].pop("version")
                    else:
                        changed["dependencies"]["rustls"]["version"] = minimum
                    with self.subTest(owner=owner, minimum=minimum), self.assertRaisesRegex(ValueError, "Rustls minimum"):
                        verify_manifest(owner, changed, kind)
                verify_manifest(owner, successor, kind)
        lock = tomllib.loads(contents["zenoh"]["Cargo.lock"].decode())
        if kind in {ReleaseKind.TLS_SECURITY_SUCCESSOR, ReleaseKind.TLS_INGRESS_SUCCESSOR}:
            for owner in candidates:
                successor = tomllib.loads(contents[owner]["Cargo.lock"].decode())
                changed = copy.deepcopy(successor)
                next(package for package in changed["package"] if package["name"] == "rustls")["version"] = "0.23.45"
                with self.subTest(owner=owner), self.assertRaisesRegex(ValueError, "locked Rustls minimum"):
                    verify_lock(owner, changed, checksums, kind)
                verify_lock(owner, successor, checksums, kind)
        if kind is ReleaseKind.TLS_INGRESS_SUCCESSOR:
            for owner in EDGES:
                successor = tomllib.loads(contents[owner]["Cargo.toml"].decode())
                for alias in ("tls-listener", "retired-listener"):
                    changed = copy.deepcopy(successor)
                    changed.setdefault("dependencies", {})[alias] = {"package": "tls-listener", "version": "0.11.2"}
                    with self.subTest(owner=owner, alias=alias), self.assertRaisesRegex(ValueError, "retired listener dependency"):
                        verify_manifest(owner, changed, kind)
                verify_manifest(owner, successor, kind)
                successor_lock = tomllib.loads(contents[owner]["Cargo.lock"].decode())
                changed = copy.deepcopy(successor_lock)
                changed["package"].append({"name": "tls-listener", "version": "0.11.2",
                                           "source": "registry+https://github.com/rust-lang/crates.io-index"})
                with self.subTest(owner=owner), self.assertRaisesRegex(ValueError, "retired locked listener"):
                    verify_lock(owner, changed, checksums, kind)
                verify_lock(owner, successor_lock, checksums, kind)
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

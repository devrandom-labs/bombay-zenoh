"""Independent finite observations for the packaged query CI gates."""
from pathlib import Path
import copy
import tomllib
import unittest

from verify_archives import ReleaseKind, TLS_SECURITY_CANDIDATES, TLS_INGRESS_CANDIDATES, version

from verify_packaged_queries import QuerySourceProfile, query_release_kind, QUERY_TESTS, ROUTED_QUERY_TESTS, verify_candidate_lock, verify_query_execution, verify_query_profile


class PackagedQueryEvidence(unittest.TestCase):
    def test_hidden_features_and_zero_tests_are_refused(self):
        manifest = Path("/tmp/packaged/Cargo.toml")
        owner = {"id": "owner", "manifest_path": str(manifest), "dependencies": []}
        metadata = {"packages": [owner], "resolve": {"nodes": [{"id": "owner", "features": ["default", "transport_tls"]}]}}
        verify_query_profile(metadata, manifest)
        owner["dependencies"] = [{"name": "bombay-zenoh", "rename": "zenoh-test"}]
        with self.assertRaisesRegex(AssertionError, "self-dev alias"):
            verify_query_profile(metadata, manifest)
        owner["dependencies"] = []
        metadata["resolve"]["nodes"][0]["features"] = ["default", "internal", "transport_tls", "unstable"]
        with self.assertRaisesRegex(AssertionError, "only default TLS"):
            verify_query_profile(metadata, manifest)
        empty = "running 0 tests\ntest result: ok. 0 passed; 0 failed; 0 ignored;"
        with self.assertRaisesRegex(AssertionError, "one owning query test"):
            verify_query_execution(empty, QUERY_TESTS[0])
        healthy = "running 1 test\ntest api::session::query_identifiers::accepted_queries_keep_their_reply_consumer_at_identifier_exhaustion ... ok\ntest result: ok. 1 passed; 0 failed; 0 ignored;"
        verify_query_execution(healthy, QUERY_TESTS[0])
        for query_test in QUERY_TESTS[1:] + ROUTED_QUERY_TESTS:
            with self.assertRaisesRegex(AssertionError, "selected owning query test"):
                verify_query_execution(healthy, query_test)
            selected = healthy.replace(QUERY_TESTS[0], query_test)
            verify_query_execution(selected, query_test)
            with self.assertRaisesRegex(AssertionError, "one owning query test"):
                verify_query_execution(empty, query_test)
            failed = selected.replace(" ... ok", " ... FAILED")
            with self.assertRaisesRegex(AssertionError, "selected owning query test"):
                verify_query_execution(failed, query_test)
            skipped = selected.replace(" ... ok", " ... ignored")
            with self.assertRaisesRegex(AssertionError, "selected owning query test"):
                verify_query_execution(skipped, query_test)

    def test_candidate_source_and_lock_allow_only_the_audited_owner(self):
        manifest = Path("/tmp/packaged/Cargo.toml")
        candidate_manifest = Path("/tmp/candidate/Cargo.toml")
        owner = {"id": "owner", "manifest_path": str(manifest), "dependencies": []}
        candidate = {"id": "candidate", "name": "bombay-zenoh-config", "version": "1.10.2", "source": None, "manifest_path": str(candidate_manifest)}
        metadata = {"packages": [owner, candidate], "resolve": {"nodes": [{"id": "owner", "features": ["default", "transport_tls"]}]}}
        candidates = {"bombay-zenoh-config": candidate_manifest}
        verify_query_profile(metadata, manifest, candidates=candidates)
        with self.assertRaisesRegex(AssertionError, "actual crates.io"):
            verify_query_profile(metadata, manifest)
        for field, changed, diagnostic in [("version", "1.10.1", "version changed"), ("manifest_path", "/tmp/unreviewed/Cargo.toml", "archive owner changed"), ("source", "registry+https://github.com/rust-lang/crates.io-index", "audited archive")]:
            original = candidate[field]
            candidate[field] = changed
            with self.subTest(field=field), self.assertRaisesRegex(AssertionError, diagnostic):
                verify_query_profile(metadata, manifest, candidates=candidates)
            candidate[field] = original
        original_lock = b'version = 4\n[[package]]\nname = "bombay-zenoh-config"\nversion = "1.10.2"\nsource = "registry+https://github.com/rust-lang/crates.io-index"\nchecksum = "reviewed"\n'
        execution = {"version": 4, "package": [{"name": "bombay-zenoh-config", "version": "1.10.2"}]}
        verify_candidate_lock(original_lock, execution, candidates)
        execution["package"][0]["version"] = "1.10.1"
        with self.assertRaisesRegex(AssertionError, "unrelated records"):
            verify_candidate_lock(original_lock, execution, candidates)
        execution["package"][0]["version"] = "1.10.2"
        verify_candidate_lock(original_lock, execution, candidates)

    def test_cargo_format_upgrade_preserves_every_other_lock_fact(self):
        original = b'version = 3\n[[package]]\nname = "bombay-zenoh-config"\nversion = "1.10.2"\nsource = "registry+https://github.com/rust-lang/crates.io-index"\nchecksum = "reviewed"\ndependencies = ["serde 1.0.228"]\n[[package]]\nname = "serde"\nversion = "1.0.228"\nsource = "registry+https://github.com/rust-lang/crates.io-index"\nchecksum = "unchanged"\n'
        execution = {"version": 4, "package": [
            {"name": "bombay-zenoh-config", "version": "1.10.2", "dependencies": ["serde 1.0.228"]},
            {"name": "serde", "version": "1.0.228", "source": "registry+https://github.com/rust-lang/crates.io-index", "checksum": "unchanged"},
        ]}
        candidates = {"bombay-zenoh-config": Path("/tmp/candidate/Cargo.toml")}
        verify_candidate_lock(original, execution, candidates)
        for format_version in (None, 1, 2, 5):
            changed = copy.deepcopy(execution)
            if format_version is None:
                changed.pop("version")
            else:
                changed["version"] = format_version
            with self.subTest(format=format_version), self.assertRaisesRegex(AssertionError, "unrelated records"):
                verify_candidate_lock(original, changed, candidates)
        for field, value in (("version", "1.0.229"), ("source", "git+https://unreviewed.invalid"), ("checksum", "changed"), ("dependencies", ["unreviewed"])):
            changed = copy.deepcopy(execution)
            changed["package"][1][field] = value
            with self.subTest(field=field), self.assertRaisesRegex(AssertionError, "unrelated records"):
                verify_candidate_lock(original, changed, candidates)
        changed = copy.deepcopy(execution)
        changed["metadata"] = {"unreviewed": "value"}
        with self.assertRaisesRegex(AssertionError, "unrelated records"):
            verify_candidate_lock(original, changed, candidates)
        execution["version"] = 3
        verify_candidate_lock(original, execution, candidates)

    def test_security_candidates_preserve_versions_original_locks_and_registry_edges(self):
        for kind in (ReleaseKind.TLS_SECURITY_SUCCESSOR, ReleaseKind.TLS_INGRESS_SUCCESSOR):
            manifest = Path("/tmp/packaged/Cargo.toml")
            owner = {"id": "owner", "manifest_path": str(manifest), "dependencies": []}
            expected_versions = {"zenoh-link-commons": "1.10.2", "zenoh-link-tls": "1.10.3",
                                 "zenoh-link": "1.10.3", "zenoh-transport": "1.10.3"}
            if kind is ReleaseKind.TLS_INGRESS_SUCCESSOR:
                expected_versions = {"zenoh-link-tls": "1.10.4", "zenoh-link": "1.10.4", "zenoh-transport": "1.10.4"}
            selected = TLS_INGRESS_CANDIDATES if kind is ReleaseKind.TLS_INGRESS_SUCCESSOR else TLS_SECURITY_CANDIDATES
            self.assertEqual(selected - {"zenoh"}, set(expected_versions))
            sdk_version = version("zenoh", kind)
            self.assertEqual(sdk_version, "1.10.10" if kind is ReleaseKind.TLS_INGRESS_SUCCESSOR else "1.10.9")
            packages = [{"id": name, "name": "bombay-" + name, "version": expected,
                         "source": None, "manifest_path": "/tmp/" + name + "/Cargo.toml"}
                        for name, expected in sorted(expected_versions.items())]
            registry = {"id": "config", "name": "bombay-zenoh-config", "version": "1.10.2",
                        "source": "registry+https://github.com/rust-lang/crates.io-index"}
            rustls = {"id": "rustls", "name": "rustls", "version": "0.23.46", "source": registry["source"]}
            metadata = {"packages": [owner, *packages, registry, rustls],
                        "resolve": {"nodes": [{"id": "owner", "features": ["default", "transport_tls"]}]}}
            if kind is ReleaseKind.TLS_INGRESS_SUCCESSOR:
                metadata["packages"].append({"id": "commons", "name": "bombay-zenoh-link-commons", "version": "1.10.2", "source": registry["source"]})
            candidates = {package["name"]: Path(package["manifest_path"]) for package in packages}
            verify_query_profile(metadata, manifest, candidates=candidates, release_kind=kind)
            for package in packages:
                original = package["version"]
                package["version"] = "1.10.1"
                with self.subTest(owner=package["name"]), self.assertRaisesRegex(AssertionError, "candidate dependency version"):
                    verify_query_profile(metadata, manifest, candidates=candidates, release_kind=kind)
                package["version"] = original
            registry["source"] = None
            with self.assertRaisesRegex(AssertionError, "actual crates.io"):
                verify_query_profile(metadata, manifest, candidates=candidates, release_kind=kind)
            registry["source"] = rustls["source"]
            rustls["version"] = "0.23.45"
            with self.assertRaisesRegex(AssertionError, "packaged Rustls minimum"):
                verify_query_profile(metadata, manifest, candidates=candidates, release_kind=kind)
            rustls["version"] = "0.23.46"
            verify_query_profile(metadata, manifest, candidates=candidates, release_kind=kind)
            original = ('version = 3\n' + ''.join('[[package]]\nname = "' + package["name"] +
                        '"\nversion = "' + package["version"] + '"\nsource = "' + rustls["source"] +
                        '"\nchecksum = "reviewed"\n' for package in packages + [registry, rustls])).encode()
            execution = tomllib.loads(original.decode())
            execution["version"] = 4
            for package in execution["package"]:
                if package["name"] in candidates:
                    package.pop("source")
                    package.pop("checksum")
            verify_candidate_lock(original, execution, candidates, kind)
            changed = copy.deepcopy(execution)
            next(package for package in changed["package"] if package["name"] == "bombay-zenoh-config").pop("checksum")
            with self.assertRaisesRegex(AssertionError, "unrelated records"):
                verify_candidate_lock(original, changed, candidates, kind)
            verify_candidate_lock(original, execution, candidates, kind)

    def test_source_profiles_refuse_conflicting_release_kinds(self):
        registry_kind = query_release_kind(QuerySourceProfile.REGISTRY, None)
        self.assertIs(registry_kind, ReleaseKind.TLS_INGRESS_SUCCESSOR)
        for profile, expected in ((QuerySourceProfile.CONFIGURATION_COHORT, ReleaseKind.CONFIGURATION_SUCCESSOR),
                                  (QuerySourceProfile.TLS_SECURITY_COHORT, ReleaseKind.TLS_SECURITY_SUCCESSOR),
                                  (QuerySourceProfile.TLS_INGRESS_COHORT, ReleaseKind.TLS_INGRESS_SUCCESSOR)):
            default_kind = query_release_kind(profile, None)
            self.assertIs(default_kind, expected)
            for kind in ReleaseKind:
                if kind is expected:
                    selected_kind = query_release_kind(profile, kind)
                    self.assertIs(selected_kind, expected)
                else:
                    with self.subTest(profile=profile, kind=kind), self.assertRaisesRegex(AssertionError, "profile and release kind conflict"):
                        query_release_kind(profile, kind)
        for kind in ReleaseKind:
            registry_kind = query_release_kind(QuerySourceProfile.REGISTRY, kind)
            self.assertIs(registry_kind, kind)

    def test_maintainer_profile_preserves_registry_and_alias_denials(self):
        manifest = Path("/tmp/packaged/Cargo.toml")
        owner = {"id": "owner", "manifest_path": str(manifest), "dependencies": []}
        dependency = {"id": "transport", "source": "registry+https://github.com/rust-lang/crates.io-index"}
        metadata = {"packages": [owner, dependency], "resolve": {"nodes": [{"id": "owner", "features": ["test", "transport_tls"]}]}}
        verify_query_profile(metadata, manifest, ("test", "transport_tls"))
        for features in [["default", "transport_tls"], ["internal", "test", "transport_tls"], ["test", "transport_tls", "unstable"]]:
            metadata["resolve"]["nodes"][0]["features"] = features
            with self.assertRaisesRegex(AssertionError, "only default TLS"):
                verify_query_profile(metadata, manifest, ("test", "transport_tls"))
        metadata["resolve"]["nodes"][0]["features"] = ["test", "transport_tls"]
        dependency["source"] = None
        with self.assertRaisesRegex(AssertionError, "actual crates.io"):
            verify_query_profile(metadata, manifest, ("test", "transport_tls"))
        dependency["source"] = "registry+https://github.com/rust-lang/crates.io-index"
        owner["dependencies"] = [{"name": "bombay-zenoh", "rename": "zenoh-test"}]
        with self.assertRaisesRegex(AssertionError, "self-dev alias"):
            verify_query_profile(metadata, manifest, ("test", "transport_tls"))


if __name__ == "__main__":
    unittest.main()

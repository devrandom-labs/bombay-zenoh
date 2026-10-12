"""Independent finite observations for the packaged query CI gates."""
from pathlib import Path
import unittest

from verify_packaged_queries import QUERY_TESTS, ROUTED_QUERY_TESTS, verify_query_execution, verify_query_profile


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

"""Independent finite observations for the packaged query CI gates."""
from pathlib import Path
import unittest

from verify_packaged_queries import QUERY_TESTS, verify_query_execution, verify_query_profile


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
        for query_test in QUERY_TESTS[1:]:
            with self.assertRaisesRegex(AssertionError, "selected owning query test"):
                verify_query_execution(healthy, query_test)
            selected = healthy.replace(QUERY_TESTS[0], query_test)
            verify_query_execution(selected, query_test)
            with self.assertRaisesRegex(AssertionError, "one owning query test"):
                verify_query_execution(empty, query_test)
            failed = selected.replace(" ... ok", " ... FAILED")
            with self.assertRaisesRegex(AssertionError, "selected owning query test"):
                verify_query_execution(failed, query_test)


if __name__ == "__main__":
    unittest.main()

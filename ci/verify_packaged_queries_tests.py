"""Independent finite observations for the packaged query CI gates."""
from pathlib import Path
import unittest

from verify_packaged_queries import verify_query_execution, verify_query_profile


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
            verify_query_execution(empty)
        healthy = "running 1 test\ntest api::session::query_identifiers::accepted_queries_keep_their_reply_consumer_at_identifier_exhaustion ... ok\ntest result: ok. 1 passed; 0 failed; 0 ignored;"
        verify_query_execution(healthy)


if __name__ == "__main__":
    unittest.main()

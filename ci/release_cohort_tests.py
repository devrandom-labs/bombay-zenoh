"""Observable custody denials for the fixed partial publication."""

import copy
import json
import tempfile
import unittest
from pathlib import Path

import release_cohort as cohort


class PartialPublicationTests(unittest.TestCase):
    def setUp(self):
        self.archives = {name: {"version": "1.10.1", "checksum": name}
                         for name in cohort.PUBLISHED | cohort.REMAINING}
        self.receipts = {name: archive if name in cohort.PUBLISHED else {"status": "absent", "http": 404}
                         for name, archive in self.archives.items()}
        self.origin = {"id": cohort.ORIGINAL_RUN, "head_sha": cohort.SOURCE,
                       "event": "workflow_dispatch", "path": ".github/workflows/publish.yml",
                       "head_branch": "main", "status": "completed", "conclusion": "failure"}

    def test_origin_requires_exact_retained_operation(self):
        cohort.verify_origin(self.origin)
        for field, replacement in {"id": 1, "head_sha": "different", "event": "push",
                                   "path": "other", "head_branch": "other", "status": "in_progress",
                                   "conclusion": "cancelled"}.items():
            altered = {**self.origin, field: replacement}
            with self.subTest(field=field), self.assertRaisesRegex(ValueError, "original release origin"):
                cohort.verify_origin(altered)
        cohort.verify_origin(self.origin)

    def test_exact_prefix_and_missing_selection(self):
        cohort.verify_receipts(self.archives, self.receipts, "before")
        for name in cohort.PUBLISHED:
            changed = copy.deepcopy(self.receipts)
            changed[name]["checksum"] = "different"
            with self.assertRaisesRegex(ValueError, "published prefix custody"):
                cohort.verify_receipts(self.archives, changed, "before")
        for name in cohort.REMAINING:
            changed = {**self.receipts, name: self.archives[name]}
            with self.assertRaisesRegex(ValueError, "remaining selection"):
                cohort.verify_receipts(self.archives, changed, "before")
        for changed in (dict(list(self.archives.items())[1:]), {**self.archives, "extra": {}}):
            with self.assertRaisesRegex(ValueError, "exact cohort"):
                cohort.verify_receipts(changed, self.receipts, "before")
        cohort.verify_receipts(self.archives, self.receipts, "before")
        cohort.verify_receipts(self.archives, self.archives, "after")
        with self.assertRaisesRegex(ValueError, "publication not established"):
            cohort.verify_receipts(self.archives, self.receipts, "after")

    def test_remaining_candidate_bytes_and_restricted_authority(self):
        with tempfile.TemporaryDirectory() as temporary:
            directory = Path(temporary)
            archives = copy.deepcopy(self.archives)
            for name in cohort.REMAINING:
                body = name.encode()
                archives[name]["checksum"] = cohort.hashlib.sha256(body).hexdigest()
                (directory / f"{name}-1.10.1.crate").write_bytes(body)
            selected = cohort.verify_candidates(archives, directory)
            self.assertEqual(set(selected), cohort.REMAINING)
            name = sorted(cohort.REMAINING)[0]
            path = directory / f"{name}-1.10.1.crate"
            original = path.read_bytes()
            path.write_bytes(b"changed")
            with self.assertRaisesRegex(ValueError, "remaining candidate custody"):
                cohort.verify_candidates(archives, directory)
            path.write_bytes(original)
            restored = cohort.verify_candidates(archives, directory)
            self.assertEqual(restored, selected)

    def test_original_ledger_bytes(self):
        with tempfile.TemporaryDirectory() as temporary:
            path = Path(temporary) / "audited-cohort.json"
            path.write_text(json.dumps(self.archives))
            with self.assertRaisesRegex(ValueError, "original cohort ledger"):
                cohort.read_original_cohort(path)


if __name__ == "__main__":
    unittest.main()

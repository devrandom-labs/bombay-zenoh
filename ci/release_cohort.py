"""Custody of the one retained partial registry publication."""

import argparse
import hashlib
import json
import os
from pathlib import Path
from urllib.error import HTTPError, URLError
from urllib.request import Request, urlopen

SOURCE = "e82481825e313ff14e5ea3a1d6e842040945372a"
ORIGINAL_RUN = 38028659029
LEDGER_SHA256 = "7de1e464c87156b91c735b086d886324d2d5bbbe5c28ef3c4c6229ed17962d4f"
PUBLISHED = {"bombay-validated-struct-macros", "bombay-validated-struct",
             "bombay-zenoh-link-commons", "bombay-zenoh-config", "bombay-zenoh-link-tls"}
REMAINING = {"bombay-zenoh-plugin-trait", "bombay-zenoh-link", "bombay-zenoh-transport", "bombay-zenoh"}


def verify_origin(origin):
    expected = {"id": ORIGINAL_RUN, "head_sha": SOURCE, "event": "workflow_dispatch",
                "path": ".github/workflows/publish.yml", "head_branch": "main",
                "status": "completed", "conclusion": "failure"}
    if any(origin.get(field) != value for field, value in expected.items()):
        raise ValueError("original release origin")


def read_original_cohort(path):
    body = path.read_bytes()
    if hashlib.sha256(body).hexdigest() != LEDGER_SHA256:
        raise ValueError("original cohort ledger")
    cohort = json.loads(body)
    if set(cohort) != PUBLISHED | REMAINING:
        raise ValueError("exact cohort")
    return cohort


def verify_receipts(cohort, receipts, phase):
    if set(cohort) != PUBLISHED | REMAINING or set(receipts) != set(cohort):
        raise ValueError("exact cohort")
    for name, archive in cohort.items():
        if name in PUBLISHED and receipts[name] != archive:
            raise ValueError(f"published prefix custody: {name}")
        if phase == "before" and name in REMAINING and receipts[name] != {"status": "absent", "http": 404}:
            raise ValueError(f"remaining selection: {name}")
        if phase == "after" and receipts[name] != archive:
            raise ValueError(f"publication not established: {name}")


def verify_candidates(cohort, directory):
    selected = {}
    for name in sorted(REMAINING):
        archive = cohort[name]
        body = (directory / f"{name}-{archive['version']}.crate").read_bytes()
        if hashlib.sha256(body).hexdigest() != archive["checksum"]:
            raise ValueError(f"remaining candidate custody: {name}")
        selected[name] = archive
    return selected


def record_registry_receipts(cohort, directory, phase):
    path = directory / "registry-receipts.json"
    recorded = json.loads(path.read_text()) if path.exists() else {"source": SOURCE, "original_run": ORIGINAL_RUN}
    recorded[phase] = {}
    for name, archive in sorted(cohort.items()):
        request = Request(f"https://crates.io/api/v1/crates/{name}/{archive['version']}",
                          headers={"User-Agent": "Bombay controlled partial publication"})
        try:
            with urlopen(request, timeout=30) as reply:
                published = json.load(reply)["version"]
            receipt = {"version": published["num"], "checksum": published["checksum"]}
        except HTTPError as error:
            receipt = {"status": "absent" if error.code == 404 else "unavailable", "http": error.code}
        except (URLError, TimeoutError, KeyError, ValueError):
            receipt = {"status": "unavailable"}
        recorded[phase][name] = receipt
        path.write_text(json.dumps(recorded, indent=2, sort_keys=True) + "\n")
        summary = f"{name}: {json.dumps(receipt, sort_keys=True)}; source {SOURCE}"
        print(summary)
        with open(os.environ["GITHUB_STEP_SUMMARY"], "a") as report:
            report.write(summary + "\n")
    verify_receipts(cohort, recorded[phase], phase)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("operation", choices=("origin", "candidates", "before", "after"))
    parser.add_argument("retained", type=Path)
    parser.add_argument("candidates", type=Path)
    arguments = parser.parse_args()
    cohort = read_original_cohort(arguments.retained / "audited-cohort.json")
    if arguments.operation == "origin":
        verify_origin(json.loads((arguments.retained / "run.json").read_text()))
        previous = json.loads((arguments.retained / "registry-receipts.json").read_text())
        if previous["source"] != SOURCE:
            raise ValueError("original receipt source")
        verify_receipts(cohort, previous["after"], "before")
    elif arguments.operation == "candidates":
        selected = verify_candidates(cohort, arguments.candidates)
        (arguments.candidates.parent / "remaining-cohort.json").write_text(json.dumps(selected, indent=2, sort_keys=True) + "\n")
    else:
        record_registry_receipts(cohort, arguments.candidates.parent, arguments.operation)


if __name__ == "__main__":
    main()

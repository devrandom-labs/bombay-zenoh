"""Exercise the actual packaged query owner without workspace feature unification."""
import json
import os
from pathlib import Path
import subprocess
from tempfile import TemporaryDirectory
import tomllib

from verify_archives import read_archive

QUERY_TEST = "api::session::query_identifiers::accepted_queries_keep_their_reply_consumer_at_identifier_exhaustion"


def verify_query_profile(metadata, manifest):
    owner = next(package for package in metadata["packages"] if package["manifest_path"] == str(manifest))
    node = next(node for node in metadata["resolve"]["nodes"] if node["id"] == owner["id"])
    assert node["features"] == ["default", "transport_tls"], "packaged query requires only default TLS features"
    assert all((dependency.get("rename") or dependency["name"]) != "zenoh-test" for dependency in owner["dependencies"]), "workspace self-dev alias escaped packaging"
    assert all(package["source"] == "registry+https://github.com/rust-lang/crates.io-index" for package in metadata["packages"] if package["id"] != owner["id"]), "packaged dependencies must be actual crates.io packages"


def verify_query_execution(output):
    assert output.count("running 1 test") == 1, "one owning query test must execute"
    assert f"test {QUERY_TEST} ... ok" in output, "the selected owning query test must pass"
    assert "test result: ok. 1 passed; 0 failed; 0 ignored;" in output, "the owning query test cannot be skipped"


def main():
    version = tomllib.loads(Path("zenoh/Cargo.toml").read_text())["package"]["version"]
    prefix = "bombay-zenoh-" + version
    selected = json.loads(Path("target/package/audited-cohort.json").read_text())["bombay-zenoh"]
    assert selected["version"] == version, "audited query package version changed"
    assert isinstance(selected["checksum"], str), "audited query package checksum must be present"
    contents = read_archive(Path("target/package") / (prefix + ".crate"), prefix, selected["checksum"])
    environment = os.environ | {"CARGO_TARGET_DIR": str(Path(os.environ.get("CARGO_TARGET_DIR", "target")).resolve())}
    with TemporaryDirectory(prefix="bombay-packaged-query-") as temporary:
        source = Path(temporary)
        for name, payload in contents.items():
            destination = source / name
            destination.parent.mkdir(parents=True, exist_ok=True)
            destination.write_bytes(payload)
        manifest = source / "Cargo.toml"
        custody = {name: (source / name).read_bytes() for name in ["Cargo.toml", "Cargo.lock"]}
        Path("target/package/packaged-query-manifest.toml").write_bytes(custody["Cargo.toml"])
        Path("target/package/packaged-query-Cargo.lock").write_bytes(custody["Cargo.lock"])
        arguments = ["cargo", "metadata", "--manifest-path", str(manifest), "--locked", "--format-version", "1"]
        metadata = subprocess.run(arguments, env=environment, capture_output=True, text=True)
        Path("target/package/packaged-query-metadata.stdout").write_text(metadata.stdout)
        Path("target/package/packaged-query-metadata.stderr").write_text(metadata.stderr)
        metadata.check_returncode()
        verify_query_profile(json.loads(metadata.stdout), manifest)
        for profile in ["debug", "release"]:
            arguments = ["cargo", "test", "--manifest-path", str(manifest), "--locked", "--jobs", "2", "--lib"]
            if profile == "release":
                arguments.append("--release")
            result = subprocess.run(arguments + [QUERY_TEST, "--", "--exact", "--nocapture"], env=environment, capture_output=True, text=True)
            output = result.stdout + result.stderr
            Path("target/package", "packaged-query-" + profile + ".log").write_text(output)
            print(output, flush=True)
            result.check_returncode()
            verify_query_execution(output)
            assert all((source / name).read_bytes() == payload for name, payload in custody.items()), "packaged manifest/lock custody changed"


if __name__ == "__main__":
    main()

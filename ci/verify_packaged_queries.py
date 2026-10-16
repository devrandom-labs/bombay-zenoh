"""Exercise the actual packaged query owner without workspace feature unification."""
import argparse
import enum
import hashlib
import json
import os
from pathlib import Path
import subprocess
from tempfile import TemporaryDirectory
import tomllib

from verify_archives import EDGES, identity, published_dependencies, published_owners, read_archive, version, ReleaseKind

QUERY_TESTS = (
    "api::session::query_identifiers::accepted_queries_keep_their_reply_consumer_at_identifier_exhaustion",
    "api::session::query_retirement::completed_queries_release_their_timeout_session_owners",
    "api::session::query_retirement::retired_queriers_release_timeouts_while_native_queries_are_retained",
    "api::session::query_retirement::completed_queries_retain_timeout_owners_before_net_can_poll",
    "api::session::query_retirement::retained_query_capacity_waits_for_native_timeout_destruction",
    "api::session::query_retirement::rejected_query_keeps_capacity_through_callback_destruction",
    "api::session::query_retirement::aborted_timeout_keeps_the_live_query_capacity_reserved",
    "api::session::query_retirement::timeout_callback_unwind_destroys_originals_before_capacity_reuse",
    "api::session::query_retirement::oversized_direct_config_refuses_before_runtime_construction",
    "api::session::query_retirement::invalid_advanced_init_preserves_its_original_runtime",
    "api::session::query_retirement::closed_query_capacity_preserves_native_refusal_precedence",
    "api::session::query_retirement::active_reply_callback_retains_its_original_query_capacity",
    "api::session::query_retirement::active_error_reply_callback_retains_its_original_query_capacity",
    "api::session::query_retirement::active_first_monotonic_reply_callback_retains_its_original_query_capacity",
    "api::session::query_retirement::active_newer_monotonic_reply_callback_retains_its_original_query_capacity",
    "api::session::query_retirement::active_latest_reply_callback_retains_its_original_query_capacity",
)
ROUTED_QUERY_TESTS = (
    "net::tests::regions::forwarding::routed_queries_keep_their_original_consumers_at_identifier_exhaustion",
    "net::tests::regions::forwarding::routed_queries_issue_the_last_identifier_once_and_preserve_target_selection",
)


class QuerySourceProfile(enum.Enum):
    REGISTRY = "registry"
    CONFIGURATION_COHORT = "configuration-cohort"
    TLS_SECURITY_COHORT = "tls-security-cohort"
    TLS_INGRESS_COHORT = "tls-ingress-cohort"


def query_release_kind(source_profile, selected_kind):
    allowed = {
        QuerySourceProfile.REGISTRY: (ReleaseKind.INITIAL_COHORT, ReleaseKind.ZENOH_SUCCESSOR,
                                     ReleaseKind.CONFIGURATION_SUCCESSOR, ReleaseKind.TLS_SECURITY_SUCCESSOR,
                                     ReleaseKind.TLS_INGRESS_SUCCESSOR),
        QuerySourceProfile.CONFIGURATION_COHORT: (ReleaseKind.CONFIGURATION_SUCCESSOR,),
        QuerySourceProfile.TLS_SECURITY_COHORT: (ReleaseKind.TLS_SECURITY_SUCCESSOR,),
        QuerySourceProfile.TLS_INGRESS_COHORT: (ReleaseKind.TLS_INGRESS_SUCCESSOR,),
    }
    kind = allowed[source_profile][-1] if selected_kind is None else selected_kind
    assert kind in allowed[source_profile], "query source profile and release kind conflict"
    return kind


def verify_query_profile(metadata, manifest, features=("default", "transport_tls"), candidates=None, release_kind=ReleaseKind.CONFIGURATION_SUCCESSOR):
    owner = next(package for package in metadata["packages"] if package["manifest_path"] == str(manifest))
    node = next(node for node in metadata["resolve"]["nodes"] if node["id"] == owner["id"])
    assert node["features"] == list(features), "packaged query requires only default TLS or explicit maintainer TLS,test features"
    assert all((dependency.get("rename") or dependency["name"]) != "zenoh-test" for dependency in owner["dependencies"]), "workspace self-dev alias escaped packaging"
    candidates = {} if candidates is None else candidates
    observed_candidates = set()
    for package in metadata["packages"]:
        if package["id"] == owner["id"]:
            continue
        if package.get("name") in candidates:
            assert package["source"] is None, "candidate dependency must use its audited archive"
            assert package["version"] == version(package["name"].removeprefix("bombay-"), release_kind), "candidate dependency version changed"
            assert package["manifest_path"] == str(candidates[package["name"]]), "candidate archive owner changed"
            assert package["name"] not in observed_candidates, "duplicate candidate dependency"
            observed_candidates.add(package["name"])
        else:
            assert package["source"] == "registry+https://github.com/rust-lang/crates.io-index", "packaged dependencies must be actual crates.io packages"
    assert observed_candidates == set(candidates), "candidate dependency closure changed"
    if release_kind in {ReleaseKind.TLS_SECURITY_SUCCESSOR, ReleaseKind.TLS_INGRESS_SUCCESSOR}:
        rustls = [package for package in metadata["packages"] if package.get("name") == "rustls"]
        assert len(rustls) == 1 and rustls[0]["version"] == "0.23.46", "packaged Rustls minimum changed"


def verify_query_execution(output, query_test):
    assert output.count("running 1 test") == 1, "one owning query test must execute"
    assert f"test {query_test} ... ok" in output, "the selected owning query test must pass"
    assert "test result: ok. 1 passed; 0 failed; 0 ignored;" in output, "the owning query test cannot be skipped"


def verify_candidate_lock(original, execution, candidates, release_kind=ReleaseKind.CONFIGURATION_SUCCESSOR):
    expected = tomllib.loads(original.decode())
    if expected["version"] == 3 and execution.get("version") == 4:
        expected["version"] = 4
    for package in expected["package"]:
        if package["name"] in candidates:
            assert package["version"] == version(package["name"].removeprefix("bombay-"), release_kind), "archived candidate version changed"
            registry_source = package.pop("source")
            assert registry_source == "registry+https://github.com/rust-lang/crates.io-index", "archived candidate registry changed"
            package.pop("checksum")
    assert execution == expected, "candidate execution lock changed unrelated records"


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--source-profile", choices=[profile.value for profile in QuerySourceProfile], default=QuerySourceProfile.REGISTRY.value)
    parser.add_argument("--archives", type=Path)
    parser.add_argument("--release-kind", choices=[kind.value for kind in ReleaseKind])
    options = parser.parse_args()
    source_profile = QuerySourceProfile(options.source_profile)
    release_kind = query_release_kind(source_profile, ReleaseKind(options.release_kind) if options.release_kind else None)
    archives = options.archives or (Path("target/package") if source_profile is QuerySourceProfile.REGISTRY else Path("target/package/tmp-crate"))
    selected_version = tomllib.loads(Path("zenoh/Cargo.toml").read_text())["package"]["version"]
    assert selected_version == version("zenoh", release_kind), "query release kind and source version conflict"
    prefix = "bombay-zenoh-" + selected_version
    selected = json.loads(Path("target/package/audited-cohort.json").read_text())["bombay-zenoh"]
    assert selected["version"] == selected_version, "audited query package version changed"
    assert isinstance(selected["checksum"], str), "audited query package checksum must be present"
    contents = read_archive(archives / (prefix + ".crate"), prefix, selected["checksum"])
    environment = os.environ | {"CARGO_TARGET_DIR": str(Path(os.environ.get("CARGO_TARGET_DIR", "target")).resolve())}
    with TemporaryDirectory(prefix="bombay-packaged-query-") as temporary:
        source = Path(temporary)
        for name, payload in contents.items():
            destination = source / name
            destination.parent.mkdir(parents=True, exist_ok=True)
            destination.write_bytes(payload)
        manifest = source / "Cargo.toml"
        candidates, candidate_arguments, candidate_custody = {}, [], {}
        if source_profile is not QuerySourceProfile.REGISTRY:
            ledger = json.loads(Path("target/package/audited-cohort.json").read_text())
            # Keep all extracted owning archives within this same temporary lifetime.
            dependency_root = source / "candidate-dependencies"
            owners = set(EDGES) - published_owners(release_kind)
            for owner in sorted(owners - {"zenoh"}):
                package = identity(owner)
                candidate_prefix = package + "-" + version(owner, release_kind)
                assert ledger[package]["version"] == version(owner, release_kind), "candidate ledger version changed"
                candidate = read_archive(archives / (candidate_prefix + ".crate"), candidate_prefix, ledger[package]["checksum"])
                directory = dependency_root / package
                for name, payload in candidate.items():
                    destination = directory / name
                    destination.parent.mkdir(parents=True, exist_ok=True)
                    destination.write_bytes(payload)
                    candidate_custody[destination] = payload
                candidates[package] = directory / "Cargo.toml"
            configuration = source / "candidate-patches.toml"
            configuration.write_text("[patch.crates-io]\n" + "".join(
                json.dumps(package) + " = { path = " + json.dumps(str(path.parent)) + " }\n"
                for package, path in sorted(candidates.items())))
            candidate_arguments = ["--config", str(configuration)]
        archive_custody = contents.copy()
        execution_custody = archive_custody.copy()
        Path("target/package/packaged-query-manifest.toml").write_bytes(archive_custody["Cargo.toml"])
        Path("target/package/packaged-query-Cargo.lock").write_bytes(archive_custody["Cargo.lock"])
        if candidates:
            arguments = ["cargo", "metadata", "--manifest-path", str(manifest), "--format-version", "1"] + candidate_arguments
            resolved = subprocess.run(arguments, env=environment, capture_output=True, text=True)
            Path("target/package/packaged-query-candidate-resolve.stdout").write_text(resolved.stdout)
            Path("target/package/packaged-query-candidate-resolve.stderr").write_text(resolved.stderr)
            resolved.check_returncode()
            Path("target/package/packaged-query-candidate-Cargo.lock").write_bytes((source / "Cargo.lock").read_bytes())
            verify_candidate_lock(archive_custody["Cargo.lock"], tomllib.loads((source / "Cargo.lock").read_text()), candidates, release_kind)
            verify_query_profile(json.loads(resolved.stdout), manifest, candidates=candidates, release_kind=release_kind)
            execution_custody["Cargo.lock"] = (source / "Cargo.lock").read_bytes()
        for profile_prefix, features, feature_arguments, query_tests in (
            ("", ("default", "transport_tls"), [], QUERY_TESTS),
            ("maintainer-", ("test", "transport_tls"), ["--no-default-features", "--features", "transport_tls,test"], ROUTED_QUERY_TESTS),
        ):
            arguments = ["cargo", "metadata", "--manifest-path", str(manifest), "--locked", "--format-version", "1"] + feature_arguments + candidate_arguments
            metadata = subprocess.run(arguments, env=environment, capture_output=True, text=True)
            Path("target/package", "packaged-query-" + profile_prefix + "metadata.stdout").write_text(metadata.stdout)
            Path("target/package", "packaged-query-" + profile_prefix + "metadata.stderr").write_text(metadata.stderr)
            metadata.check_returncode()
            query_metadata = json.loads(metadata.stdout)
            verify_query_profile(query_metadata, manifest, features, candidates, release_kind)
            transport = next(package for package in query_metadata["packages"] if package["name"] == "bombay-zenoh-transport")
            transport_node = next(node for node in query_metadata["resolve"]["nodes"] if node["id"] == transport["id"])
            Path("target/package", "packaged-query-" + profile_prefix + "transport-features.json").write_text(json.dumps(transport_node, indent=2) + "\n")
            assert all((source / name).read_bytes() == payload for name, payload in execution_custody.items()), "packaged manifest/lock custody changed"
            assert all(path.read_bytes() == payload for path, payload in candidate_custody.items()), "candidate archive source custody changed"
            for profile in ["debug", "release"]:
                arguments = ["cargo", "test", "--manifest-path", str(manifest), "--locked", "--jobs", "2", "--lib"] + feature_arguments + candidate_arguments
                if profile == "release":
                    arguments.append("--release")
                for query_test in query_tests:
                    result = subprocess.run(arguments + [query_test, "--", "--exact", "--nocapture"], env=environment, capture_output=True, text=True)
                    output = result.stdout + result.stderr
                    Path("target/package", "packaged-query-" + profile_prefix + profile + "-" + query_test.rsplit("::", 1)[-1] + ".log").write_text(output)
                    print(output, flush=True)
                    result.check_returncode()
                    verify_query_execution(output, query_test)
                    assert all((source / name).read_bytes() == payload for name, payload in execution_custody.items()), "packaged manifest/lock custody changed"
                    assert all(path.read_bytes() == payload for path, payload in candidate_custody.items()), "candidate archive source custody changed"
        if candidates:
            configuration = candidates.get("bombay-zenoh-config")
            if configuration is None:
                prefix = "bombay-zenoh-config-" + version("zenoh-config", release_kind)
                immutable = read_archive(archives / (prefix + ".crate"), prefix, published_dependencies(release_kind)["zenoh-config"])
                directory = source / "immutable-configuration"
                for name, payload in immutable.items():
                    destination = directory / name
                    destination.parent.mkdir(parents=True, exist_ok=True)
                    destination.write_bytes(payload)
                    candidate_custody[destination] = payload
                configuration = directory / "Cargo.toml"
            corpus = Path(os.environ["BOMBAY_YAML_SOURCE_CORPUS"])
            corpus_hash = hashlib.sha256(corpus.read_bytes()).hexdigest()
            for enabled, feature_arguments, configuration_tests in (
                ([], [], (
                    "tests::yaml_files_preserve_complete_deployment_configuration",
                    "tests::yaml_validation_preserves_complete_configuration_outcomes",
                    "tests::yaml_configuration_preserves_scalar_alias_and_enum_inputs",
                    "tests::yaml_configuration_preserves_refusals_and_bounded_depth",
                    "include::yaml_include_tests::yaml_includes_preserve_complete_overrides_and_relative_paths",
                    "include::yaml_include_tests::yaml_include_values_preserve_scalars_duplicates_and_literal_merges",
                    "include::yaml_include_tests::yaml_includes_preserve_structural_io_cycle_and_extension_refusals",
                    "retained_query_limits_preserve_validated_inputs",
                    "tests::yaml_flow_document_preserves_the_actual_configuration_corpus",
                )),
                (["unstable"], ["--features", "unstable"], ("tests::toml_files_preserve_complete_configuration",)),
            ):
                arguments = ["cargo", "metadata", "--manifest-path", str(configuration), "--locked", "--format-version", "1"] + feature_arguments
                metadata = subprocess.run(arguments, env=environment, capture_output=True, text=True)
                Path("target/package", "packaged-config-" + ("unstable" if enabled else "default") + "-metadata.log").write_text(metadata.stdout + metadata.stderr)
                metadata.check_returncode()
                configuration_metadata = json.loads(metadata.stdout)
                owner = next(package for package in configuration_metadata["packages"] if package["manifest_path"] == str(configuration))
                node = next(node for node in configuration_metadata["resolve"]["nodes"] if node["id"] == owner["id"])
                assert node["features"] == enabled, "packaged Config feature profile changed"
                assert all(package["source"] == "registry+https://github.com/rust-lang/crates.io-index" for package in configuration_metadata["packages"] if package["id"] != owner["id"]), "packaged Config dependencies must remain registry-only"
                for profile in ["debug", "release"]:
                    for configuration_test in configuration_tests:
                        arguments = ["cargo", "test", "--manifest-path", str(configuration), "--locked", "--jobs", "2", "--lib"] + feature_arguments
                        if profile == "release":
                            arguments.append("--release")
                        arguments += [configuration_test, "--", "--exact", "--nocapture"]
                        if configuration_test.endswith("_actual_configuration_corpus"):
                            arguments.append("--ignored")
                        result = subprocess.run(arguments, env=environment, capture_output=True, text=True)
                        output = result.stdout + result.stderr
                        Path("target/package", "packaged-config-" + profile + "-" + configuration_test.rsplit("::", 1)[-1] + ".log").write_text(output)
                        result.check_returncode()
                        verify_query_execution(output, configuration_test)
                        assert hashlib.sha256(corpus.read_bytes()).hexdigest() == corpus_hash, "Config source corpus changed"
                        assert all(path.read_bytes() == payload for path, payload in candidate_custody.items()), "Config archive source custody changed"
            Path("target/package/packaged-config-corpus.json").write_text(json.dumps({"path": str(corpus), "sha256": corpus_hash}) + "\n")


if __name__ == "__main__":
    main()

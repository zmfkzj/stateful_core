from __future__ import annotations

import copy
import hashlib
import contextlib
import importlib.util
import io
import json
import sys
import subprocess
import shutil
import tarfile
import tempfile
import unittest
from unittest import mock

from threading import Event, Lock, Thread
from pathlib import Path
from types import ModuleType

SCRIPT_DIR = Path(__file__).resolve().parents[1]


def load_script(name: str) -> ModuleType:
    path = SCRIPT_DIR / name
    spec = importlib.util.spec_from_file_location(f"{path.stem}_test", path)
    assert spec and spec.loader
    module = importlib.util.module_from_spec(spec)
    sys.modules[spec.name] = module
    spec.loader.exec_module(module)
    return module


ROOT = Path(__file__).resolve().parents[4]
MANIFEST = ROOT / "datasets" / "statefulbench-realworld" / "manifest.json"
PINS = (
    ("requests", "f361ead047be5cb873174218582f7d8b9fcd9f49", "7f60df8524d7a042f604a4176cc64777f6543037ab96dc4adaaabff55ada28fd"),
    ("jsonschema", "97c044c48d6c6c08f88142ad27edc590f2a2cb07", "1d5bef7a24de2bec70a7840fef22cbaa5b169cf5159a0194635a7720eaa19a75"),
    ("pytest-asyncio", "66253978d8518925d3f5d2c12615fd7005b63080", "6715e3e9991cce7fb56ab50e19ceee46c5528ed4817ff9375eab8ab23612cf1d"),
    ("pytest-xdist", "f63b6a25b4eb932385c6ee4651eac5c08fbd3a20", "d035858bc41d5aa126e54a3edf8af4a4e871d0b7e5383211da767ab50ad6d511"),
    ("click", "b67832c2167e5b0ff6764a8c04a0a9087e697b5a", "bc2f89f9b4687d51ca6ff592f6de34a9f8f97c49b4637c84eabd6a8df16ed1d2"),
    ("django-storages", "ca89a94a7462a2423df460e7bfd5f847457042ca", "e0a0a36d3b1470776b6463e5dcd44c805fd31ccd3090110ea16936c176d90fab"),
    ("attrs", "45de9beb093d2142517ab7d1ebda6522e3d3c4ac", "b330a639611e08fcfd54baaf3780d364c5e9bec44ab33fda961f0fe6956daffd"),
    ("watchdog", "c9edf3296d9edb9afded6adfaf3987e87ca8f928", "a6e12fd17e2706161733cf111e7cd899a15b7e8a8f66540239c57e1a60d3d40d"),
    ("pendulum", "5ad098bc7b74d660679f0606673728042b9d4aca", "d49ad8f8c6f43a18c3744dec61730fb369cf91dc77bb1c23df3360ae76d11397"),
    ("authlib", "5cb26721a39f74a196304e90fa5ae8d31925fd4a", "c7e7818a31fd3ee7be4e370786974f92eb6c4f90f24efeeb6d3ea02fb0aca6e2"),
)


class ManifestTests(unittest.TestCase):
    def setUp(self) -> None:
        self.tempdir = tempfile.TemporaryDirectory()
        self.manifest_path = Path(self.tempdir.name) / "manifest.json"
        self.manifest_path.write_text(MANIFEST.read_text(encoding="utf-8"), encoding="utf-8")
        self.mod = load_script("statefulbench_realworld.py")

    def tearDown(self) -> None:
        self.tempdir.cleanup()

    def load_data(self) -> dict:
        return json.loads(self.manifest_path.read_text(encoding="utf-8"))

    def write_data(self, data: dict) -> None:
        self.manifest_path.write_text(json.dumps(data), encoding="utf-8")

    def test_load_manifest_accepts_ten_unique_pinned_repositories(self) -> None:
        manifest = self.mod.load_manifest(self.manifest_path)
        entries = self.mod.repo_entries(manifest)

        self.assertEqual(len(entries), 10)
        self.assertEqual(
            tuple((entry["key"], entry["commit"], entry["archive_sha256"]) for entry in entries),
            PINS,
        )
        for entry in entries:
            self.assertTrue(entry["requested_url"].startswith("https://"))
            self.assertTrue(entry["canonical_url"].startswith("https://"))
            self.assertTrue(entry["archive_url"].startswith("https://"))
            self.assertEqual(entry["python"], "3.14.6")
            self.assertTrue(entry["setup"])
            self.assertTrue(entry["suite"])
            self.assertTrue(all(isinstance(part, str) and part for part in entry["setup"]))
            self.assertTrue(all(isinstance(part, str) and part for part in entry["suite"]))
            self.assertEqual(entry["corpus"], f"repos/{entry['key']}.json")

    def test_manifest_loads_all_frozen_corpora(self) -> None:
        manifest = self.mod.load_manifest(MANIFEST)
        for repository in self.mod.repo_entries(manifest):
            with self.subTest(repository=repository["key"]):
                corpus = self.mod.load_corpus(MANIFEST.parent / repository["corpus"])
                self.assertTrue(self.mod._corpus_matches_repository(repository, corpus))


    def test_manifest_setup_declares_test_dependencies(self) -> None:
        sources = {
            "requests": ("--group", "test"),
            "jsonschema": ("--group", "test", "pytest"),
            "pytest-asyncio": (".[testing]", "pytest==8.4.2"),
            "pytest-xdist": (".[testing]", "pytest==8.4.2"),
            "click": ("--group", "tests"),
            "django-storages": ("pytest",),
            "attrs": ("--group", "tests"),
            "watchdog": ("-r", "requirements-tests.txt"),
            "pendulum": ("pytest", "pytest-benchmark"),
            "authlib": ("--group", "dev", "joserfc==1.6.1"),
        }
        entries = self.mod.repo_entries(self.mod.load_manifest(self.manifest_path))

        for entry in entries:
            with self.subTest(repository=entry["key"]):
                self.assertTrue(set(sources[entry["key"]]).issubset(entry["setup"]))

    def test_manifest_declares_source_proven_environments_and_safe_suites(self) -> None:
        entries = {
            entry["key"]: entry
            for entry in self.mod.repo_entries(self.mod.load_manifest(self.manifest_path))
        }

        self.assertEqual(
            entries["django-storages"]["environment"],
            {
                "DJANGO_SETTINGS_MODULE": "tests.settings",
                "AWS_CONFIG_FILE": "tests/no_such_file.conf",
            },
        )
        self.assertEqual(
            entries["attrs"]["environment"],
            {"SETUPTOOLS_SCM_PRETEND_VERSION": "26.1.1.dev24"},
        )
        self.assertEqual(
            entries["watchdog"]["suite"],
            [
                "python",
                "-m",
                "pytest",
                "-q",
                "-p",
                "no:cov",
                "-o",
                "addopts=--showlocals -vvv",
                "--ignore=tests/test_emitter.py",
                "--ignore=tests/test_fsevents.py",
                "--deselect=tests/test_0_watchmedo.py::test_tricks_from_file",
            ],
        )
        self.assertEqual(
            entries["pytest-asyncio"]["suite"],
            [
                "python",
                "-m",
                "pytest",
                "-q",
                "--deselect=tests/test_set_event_loop.py::test_asyncio_run_after_async_fixture_does_not_leak_loop",
            ],
        )
        exclusions = {
            "requests": {
                "--deselect=tests/test_requests.py::TestRequests::test_empty_stream_with_auth_does_not_set_content_length_header": (
                    "Pinned Requests task contract restores chunked/no-Content-Length "
                    "handling for authenticated empty seekable streams; upstream test "
                    "asserts the superseded behavior."
                ),
                "--deselect=tests/test_requests.py::TestRequests::test_invalid_ssl_certificate_files": (
                    "Pinned Requests task contract requires structured FileNotFoundError "
                    "for all missing certificate forms; upstream test asserts legacy "
                    "IOError text."
                ),
                "--deselect=tests/test_requests.py::TestRequests::test_cookie_quote_wrapped": (
                    "Pinned Requests task contract preserves byte-for-byte escaped cookie "
                    "quotes; upstream test asserts the superseded unwrapped value."
                ),
            },
            "jsonschema": {
                "--deselect=jsonschema/tests/test_validators.py::TestValidationErrorDetails::test_anyOf": (
                    "Pinned jsonschema task contract includes the parent branch index in "
                    "child relative_schema_path; upstream test asserts the superseded "
                    "root-relative path."
                ),
                "--deselect=jsonschema/tests/test_validators.py::TestValidationErrorDetails::test_type": (
                    "Pinned jsonschema task contract includes the parent branch index in "
                    "child relative_schema_path; upstream test asserts the superseded "
                    "root-relative path."
                ),
                "--deselect=jsonschema/tests/test_validators.py::TestValidationErrorDetails::test_ref_sibling": (
                    "Pinned jsonschema task contract retains referenced-child "
                    "relative_schema_path; upstream test asserts the superseded "
                    "root/reference-site path."
                ),
            },
            "pytest-asyncio": {
                "--deselect=tests/test_set_event_loop.py::test_asyncio_run_after_async_fixture_does_not_leak_loop": (
                    "Pinned CPython 3.14.6 with every supported pytest version reproduces "
                    "pytest-asyncio's duplicate-plugin warning, promoted to error by this "
                    "upstream test."
                ),
            },
            "watchdog": {
                "--ignore=tests/test_emitter.py": (
                    "Pinned CPython 3.14.6 on the macOS sandbox crashes Watchdog's "
                    "FSEvents-backed emitter tests."
                ),
                "--ignore=tests/test_fsevents.py": (
                    "Pinned CPython 3.14.6 on the macOS sandbox cannot run native "
                    "FSEvents stream tests without a crash."
                ),
                "--deselect=tests/test_0_watchmedo.py::test_tricks_from_file": (
                    "Pinned CPython 3.14.6 on the macOS sandbox crashes "
                    "test_tricks_from_file."
                ),
            },
        }
        for key, expected in exclusions.items():
            with self.subTest(repository=key):
                self.assertEqual(entries[key]["metadata"]["exclusions"], expected)
                self.assertTrue(set(expected).issubset(entries[key]["suite"]))
        self.assertNotIn("-k", entries["watchdog"]["suite"])
        self.assertIn("addopts=--showlocals -vvv", entries["watchdog"]["suite"])

    def test_django_storages_exclusions_bind_exact_nodes_to_suite(self) -> None:
        entry = next(
            entry
            for entry in self.mod.repo_entries(self.mod.load_manifest(self.manifest_path))
            if entry["key"] == "django-storages"
        )
        expected = {
            "--deselect=tests/test_s3.py::S3StorageTests::test_auth_config": (
                "Pinned django-storages task contract removes legacy credential aliases; "
                "upstream test asserts the superseded behavior."
            ),
            "--deselect=tests/test_s3.py::S3StorageTests::test_pickle_with_bucket": (
                "Pinned django-storages task contract defines the cache/pickle model; "
                "upstream test asserts the superseded behavior."
            ),
            "--deselect=tests/test_s3.py::S3StorageTests::test_security_token": (
                "Pinned django-storages task contract removes the token alias; upstream "
                "test asserts the superseded behavior."
            ),
            "--deselect=tests/test_s3.py::S3StorageTests::test_url_unsigned": (
                "Pinned django-storages task contract defines unsigned URL endpoint "
                "behavior; upstream test asserts the superseded behavior."
            ),
        }

        self.assertIn("metadata", entry)
        self.assertEqual(entry["metadata"]["exclusions"], expected)
        self.assertEqual(
            entry["suite"],
            ["python", "-m", "pytest", "-q", *expected],
        )

    def test_load_manifest_rejects_exclusion_metadata_that_does_not_match_the_suite(self) -> None:
        data = self.load_data()
        entry = next(repo for repo in data["repositories"] if repo["key"] == "watchdog")
        entry["metadata"]["exclusions"]["--deselect=tests/not-a-real-test.py::test_case"] = (
            "Pinned runtime reason."
        )
        self.write_data(data)

        with self.assertRaisesRegex(ValueError, "metadata exclusion"):
            self.mod.load_manifest(self.manifest_path)

    def test_load_manifest_rejects_undocumented_suite_exclusion(self) -> None:
        data = self.load_data()
        entry = next(repo for repo in data["repositories"] if repo["key"] == "watchdog")
        entry["suite"].append("--ignore=tests")
        self.write_data(data)

        with self.assertRaisesRegex(ValueError, "suite exclusions"):
            self.mod.load_manifest(self.manifest_path)

    def test_load_manifest_rejects_suite_exclusions_without_metadata(self) -> None:
        data = self.load_data()
        entry = next(repo for repo in data["repositories"] if repo["key"] == "watchdog")
        del entry["metadata"]
        self.write_data(data)

        with self.assertRaisesRegex(ValueError, "suite exclusions"):
            self.mod.load_manifest(self.manifest_path)

    def test_load_manifest_accepts_two_argument_suite_exclusions(self) -> None:
        data = self.load_data()
        entry = next(repo for repo in data["repositories"] if repo["key"] == "watchdog")
        index = entry["suite"].index("--ignore=tests/test_emitter.py")
        entry["suite"][index : index + 1] = ["--ignore", "tests/test_emitter.py"]
        self.write_data(data)

        self.mod.load_manifest(self.manifest_path)

    def test_load_manifest_rejects_duplicate_suite_exclusions(self) -> None:
        data = self.load_data()
        entry = next(repo for repo in data["repositories"] if repo["key"] == "watchdog")
        entry["suite"].append("--ignore=tests/test_emitter.py")
        self.write_data(data)

        with self.assertRaisesRegex(ValueError, "suite exclusions"):
            self.mod.load_manifest(self.manifest_path)

    def test_load_manifest_rejects_option_as_split_exclusion_value(self) -> None:
        for option, value in (
            ("--ignore", "--deselect=tests/test_emitter.py"),
            ("--deselect", "--ignore=tests/test_emitter.py"),
        ):
            with self.subTest(option=option):
                data = self.load_data()
                entry = next(repo for repo in data["repositories"] if repo["key"] == "pendulum")
                entry["suite"] = ["python", "-m", "pytest", "-q", option, value]
                entry["metadata"] = {
                    "exclusions": {
                        f"{option}={value}": "Pinned runtime reason.",
                        value: "Pinned runtime reason.",
                    }
                }
                self.write_data(data)

                with self.assertRaisesRegex(ValueError, "suite exclusions must have a value"):
                    self.mod.load_manifest(self.manifest_path)

    def test_load_manifest_rejects_unapproved_pytest_selection(self) -> None:
        for addition in (
            ["-k", "not slow"],
            ["--ignore-glob=tests/test_*.py"],
            ["tests/test_requests.py"],
            ["-o", "addopts=-k not_slow"],
        ):
            with self.subTest(addition=addition):
                data = self.load_data()
                entry = next(repo for repo in data["repositories"] if repo["key"] == "pendulum")
                entry["suite"] = ["python", "-m", "pytest", "-q", *addition]
                self.write_data(data)

                with self.assertRaisesRegex(ValueError, "suite must be an audited pytest argv"):
                    self.mod.load_manifest(self.manifest_path)


    def test_manifest_repository_matches_full_corpus_identity(self) -> None:
        repo = self.mod.repo_entries(self.mod.load_manifest(self.manifest_path))[0]
        corpus = self.mod.load_corpus(MANIFEST.parent / repo["corpus"])

        self.assertTrue(self.mod._corpus_matches_repository(repo, corpus))
    def test_load_manifest_rejects_duplicate_keys(self) -> None:
        data = self.load_data()
        data["repositories"][1]["key"] = data["repositories"][0]["key"]
        self.write_data(data)

        with self.assertRaisesRegex(ValueError, "duplicate repository key"):
            self.mod.load_manifest(self.manifest_path)

    def test_load_manifest_rejects_malformed_hash_url_and_argv(self) -> None:
        cases = (
            ("commit", "not-a-sha", "commit"),
            ("archive_sha256", "f" * 63, "archive_sha256"),
            ("canonical_url", "http://github.com/psf/requests", "canonical_url"),
            ("setup", [], "setup"),
            ("suite", [""], "suite"),
        )
        for field, value, message in cases:
            with self.subTest(field=field):
                data = json.loads(MANIFEST.read_text(encoding="utf-8"))
                data["repositories"][0][field] = value
                self.write_data(data)
                with self.assertRaisesRegex(ValueError, message):
                    self.mod.load_manifest(self.manifest_path)

    def test_load_manifest_binds_github_archive_to_canonical_repository_and_commit(self) -> None:
        commit = self.load_data()["repositories"][0]["commit"]
        cases = (
            ("canonical_url", "https://github.com/psf/requests/", "canonical_url"),
            ("canonical_url", "https://github.com/psf/\nrequests", "canonical_url"),
            ("canonical_url", "https://github.com:443/psf/requests", "canonical_url"),
            ("canonical_url", "https://github.com/psf/requests?source=manifest", "canonical_url"),
            ("archive_url", f"https://github.com/psf/other/archive/{commit}.tar.gz", "archive_url"),
            ("archive_url", f"https://github.com/psf/requests/archive/{'0' * 40}.tar.gz", "archive_url"),
            ("archive_url", f"https://github.com:443/psf/requests/archive/{commit}.tar.gz", "archive_url"),
            ("archive_url", f"https://github.com/psf/requests/archive/{commit}.tar.gz#fragment", "archive_url"),
        )
        for field, value, message in cases:
            with self.subTest(field=field, value=value):
                data = json.loads(MANIFEST.read_text(encoding="utf-8"))
                data["repositories"][0][field] = value
                self.write_data(data)

                with self.assertRaisesRegex(ValueError, message):
                    self.mod.load_manifest(self.manifest_path)


    def test_load_manifest_rejects_corpus_escape(self) -> None:
        data = self.load_data()
        data["repositories"][0]["corpus"] = "../escape.json"
        self.write_data(data)

        with self.assertRaisesRegex(ValueError, "corpus path"):
            self.mod.load_manifest(self.manifest_path)

    def test_load_manifest_rejects_unsafe_repository_keys(self) -> None:
        for key in (".", "..", "/absolute", "nested/key"):
            with self.subTest(key=key):
                data = self.load_data()
                data["repositories"][0]["key"] = key
                self.write_data(data)

                with self.assertRaisesRegex(ValueError, "key"):
                    self.mod.load_manifest(self.manifest_path)

    def test_load_manifest_validates_optional_environment_map(self) -> None:
        data = self.load_data()
        data["repositories"][0]["environment"] = {"PROJECT_SETTING": "enabled"}
        self.write_data(data)

        self.assertEqual(
            self.mod.load_manifest(self.manifest_path)["repositories"][0]["environment"],
            {"PROJECT_SETTING": "enabled"},
        )

        for value in (["not", "a", "map"], {"bad-name": "value"}, {"PYTHONPATH": "unsafe"}):
            with self.subTest(value=value):
                data = self.load_data()
                data["repositories"][0]["environment"] = value
                self.write_data(data)
                with self.assertRaisesRegex(ValueError, "environment"):
                    self.mod.load_manifest(self.manifest_path)
    def test_verified_python_rejects_manifest_version_mismatch(self) -> None:
        with mock.patch.object(
            self.mod.sys, "version_info", (3, 14, 5, "final", 0)
        ), self.assertRaisesRegex(ValueError, "python"):
            self.mod.verified_python("3.14.6")

    def test_workspace_environment_isolated_without_rustup_and_has_resolved_rust_tools(self) -> None:
        workspace = Path(self.tempdir.name) / "workspace"
        venv = workspace / ".statefulbench-venv"
        rust_bin = Path(self.tempdir.name) / "pendulum-toolchain" / "bin"
        rust_bin.mkdir(parents=True)
        rust_tools = {name: rust_bin / name for name in ("rustc", "cargo")}
        for executable in rust_tools.values():
            executable.touch(mode=0o755)
        with mock.patch.dict(
            self.mod.os.environ,
            {
                "PYTHONPATH": "/host/python",
                "UNRELATED_HOST_SETTING": "discard",
                "RUSTUP_HOME": "/host/rustup",
            },
            clear=True,
        ), mock.patch.object(
            self.mod.shutil, "which", return_value="/host/bin/rustup"
        ), mock.patch.object(
            self.mod.subprocess,
            "run",
            side_effect=[
                subprocess.CompletedProcess(
                    ["/host/bin/rustup", "which", "rustc"],
                    0,
                    f"{rust_tools['rustc']}\n",
                    "",
                ),
                subprocess.CompletedProcess(
                    ["/host/bin/rustup", "which", "cargo"],
                    0,
                    f"{rust_tools['cargo']}\n",
                    "",
                ),
            ],
        ) as rustup_which:
            environment = self.mod._sanitized_environment(venv, workspace)

        runtime_root = workspace.parent / ".statefulbench-runtime"
        self.assertEqual(environment["VIRTUAL_ENV"], str(venv))
        self.assertEqual(environment["HOME"], str(runtime_root / "home"))
        self.assertEqual(environment["PIP_CACHE_DIR"], str(runtime_root / "pip-cache"))
        self.assertEqual(environment["TMPDIR"], str(runtime_root / "tmp"))
        self.assertEqual(environment["CARGO_HOME"], str(runtime_root / "cargo-home"))
        self.assertNotIn("RUSTUP_HOME", environment)
        for name in ("HOME", "PIP_CACHE_DIR", "TMPDIR", "CARGO_HOME"):
            location = Path(environment[name])
            self.assertTrue(location.is_dir())
            self.assertTrue(location.is_relative_to(runtime_root))
            self.assertFalse(location.is_relative_to(workspace))
        self.assertEqual(environment["PATH"].split(":")[:2], [str(venv / "bin"), str(rust_bin.resolve())])
        rustup_which.assert_has_calls(
            [
                mock.call(
                    ["/host/bin/rustup", "which", "rustc"],
                    capture_output=True,
                    check=False,
                    encoding="utf-8",
                    errors="replace",
                    cwd=workspace,
                ),
                mock.call(
                    ["/host/bin/rustup", "which", "cargo"],
                    capture_output=True,
                    check=False,
                    encoding="utf-8",
                    errors="replace",
                    cwd=workspace,
                ),
            ]
        )
        self.assertNotIn("PYTHONPATH", environment)
        self.assertNotIn("UNRELATED_HOST_SETTING", environment)


    def test_explicit_pip_cache_is_shared_while_workspace_runtime_isolated(self) -> None:
        pip_cache = Path(self.tempdir.name) / "benchmark-cache" / "pip-cache"
        first_workspace = Path(self.tempdir.name) / "first" / "workspace"
        second_workspace = Path(self.tempdir.name) / "second" / "workspace"
        with mock.patch.object(self.mod, "_rust_tool_directories", return_value=()):
            first = self.mod._sanitized_environment(
                workspace=first_workspace, pip_cache_dir=pip_cache
            )
            second = self.mod._sanitized_environment(
                workspace=second_workspace, pip_cache_dir=pip_cache
            )

        self.assertEqual(first["PIP_CACHE_DIR"], str(pip_cache))
        self.assertEqual(second["PIP_CACHE_DIR"], str(pip_cache))
        self.assertTrue(pip_cache.is_dir())
        for name in ("HOME", "TMPDIR", "CARGO_HOME"):
            self.assertNotEqual(first[name], second[name])
            self.assertFalse(Path(first[name]).is_relative_to(first_workspace))
            self.assertFalse(Path(second[name]).is_relative_to(second_workspace))

    def test_repository_environment_cannot_override_protected_runtime_paths(self) -> None:
        protected = {"HOME": "/runtime/home", "PIP_CACHE_DIR": "/cache/pip-cache"}

        environment = self.mod._repository_environment(
            {
                "environment": {
                    "HOME": "/repository/home",
                    "PIP_CACHE_DIR": "/repository/pip-cache",
                    "PROJECT_SETTING": "enabled",
                }
            },
            protected,
        )

        self.assertEqual(environment["HOME"], protected["HOME"])
        self.assertEqual(environment["PIP_CACHE_DIR"], protected["PIP_CACHE_DIR"])
        self.assertEqual(environment["PROJECT_SETTING"], "enabled")
    def test_workspace_environment_falls_back_to_direct_resolved_rust_tools(self) -> None:
        workspace = Path(self.tempdir.name) / "workspace"
        rust_bin = Path(self.tempdir.name) / "direct-toolchain" / "bin"
        rust_bin.mkdir(parents=True)
        tools = {name: rust_bin / name for name in ("rustc", "cargo")}
        for executable in tools.values():
            executable.touch(mode=0o755)
        with mock.patch.object(
            self.mod.shutil,
            "which",
            side_effect=lambda name: {
                "rustup": "/host/bin/rustup",
                "rustc": str(tools["rustc"]),
                "cargo": str(tools["cargo"]),
            }.get(name),
        ), mock.patch.object(
            self.mod.subprocess,
            "run",
            return_value=subprocess.CompletedProcess([], 1, "", "unavailable"),
        ):
            environment = self.mod._sanitized_environment(workspace=workspace)

        self.assertEqual(environment["PATH"].split(":")[0], str(rust_bin.resolve()))



class CorpusTests(unittest.TestCase):
    def setUp(self) -> None:
        self.tempdir = tempfile.TemporaryDirectory()
        self.dataset_root = Path(self.tempdir.name) / "statefulbench-realworld"
        self.corpus_path = self.dataset_root / "repos" / "fixture.json"
        self.issue_snapshot_path = self.dataset_root / "issues" / "fixture.json"
        self.corpus_path.parent.mkdir(parents=True)
        self.mod = load_script("statefulbench_realworld.py")
        self.write_data(self.corpus_data())

    def tearDown(self) -> None:
        self.tempdir.cleanup()

    def corpus_data(self) -> dict:
        tasks = []
        for index in range(10):
            group = index // 2
            tasks.append(
                {
                    "key": f"task-{index}",
                    "kind": "bug" if index < 5 else "feature",
                    "sources": [f"https://github.com/example/project/issues/{index}"],
                    "source_hash": hashlib.sha256(f"body {index}".encode("utf-8")).hexdigest(),
                    "prompt": f"Implement task {index}.",
                    "acceptance": ["normal behavior", "boundary behavior", "error behavior"],
                    "overlap_anchors": [
                        {
                            "path": f"package/module_{group}.py",
                            "symbol": f"package.module_{group}.target",
                        }
                    ],
                    "evaluator": f"evaluators/task-{index}.py",
                    "reference_patch": f"references/task-{index}.patch",
                }
            )
        return {
            "repository": "fixture",
            "issue_snapshot": "issues/fixture.json",
            "tasks": tasks,
            "final_prompt": "Repair all task implementations.",
            "evaluators": [f"evaluators/task-{index}.py" for index in range(10)],
            "integrated_reference_patch": "references/integrated.patch",
        }

    def load_data(self) -> dict:
        return json.loads(self.corpus_path.read_text(encoding="utf-8"))

    def write_data(self, data: dict) -> None:
        self.corpus_path.write_text(json.dumps(data), encoding="utf-8")
        self.issue_snapshot_path.parent.mkdir(parents=True, exist_ok=True)
        self.issue_snapshot_path.write_text(
            json.dumps(
                {
                    "issues": [
                        {
                            "html_url": f"https://github.com/example/project/issues/{index}",
                            "body": f"body {index}",
                        }
                        for index in range(10)
                    ]
                }
            ),
            encoding="utf-8",
        )

    def test_load_corpus_accepts_balanced_connected_tasks(self) -> None:
        corpus = self.mod.load_corpus(self.corpus_path)

        self.assertEqual(corpus["repository"], "fixture")
        self.assertEqual(len(corpus["tasks"]), 10)

    def test_load_corpus_accepts_list_issue_snapshot(self) -> None:
        self.write_data(self.corpus_data())
        snapshot = json.loads(self.issue_snapshot_path.read_text(encoding="utf-8"))
        self.issue_snapshot_path.write_text(
            json.dumps(snapshot["issues"]), encoding="utf-8"
        )

        self.mod.load_corpus(self.corpus_path)

    def test_load_corpus_accepts_canonical_github_source_ports(self) -> None:
        for source in (
            "https://github.com/example/project/issues/0",
            "https://github.com:443/example/project/issues/0",
        ):
            with self.subTest(source=source):
                data = self.corpus_data()
                data["tasks"][0]["sources"] = [source]
                self.write_data(data)

                self.mod.load_corpus(self.corpus_path)

    def test_load_corpus_rejects_source_hash_mismatch(self) -> None:
        data = self.corpus_data()
        data["tasks"][0]["source_hash"] = "0" * 64
        self.write_data(data)

        with self.assertRaisesRegex(ValueError, "source_hash"):
            self.mod.load_corpus(self.corpus_path)

    def test_load_corpus_rejects_missing_issue_snapshot_source(self) -> None:
        self.write_data(self.corpus_data())
        snapshot = json.loads(self.issue_snapshot_path.read_text(encoding="utf-8"))
        snapshot["issues"].pop(0)
        self.issue_snapshot_path.write_text(json.dumps(snapshot), encoding="utf-8")

        with self.assertRaisesRegex(ValueError, "source"):
            self.mod.load_corpus(self.corpus_path)

    def test_load_corpus_rejects_duplicate_issue_snapshot_source(self) -> None:
        self.write_data(self.corpus_data())
        snapshot = json.loads(self.issue_snapshot_path.read_text(encoding="utf-8"))
        duplicate = snapshot["issues"][0].copy()
        duplicate["html_url"] = "https://github.com/EXAMPLE/PROJECT/issues/0"
        snapshot["issues"].append(duplicate)
        self.issue_snapshot_path.write_text(json.dumps(snapshot), encoding="utf-8")

        with self.assertRaisesRegex(ValueError, "duplicate"):
            self.mod.load_corpus(self.corpus_path)

    def test_load_corpus_rejects_non_string_issue_snapshot_body(self) -> None:
        self.write_data(self.corpus_data())
        snapshot = json.loads(self.issue_snapshot_path.read_text(encoding="utf-8"))
        snapshot["issues"][0]["body"] = None
        self.issue_snapshot_path.write_text(json.dumps(snapshot), encoding="utf-8")

        with self.assertRaisesRegex(ValueError, "body"):
            self.mod.load_corpus(self.corpus_path)


    def test_load_corpus_rejects_duplicate_task_keys(self) -> None:
        data = self.load_data()
        data["tasks"][1]["key"] = data["tasks"][0]["key"]
        self.write_data(data)

        with self.assertRaisesRegex(ValueError, "duplicate task key"):
            self.mod.load_corpus(self.corpus_path)

    def test_load_corpus_requires_five_bug_and_five_feature_tasks(self) -> None:
        data = self.load_data()
        data["tasks"][5]["kind"] = "bug"
        self.write_data(data)

        with self.assertRaisesRegex(ValueError, "five bug"):
            self.mod.load_corpus(self.corpus_path)

    def test_load_corpus_requires_three_acceptance_criteria(self) -> None:
        data = self.load_data()
        data["tasks"][0]["acceptance"] = ["normal", "boundary"]
        self.write_data(data)

        with self.assertRaisesRegex(ValueError, "acceptance"):
            self.mod.load_corpus(self.corpus_path)

    def test_load_corpus_rejects_malformed_sources_and_hashes(self) -> None:
        cases = (
            ("sources", ["https://example.com/not-github"], "sources"),
            (
                "sources",
                ["https://github.com:not-a-port/example/project/issues/0"],
                "sources",
            ),
            ("sources", ["https://github.com:444/example/project/issues/0"], "sources"),
            ("source_hash", "f" * 63, "source_hash"),
        )
        for field, value, message in cases:
            with self.subTest(field=field):
                data = self.corpus_data()
                data["tasks"][0][field] = value
                self.write_data(data)

                with self.assertRaisesRegex(ValueError, message):
                    self.mod.load_corpus(self.corpus_path)

    def test_load_corpus_rejects_paths_outside_dataset_root(self) -> None:
        cases = (
            ("evaluator", "../escape.py"),
            ("reference_patch", "../escape.patch"),
            ("integrated_reference_patch", "../integrated.patch"),
        )
        for field, value in cases:
            with self.subTest(field=field):
                data = self.corpus_data()
                target = data if field == "integrated_reference_patch" else data["tasks"][0]
                target[field] = value
                self.write_data(data)

                with self.assertRaisesRegex(ValueError, "path"):
                    self.mod.load_corpus(self.corpus_path)

    def test_load_corpus_rejects_evaluator_traversal_outside_evaluators(self) -> None:
        data = self.load_data()
        data["tasks"][0]["evaluator"] = "evaluators/../pyproject.toml"
        data["evaluators"][0] = "evaluators/../pyproject.toml"
        self.write_data(data)

        with self.assertRaisesRegex(ValueError, "evaluator"):
            self.mod.load_corpus(self.corpus_path)

        self.assertFalse((self.dataset_root / "pyproject.toml").exists())

    def test_load_corpus_requires_nonempty_path_and_symbol_anchors(self) -> None:
        cases = (
            [],
            [{"path": "", "symbol": "package.module.target"}],
            [{"path": "package/module.py", "symbol": ""}],
        )
        for anchors in cases:
            with self.subTest(anchors=anchors):
                data = self.corpus_data()
                data["tasks"][0]["overlap_anchors"] = anchors
                self.write_data(data)

                with self.assertRaises(ValueError):
                    self.mod.load_corpus(self.corpus_path)

    def test_load_corpus_rejects_isolated_task(self) -> None:
        data = self.load_data()
        data["tasks"][0]["overlap_anchors"] = [
            {"path": "package/isolated.py", "symbol": "package.isolated.target"}
        ]
        self.write_data(data)

        with self.assertRaisesRegex(ValueError, "isolated"):
            self.mod.load_corpus(self.corpus_path)

    def test_load_corpus_requires_exact_task_evaluators(self) -> None:
        data = self.load_data()
        data["evaluators"] = data["evaluators"][1:]
        self.write_data(data)

        with self.assertRaisesRegex(ValueError, "evaluators"):
            self.mod.load_corpus(self.corpus_path)

    def test_load_corpus_rejects_unsafe_task_keys(self) -> None:
        for key in (".", "..", "/absolute", "nested/key"):
            with self.subTest(key=key):
                data = self.corpus_data()
                data["tasks"][0]["key"] = key
                self.write_data(data)

                with self.assertRaisesRegex(ValueError, "key"):
                    self.mod.load_corpus(self.corpus_path)



    def test_load_corpus_rejects_test_only_anchor_paths(self) -> None:
        data = self.corpus_data()
        data["tasks"][0]["overlap_anchors"] = [
            {"path": "tests/test_module.py", "symbol": "tests.test_module.target"}
        ]
        self.write_data(data)

        with self.assertRaisesRegex(ValueError, "production"):
            self.mod.load_corpus(self.corpus_path)

class ArchiveTests(unittest.TestCase):
    def setUp(self) -> None:
        self.tempdir = tempfile.TemporaryDirectory()
        self.root = Path(self.tempdir.name)
        self.mod = load_script("statefulbench_realworld.py")

    def tearDown(self) -> None:
        self.tempdir.cleanup()

    def archive_bytes(
        self,
        files: dict[str, bytes],
        *,
        symlink: str | None = None,
        symlink_path: str = "link",
        hardlink: str | None = None,
        extra_root: bool = False,
    ) -> bytes:
        output = io.BytesIO()
        with tarfile.open(fileobj=output, mode="w:gz") as archive:
            root = tarfile.TarInfo("source")
            root.type = tarfile.DIRTYPE
            archive.addfile(root)
            for name, contents in files.items():
                member = tarfile.TarInfo(f"source/{name}")
                member.size = len(contents)
                archive.addfile(member, io.BytesIO(contents))
            if symlink is not None:
                member = tarfile.TarInfo(f"source/{symlink_path}")
                member.type = tarfile.SYMTYPE
                member.linkname = symlink
                archive.addfile(member)
            if hardlink is not None:
                member = tarfile.TarInfo("source/hardlink")
                member.type = tarfile.LNKTYPE
                member.linkname = hardlink
                archive.addfile(member)
            if extra_root:
                root = tarfile.TarInfo("other")
                root.type = tarfile.DIRTYPE
                archive.addfile(root)
        return output.getvalue()

    def write_archive(self, contents: bytes) -> tuple[Path, str]:
        archive = self.root / "source.tar.gz"
        archive.write_bytes(contents)
        return archive, hashlib.sha256(contents).hexdigest()

    def test_ensure_archive_downloads_once_then_uses_verified_cache(self) -> None:
        contents = self.archive_bytes({"pyproject.toml": b"[project]\n"})
        expected_sha256 = hashlib.sha256(contents).hexdigest()
        repo = {
            "archive_url": "https://example.invalid/source.tar.gz",
            "archive_sha256": expected_sha256,
        }
        calls: list[str] = []

        def opener(url: str) -> io.BytesIO:
            calls.append(url)
            return io.BytesIO(contents)

        cache_dir = self.root / "cache"
        archive = self.mod.ensure_archive(repo, cache_dir, opener)

        self.assertEqual(archive, cache_dir / f"{expected_sha256}.tar.gz")
        self.assertEqual(archive.read_bytes(), contents)
        self.assertEqual(calls, [repo["archive_url"]])
        self.assertEqual(
            self.mod.ensure_archive(repo, cache_dir, lambda _: self.fail("network called")),
            archive,
        )

    def test_ensure_archive_rejects_mismatch_and_removes_temporary_download(self) -> None:
        contents = self.archive_bytes({"pyproject.toml": b"[project]\n"})
        repo = {
            "archive_url": "https://example.invalid/source.tar.gz",
            "archive_sha256": "0" * 64,
        }
        cache_dir = self.root / "cache"

        with self.assertRaisesRegex(ValueError, "checksum"):
            self.mod.ensure_archive(repo, cache_dir, lambda _: io.BytesIO(contents))

        self.assertFalse(any(cache_dir.glob("*.tmp")))
        self.assertFalse((cache_dir / f"{repo['archive_sha256']}.tar.gz").exists())

    def test_ensure_archive_concurrent_downloads_do_not_promote_unverified_bytes(self) -> None:
        contents = self.archive_bytes({"pyproject.toml": b"[project]\n"})
        unverified = b"unverified download"
        expected_sha256 = hashlib.sha256(contents).hexdigest()
        repo = {
            "archive_url": "https://example.invalid/source.tar.gz",
            "archive_sha256": expected_sha256,
        }
        first_write = Event()
        second_write = Event()
        first_finished = Event()
        opener_lock = Lock()
        opener_count = 0
        results: list[Path] = []
        errors: list[BaseException] = []

        class Response:
            def __init__(self, data: bytes, before_eof: Event | None = None) -> None:
                self.data = data
                self.before_eof = before_eof
                self.reads = 0

            def __enter__(self) -> Response:
                return self

            def __exit__(self, *_: object) -> None:
                return None

            def read(self, _: int) -> bytes:
                if self.reads == 0:
                    self.reads += 1
                    if self.data == contents:
                        first_write.set()
                    else:
                        second_write.set()
                    return self.data
                if self.before_eof is not None:
                    self.before_eof.wait(1)
                return b""

        def opener(_: str) -> Response:
            nonlocal opener_count
            with opener_lock:
                opener_count += 1
                return Response(contents) if opener_count == 1 else Response(unverified, first_finished)

        def download() -> None:
            try:
                results.append(self.mod.ensure_archive(repo, self.root / "cache", opener))
            except BaseException as error:
                errors.append(error)
            finally:
                first_finished.set()

        first = Thread(target=download)
        second = Thread(target=download)
        first.start()
        self.assertTrue(first_write.wait(1))
        second.start()
        self.assertTrue(second_write.wait(1))
        first.join(1)
        second.join(1)

        self.assertFalse(first.is_alive())
        self.assertFalse(second.is_alive())
        self.assertEqual(results, [self.root / "cache" / f"{expected_sha256}.tar.gz"])
        self.assertEqual(len(errors), 1)
        self.assertIsInstance(errors[0], ValueError)
        self.assertEqual(results[0].read_bytes(), contents)
        self.assertFalse(any((self.root / "cache").glob("*.tmp")))

    def test_extract_workspace_allows_safe_internal_links(self) -> None:
        contents = self.archive_bytes(
            {"target.txt": b"linked content\n"},
            symlink="target.txt",
            hardlink="source/target.txt",
        )
        archive, expected_sha256 = self.write_archive(contents)
        destination = self.root / "links"

        self.mod.extract_workspace(archive, expected_sha256, destination)

        self.assertTrue((destination / "link").is_symlink())
        self.assertEqual((destination / "link").read_bytes(), b"linked content\n")
        self.assertEqual((destination / "hardlink").read_bytes(), b"linked content\n")
        self.assertEqual(
            (destination / "hardlink").stat().st_ino,
            (destination / "target.txt").stat().st_ino,
        )

    def test_extract_workspace_resolves_relative_symlinks_from_member_parent(self) -> None:
        contents = self.archive_bytes(
            {"target": b"linked content\n"},
            symlink="../target",
            symlink_path="sub/link",
        )
        archive, expected_sha256 = self.write_archive(contents)
        destination = self.root / "relative-symlink"

        self.mod.extract_workspace(archive, expected_sha256, destination)

        self.assertEqual((destination / "sub" / "link").read_bytes(), b"linked content\n")

    def test_extract_workspace_allows_normalized_hardlink_targets(self) -> None:
        contents = self.archive_bytes(
            {"target.txt": b"linked content\n"},
            hardlink="source/dir/../target.txt",
        )
        archive, expected_sha256 = self.write_archive(contents)
        destination = self.root / "normalized-hardlink"

        self.mod.extract_workspace(archive, expected_sha256, destination)

        self.assertEqual((destination / "hardlink").read_bytes(), b"linked content\n")
        self.assertEqual(
            (destination / "hardlink").stat().st_ino,
            (destination / "target.txt").stat().st_ino,
        )

    def test_extract_workspace_rejects_symlink_chain_escaping_root(self) -> None:
        contents = io.BytesIO()
        with tarfile.open(fileobj=contents, mode="w:gz") as source:
            root = tarfile.TarInfo("source")
            root.type = tarfile.DIRTYPE
            source.addfile(root)
            victim = tarfile.TarInfo("source/victim")
            victim.size = len(b"victim")
            source.addfile(victim, io.BytesIO(b"victim"))
            directory = tarfile.TarInfo("source/dir")
            directory.type = tarfile.SYMTYPE
            directory.linkname = "."
            source.addfile(directory)
            link = tarfile.TarInfo("source/dir/link")
            link.type = tarfile.SYMTYPE
            link.linkname = "../victim"
            source.addfile(link)
        archive, expected_sha256 = self.write_archive(contents.getvalue())
        destination = self.root / "symlink-chain"

        with self.assertRaisesRegex(ValueError, "archive"):
            self.mod.extract_workspace(archive, expected_sha256, destination)

        self.assertFalse(destination.exists())

    def test_extract_workspace_rejects_unsafe_members_and_multiple_roots(self) -> None:
        cases = (
            ("traversal", self.archive_bytes({"../escape": b"no"})),
            (
                "relative symlink",
                self.archive_bytes({"pyproject.toml": b"[project]\n"}, symlink="../escape"),
            ),
            (
                "absolute symlink",
                self.archive_bytes({"pyproject.toml": b"[project]\n"}, symlink="/escape"),
            ),
            ("multiple roots", self.archive_bytes({"pyproject.toml": b"[project]\n"}, extra_root=True)),
        )
        for name, contents in cases:
            with self.subTest(name=name):
                archive, expected_sha256 = self.write_archive(contents)
                destination = self.root / name

                with self.assertRaisesRegex(ValueError, "archive"):
                    self.mod.extract_workspace(archive, expected_sha256, destination)

                self.assertFalse(destination.exists())

    def test_extract_workspace_rejects_dot_root_members(self) -> None:
        contents = io.BytesIO()
        with tarfile.open(fileobj=contents, mode="w:gz") as source:
            root = tarfile.TarInfo(".")
            root.type = tarfile.DIRTYPE
            source.addfile(root)
            member = tarfile.TarInfo("./source/file")
            member.size = len(b"unsafe root")
            source.addfile(member, io.BytesIO(b"unsafe root"))
        archive, expected_sha256 = self.write_archive(contents.getvalue())
        destination = self.root / "dot-root"

        with self.assertRaisesRegex(ValueError, "archive"):
            self.mod.extract_workspace(archive, expected_sha256, destination)

        self.assertFalse(destination.exists())

    def test_extract_workspace_mismatch_leaves_destination_absent(self) -> None:
        archive, expected_sha256 = self.write_archive(self.archive_bytes({"pyproject.toml": b"[project]\n"}))
        destination = self.root / "checksum-mismatch"
        wrong_sha256 = "0" * 64 if expected_sha256 != "0" * 64 else "f" * 64

        with self.assertRaisesRegex(ValueError, "checksum"):
            self.mod.extract_workspace(archive, wrong_sha256, destination)

        self.assertFalse(destination.exists())

    def test_extract_workspace_creates_byte_identical_fresh_workspaces(self) -> None:
        contents = self.archive_bytes({"pyproject.toml": b"[project]\n", "src/module.py": b"answer = 42\n"})
        archive, expected_sha256 = self.write_archive(contents)
        first = self.root / "first"
        second = self.root / "second"

        self.mod.extract_workspace(archive, expected_sha256, first)
        self.mod.extract_workspace(archive, expected_sha256, second)

        self.assertEqual((first / "pyproject.toml").read_bytes(), (second / "pyproject.toml").read_bytes())
        self.assertEqual((first / "src" / "module.py").read_bytes(), (second / "src" / "module.py").read_bytes())
        self.assertNotEqual((first / "src" / "module.py").stat().st_ino, (second / "src" / "module.py").stat().st_ino)


class QualificationTests(unittest.TestCase):
    def setUp(self) -> None:
        self.tempdir = tempfile.TemporaryDirectory()
        self.root = Path(self.tempdir.name)
        self.cache = self.root / "cache"
        self.dataset = self.root / "dataset"
        self.mod = load_script("statefulbench_realworld.py")
        self.manifest = self._write_fixture()

    def tearDown(self) -> None:
        self.tempdir.cleanup()

    @staticmethod
    def _tools() -> dict[str, str]:
        return {
            "python": "Python 3.14.6",
            "omp": "omp 16.4.2",
            "stateful": "sha256:" + "a" * 64,
            "git": "git version 2.50.0",
            "rustc": "rustc 1.90.0",
            "cargo": "cargo 1.90.0",
        }

    def _patch(self, before: str, after: str, path: str = "target.py") -> str:
        if path.endswith(".py"):
            before = f"value = {before!r}"
            after = f"value = {after!r}"
        return (
            f"diff --git a/{path} b/{path}\n"
            f"--- a/{path}\n"
            f"+++ b/{path}\n"
            "@@ -1 +1 @@\n"
            f"-{before}\n"
            f"+{after}\n"
        )

    def _write_fixture(self) -> Path:
        tasks = []
        target = "base"
        for index in range(10):
            key = f"task-{index}"
            target = f"{target} {key}"
            evaluator = self.dataset / "evaluators" / f"{key}.py"
            evaluator.parent.mkdir(parents=True, exist_ok=True)
            evaluator.write_text(
                "import sys\nfrom pathlib import Path\n"
                f"assert {key!r} in (Path(sys.argv[1]) / 'target.py').read_text()\n",
                encoding="utf-8",
            )
            patch = self.dataset / "references" / f"{key}.patch"
            patch.parent.mkdir(parents=True, exist_ok=True)
            patch.write_text(self._patch("base", f"base {key}"), encoding="utf-8")
            tasks.append(
                {
                    "key": key,
                    "kind": "bug" if index < 5 else "feature",
                    "sources": [f"https://github.com/example/project/issues/{index}"],
                    "source_hash": hashlib.sha256(f"body {index}".encode("utf-8")).hexdigest(),
                    "prompt": key,
                    "acceptance": ["normal", "boundary", "error"],
                    "overlap_anchors": [{"path": "target.py", "symbol": "target.value"}],
                    "evaluator": f"evaluators/{key}.py",
                    "reference_patch": f"references/{key}.patch",
                }
            )
        (self.dataset / "references" / "integrated.patch").write_text(
            self._patch("base", target)
            + self._patch("base", "integrated", "suite.txt"),
            encoding="utf-8",
        )
        corpus = {
            "repository": "fixture",
            "issue_snapshot": "issues/fixture.json",
            "tasks": tasks,
            "final_prompt": "fix",
            "evaluators": [task["evaluator"] for task in tasks],
            "integrated_reference_patch": "references/integrated.patch",
        }
        issue_snapshot = self.dataset / "issues" / "fixture.json"
        issue_snapshot.parent.mkdir(parents=True, exist_ok=True)
        issue_snapshot.write_text(
            json.dumps(
                [
                    {
                        "html_url": f"https://github.com/example/project/issues/{index}",
                        "body": f"body {index}",
                    }
                    for index in range(10)
                ]
            ),
            encoding="utf-8",
        )
        corpus_path = self.dataset / "repos" / "fixture.json"
        corpus_path.parent.mkdir(parents=True, exist_ok=True)
        corpus_path.write_text(json.dumps(corpus), encoding="utf-8")

        archive_bytes = io.BytesIO()
        with tarfile.open(fileobj=archive_bytes, mode="w:gz") as archive:
            root = tarfile.TarInfo("fixture")
            root.type = tarfile.DIRTYPE
            archive.addfile(root)
            for name, contents in {
                "target.py": b"value = 'base'\n",
                "suite.txt": b"base\n",
                "pytest.py": (
                    b"from pathlib import Path\n"
                    b"raise SystemExit(0 if 'base' in Path('target.py').read_text() else 1)\n"
                ),
            }.items():
                member = tarfile.TarInfo(f"fixture/{name}")
                member.size = len(contents)
                archive.addfile(member, io.BytesIO(contents))
        contents = archive_bytes.getvalue()
        digest = hashlib.sha256(contents).hexdigest()
        self.cache.mkdir()
        (self.cache / f"{digest}.tar.gz").write_bytes(contents)
        repository = {
            "key": "fixture",
            "requested_url": "https://github.com/example/fixture",
            "canonical_url": "https://github.com/example/fixture",
            "commit": "0" * 40,
            "archive_url": "https://github.com/example/fixture/archive/" + "0" * 40 + ".tar.gz",
            "archive_sha256": digest,
            "python": "3.14.6",
            "setup": [sys.executable, "-c", "pass"],
            "suite": ["python", "-m", "pytest", "-q"],
            "corpus": "repos/fixture.json",
        }
        manifest = {
            "schema_version": 1,
            "generated_at": "now",
            "repositories": [
                {**repository, "key": f"fixture-{index}"} for index in range(10)
            ],
        }
        manifest["repositories"][0]["key"] = "fixture"
        manifest_path = self.dataset / "manifest.json"
        manifest_path.write_text(json.dumps(manifest), encoding="utf-8")
        return manifest_path
    def test_qualification_git_helper_ignores_host_config_and_rejects_whitespace_errors(self) -> None:
        workspace = self.root / "workspace"
        artifacts: dict[str, dict[str, str]] = {}
        with (
            mock.patch.object(self.mod, "_sanitized_environment", return_value={"PATH": "/usr/bin"}),
            mock.patch.object(
                self.mod,
                "_run_logged",
                return_value=subprocess.CompletedProcess([], 0, "", ""),
            ) as run_logged,
        ):
            self.mod._run_qualification_git(
                ["apply", "--index", "--whitespace=error-all", "patch.diff"],
                workspace,
                artifacts,
                self.root,
                "git-apply",
            )

        argv = run_logged.call_args.args[0]
        environment = run_logged.call_args.kwargs["env"]
        self.assertEqual(argv[0], "git")
        self.assertIn("core.hooksPath=/dev/null", argv)
        self.assertIn("core.autocrlf=false", argv)
        self.assertIn("--whitespace=error-all", argv)
        self.assertEqual(environment["GIT_CONFIG_NOSYSTEM"], "1")
        self.assertEqual(environment["GIT_CONFIG_GLOBAL"], "/dev/null")


    def _qualify(self) -> tuple[int, dict]:
        runtime_env = {
            "STATEFULBENCH_DOCKER_INNER": "qualification",
            "STATEFULBENCH_IMAGE_ID": "sha256:fixture",
            "STATEFULBENCH_IMAGE_PLATFORM": "linux/arm64",
            "STATEFULBENCH_IMAGE_REPO_DIGESTS": "[]",
        }
        runtime = self.mod._DOCKER.DockerRuntime(
            binary="docker",
            image="statefulbench-realworld:local",
            image_id="sha256:fixture",
            repo_digests=(),
            platform="linux/arm64",
        )
        stdout = io.StringIO()
        with (
            mock.patch.dict(self.mod.os.environ, runtime_env, clear=False),
            mock.patch.object(
                self.mod, "_inner_qualification_runtime", return_value=runtime
            ),
            mock.patch.object(
                self.mod,
                "_qualification_tool_provenance",
                return_value={
                    "python": "Python 3.14.6",
                    "omp": "omp 16.4.2",
                    "stateful": "sha256:" + "a" * 64,
                    "git": "git version 2.50.0",
                    "rustc": "rustc 1.90.0",
                    "cargo": "cargo 1.90.0",
                },
            ),
            contextlib.redirect_stdout(stdout),
        ):
            status = self.mod.main(
                [
                    "qualify",
                    "--manifest",
                    str(self.manifest),
                    "--cache",
                    str(self.cache),
                    "--repo",
                    "fixture",
                    "--docker-image",
                    "statefulbench-realworld:local",
                ]
            )
        return status, json.loads(stdout.getvalue())

    def test_qualification_receipt_is_atomic_and_fails_closed_on_stale_identity(self) -> None:
        repository = self.mod.load_manifest(self.manifest)["repositories"][0]
        corpus = self.dataset / repository["corpus"]
        manifest_text = self.manifest.read_text(encoding="utf-8")
        corpus_text = corpus.read_text(encoding="utf-8")
        runtime = self.mod._DOCKER.DockerRuntime(
            binary="/docker",
            image="statefulbench-realworld:local",
            image_id="sha256:abc",
            repo_digests=("statefulbench@sha256:def",),
            platform="linux/arm64",
        )

        receipt = self.mod.write_qualification_receipt(
            self.cache,
            repository,
            self.manifest,
            corpus,
            runtime,
            tool_provenance=self._tools(),
        )
        self.assertEqual(
            self.mod.load_qualification_receipt(
                self.cache, repository, self.manifest, corpus, runtime
            ),
            receipt,
        )
        receipt_path = self.cache / "qualification" / "receipts" / "fixture.json"
        self.assertTrue(receipt_path.is_file())

        self.manifest.write_text("{}\n", encoding="utf-8")
        with self.assertRaisesRegex(ValueError, "manifest"):
            self.mod.load_qualification_receipt(
                self.cache, repository, self.manifest, corpus, runtime
            )

        self.manifest.write_text(manifest_text, encoding="utf-8")
        corpus.write_text("{}\n", encoding="utf-8")
        with self.assertRaisesRegex(ValueError, "corpus"):
            self.mod.load_qualification_receipt(
                self.cache, repository, self.manifest, corpus, runtime
            )

        corpus.write_text(corpus_text, encoding="utf-8")
        stale_archive = {**repository, "archive_sha256": "f" * 64}
        with self.assertRaisesRegex(ValueError, "archive"):
            self.mod.load_qualification_receipt(
                self.cache, stale_archive, self.manifest, corpus, runtime
            )
        stale_runtime = self.mod._DOCKER.DockerRuntime(
            binary="/docker",
            image=runtime.image,
            image_id="sha256:def",
            repo_digests=runtime.repo_digests,
            platform=runtime.platform,
        )
        with self.assertRaisesRegex(ValueError, "image"):
            self.mod.load_qualification_receipt(
                self.cache, repository, self.manifest, corpus, stale_runtime
            )
        stale_platform = self.mod._DOCKER.DockerRuntime(
            binary="/docker",
            image=runtime.image,
            image_id=runtime.image_id,
            repo_digests=runtime.repo_digests,
            platform="linux/amd64",
        )
        with self.assertRaisesRegex(ValueError, "platform"):
            self.mod.load_qualification_receipt(
                self.cache, repository, self.manifest, corpus, stale_platform
            )

    def test_qualification_receipt_binds_every_frozen_graded_input(self) -> None:
        repository = self.mod.load_manifest(self.manifest)["repositories"][0]
        corpus_path = self.dataset / repository["corpus"]
        corpus = self.mod.load_corpus(corpus_path)
        runtime = self.mod._DOCKER.DockerRuntime(
            binary="/docker",
            image="statefulbench-realworld:local",
            image_id="sha256:abc",
            repo_digests=(),
            platform="linux/arm64",
        )
        self.mod.write_qualification_receipt(
            self.cache,
            repository,
            self.manifest,
            corpus_path,
            runtime,
            tool_provenance=self._tools(),
        )
        inputs = [
            self.dataset / corpus["issue_snapshot"],
            self.dataset / corpus["tasks"][0]["evaluator"],
            self.dataset / corpus["tasks"][0]["reference_patch"],
            self.dataset / corpus["integrated_reference_patch"],
        ]
        for path in inputs:
            original = path.read_bytes()
            path.write_bytes(original + (b"\n" if path == inputs[0] else b"\n# stale input\n"))
            with self.assertRaisesRegex(ValueError, "graded_inputs"):
                self.mod.load_qualification_receipt(
                    self.cache, repository, self.manifest, corpus_path, runtime
                )
            path.write_bytes(original)

    def test_live_input_guard_rejects_mutation_between_evaluator_copy_checks(self) -> None:
        repository = self.mod.load_manifest(self.manifest)["repositories"][0]
        corpus_path = self.dataset / repository["corpus"]
        corpus = self.mod.load_corpus(corpus_path)
        admitted = self.mod._graded_input_hashes(corpus_path, corpus)
        self.mod._require_graded_inputs(
            corpus_path, corpus, admitted, "before evaluator injection"
        )
        (self.dataset / corpus["tasks"][0]["evaluator"]).write_text(
            "raise AssertionError('mutated')\n", encoding="utf-8"
        )
        with self.assertRaisesRegex(RuntimeError, "during evaluator injection"):
            self.mod._require_graded_inputs(
                corpus_path, corpus, admitted, "during evaluator injection"
            )

    def test_private_staging_consumes_admitted_bytes_after_source_restore(self) -> None:
        repository = self.mod.load_manifest(self.manifest)["repositories"][0]
        corpus_path = self.dataset / repository["corpus"]
        corpus = self.mod.load_corpus(corpus_path)
        admitted = self.mod._graded_input_hashes(corpus_path, corpus)
        evaluator = self.dataset / corpus["tasks"][0]["evaluator"]
        original = evaluator.read_bytes()

        with self.mod._staged_graded_inputs(corpus_path, corpus, admitted) as staged_root:
            evaluator.write_bytes(b"raise AssertionError('swapped')\n")
            evaluator.write_bytes(original)
            self.assertEqual(
                (staged_root / corpus["tasks"][0]["evaluator"]).read_bytes(),
                original,
            )
            self.assertEqual(
                self.mod._graded_input_hashes(
                    staged_root / corpus_path.relative_to(self.dataset), corpus
                ),
                admitted,
            )

    def test_dataset_stage_preserves_manifest_and_corpus_after_source_mutation(self) -> None:
        manifest_before = self.manifest.read_bytes()
        corpus_path = self.dataset / "repos" / "fixture.json"
        corpus_before = corpus_path.read_bytes()

        with self.mod._staged_dataset_tree(self.manifest) as staged_manifest:
            self.manifest.write_text("{}\n", encoding="utf-8")
            corpus_path.write_text(
                json.dumps({"final_prompt": "mutated"}), encoding="utf-8"
            )
            self.assertEqual(staged_manifest.read_bytes(), manifest_before)
            staged_corpus = self.mod.load_corpus(
                staged_manifest.parent / "repos" / "fixture.json"
            )
            self.assertEqual(staged_corpus["final_prompt"], "fix")
            self.assertEqual(
                (staged_manifest.parent / "repos" / "fixture.json").read_bytes(),
                corpus_before,
            )


    def test_qualification_consumes_private_staged_inputs(self) -> None:
        original = self.mod.qualify_repository

        def qualify_then_mutate(*args, **kwargs):
            result = original(*args, **kwargs)
            (self.dataset / "evaluators" / "task-0.py").write_text(
                "raise AssertionError('mutated')\n", encoding="utf-8"
            )
            return result

        with mock.patch.object(self.mod, "qualify_repository", side_effect=qualify_then_mutate):
            status, result = self._qualify()

        self.assertEqual(status, 0, result)
        receipt = json.loads(
            (self.cache / "qualification" / "receipts" / "fixture.json").read_text(
                encoding="utf-8"
            )
        )
        self.assertNotEqual(
            receipt["graded_inputs"]["evaluators"]["evaluators/task-0.py"],
            self.mod._sha256(self.dataset / "evaluators" / "task-0.py"),
        )
    def test_qualification_receipt_records_complete_tool_provenance(self) -> None:
        repository = self.mod.load_manifest(self.manifest)["repositories"][0]
        corpus_path = self.dataset / repository["corpus"]
        runtime = self.mod._DOCKER.DockerRuntime(
            binary="/docker",
            image="statefulbench-realworld:local",
            image_id="sha256:abc",
            repo_digests=(),
            platform="linux/arm64",
        )
        tools = {
            "python": "Python 3.14.6",
            "omp": "omp 16.4.2",
            "stateful": "sha256:" + "a" * 64,
            "git": "git version 2.50.0",
            "rustc": "rustc 1.90.0",
            "cargo": "cargo 1.90.0",
        }
        receipt = self.mod.write_qualification_receipt(
            self.cache,
            repository,
            self.manifest,
            corpus_path,
            runtime,
            tool_provenance=tools,
        )
        self.assertEqual(receipt["tool_provenance"], tools)

    def test_qualification_receipt_rejects_incomplete_tool_provenance(self) -> None:
        repository = self.mod.load_manifest(self.manifest)["repositories"][0]
        runtime = self.mod._DOCKER.DockerRuntime(
            binary="/docker",
            image="statefulbench-realworld:local",
            image_id="sha256:abc",
            repo_digests=(),
            platform="linux/arm64",
        )
        with self.assertRaisesRegex(ValueError, "tool_provenance"):
            self.mod.write_qualification_receipt(
                self.cache,
                repository,
                self.manifest,
                self.dataset / repository["corpus"],
                runtime,
                tool_provenance={"python": "Python 3.14.6"},
            )

    def test_credential_seed_is_private_and_cleans_up_after_copy_error(self) -> None:
        arm_dir = self.root / "retained-arm"
        seed_dir = self.root / "system-temp-seed"
        seed_dir.mkdir(mode=0o700)

        def copy_then_fail(_source: Path, target: Path) -> None:
            target.mkdir(parents=True, exist_ok=True)
            (target / "agent.db").write_bytes(b"credential")
            raise RuntimeError("copy failed")

        with (
            mock.patch.object(self.mod.tempfile, "mkdtemp", return_value=str(seed_dir)),
            mock.patch.object(self.mod._LITE, "copy_stateful_omp_agent_db", side_effect=copy_then_fail),
        ):
            with self.assertRaisesRegex(RuntimeError, "copy failed"):
                self.mod._seed_shared_credential(arm_dir)

        self.assertFalse(seed_dir.exists())
        self.assertFalse((arm_dir / ".credential-seed").exists())

    def test_credential_seed_is_private_system_temporary_file(self) -> None:
        arm_dir = self.root / "retained-arm"
        seed_dir = self.root / "system-temp-seed"
        seed_dir.mkdir(mode=0o700)

        def copy(_source: Path, target: Path) -> None:
            target.mkdir(parents=True, exist_ok=True)
            (target / "agent.db").write_bytes(b"credential")

        with (
            mock.patch.object(self.mod.tempfile, "mkdtemp", return_value=str(seed_dir)),
            mock.patch.object(self.mod._LITE, "copy_stateful_omp_agent_db", side_effect=copy),
        ):
            credential = self.mod._seed_shared_credential(arm_dir)

        self.assertEqual(credential, seed_dir / "agent.db")
        self.assertEqual(seed_dir.stat().st_mode & 0o777, 0o700)
        self.assertEqual(credential.stat().st_mode & 0o777, 0o600)
        self.assertFalse((arm_dir / ".credential-seed").exists())
        shutil.rmtree(seed_dir)

    def test_failed_qualification_removes_existing_receipt(self) -> None:
        repository = self.mod.load_manifest(self.manifest)["repositories"][0]
        runtime = self.mod._DOCKER.DockerRuntime(
            binary="docker",
            image="statefulbench-realworld:local",
            image_id="sha256:fixture",
            repo_digests=(),
            platform="linux/arm64",
        )
        self.mod.write_qualification_receipt(
            self.cache,
            repository,
            self.manifest,
            self.dataset / repository["corpus"],
            runtime,
            tool_provenance=self._tools(),
        )
        (self.dataset / "evaluators" / "task-0.py").write_text(
            "pass\n", encoding="utf-8"
        )

        status, _ = self._qualify()

        self.assertEqual(status, 1)
        self.assertFalse(
            (self.cache / "qualification" / "receipts" / "fixture.json").exists()
        )

    def test_outer_qualification_only_runs_the_docker_gate(self) -> None:
        runtime = self.mod._DOCKER.DockerRuntime(
            binary="/docker",
            image="statefulbench-realworld:local",
            image_id="sha256:abc",
            repo_digests=(),
            platform="linux/arm64",
        )
        with (
            mock.patch.object(self.mod._DOCKER, "inspect_runtime", return_value=runtime),
            mock.patch.object(
                self.mod._DOCKER, "run_qualification_container", return_value=17
            ) as run_container,
            mock.patch.object(
                self.mod, "qualify_repository", side_effect=AssertionError("host qualification")
            ),
        ):
            status = self.mod.main(
                [
                    "qualify",
                    "--manifest",
                    str(MANIFEST),
                    "--cache",
                    str(self.cache),
                    "--repo",
                    "requests",
                    "--docker-image",
                    runtime.image,
                ]
            )

        self.assertEqual(status, 17)
        self.assertEqual(run_container.call_args.args[4], ("requests",))

    def test_public_run_stages_dataset_despite_inherited_staging_sentinel(self) -> None:
        stage_calls = []

        @contextlib.contextmanager
        def stage(manifest: Path):
            stage_calls.append(manifest)
            yield manifest

        with (
            mock.patch.dict(
                self.mod.os.environ, {"STATEFULBENCH_STAGED_DATASET": "1"}, clear=False
            ),
            mock.patch.object(self.mod, "_staged_dataset_tree", side_effect=stage),
            mock.patch.object(self.mod._DOCKER, "inspect_runtime", return_value=mock.Mock()),
            mock.patch.object(self.mod, "load_manifest", return_value={}),
            mock.patch.object(self.mod, "repo_entries", return_value=()),
            contextlib.redirect_stdout(io.StringIO()),
        ):
            status = self.mod.main(
                [
                    "run",
                    "--manifest",
                    str(self.root / "manifest.json"),
                    "--cache",
                    str(self.cache),
                    "--out",
                    str(self.root / "out"),
                    "--docker-image",
                    "statefulbench-realworld:local",
                ]
            )

        self.assertEqual(status, 1)
        self.assertEqual(stage_calls, [self.root / "manifest.json"])

    def test_inner_qualification_rejects_host_sentinel(self) -> None:
        environment = {
            "STATEFULBENCH_DOCKER_INNER": "qualification",
            "STATEFULBENCH_IMAGE_ID": "sha256:fixture",
            "STATEFULBENCH_IMAGE_PLATFORM": "linux/arm64",
            "STATEFULBENCH_SERVER_PLATFORM": "linux/arm64",
            "STATEFULBENCH_IMAGE_REPO_DIGESTS": "[]",
        }
        with (
            mock.patch.dict(self.mod.os.environ, environment, clear=False),
            mock.patch.object(Path, "is_file", return_value=False),
            mock.patch.object(
                self.mod, "load_manifest", side_effect=AssertionError("host bypass")
            ),
            contextlib.redirect_stderr(io.StringIO()),
        ):
            status = self.mod.main(
                [
                    "qualify",
                    "--manifest",
                    "/benchmark/datasets/statefulbench-realworld/manifest.json",
                    "--cache",
                    "/cache",
                    "--docker-image",
                    "statefulbench-realworld:local",
                ]
            )

        self.assertEqual(status, 1)

    def test_inner_qualification_accepts_container_marker_and_paths(self) -> None:
        environment = {
            "STATEFULBENCH_IMAGE_ID": "sha256:fixture",
            "STATEFULBENCH_IMAGE_PLATFORM": "linux/arm64",
            "STATEFULBENCH_SERVER_PLATFORM": "linux/arm64",
            "STATEFULBENCH_IMAGE_REPO_DIGESTS": "[]",
        }
        with (
            mock.patch.dict(self.mod.os.environ, environment, clear=False),
            mock.patch.object(Path, "is_file", return_value=True),
        ):
            runtime = self.mod._inner_qualification_runtime(
                "statefulbench-realworld:local",
                "docker",
                Path("/benchmark/datasets/statefulbench-realworld/manifest.json"),
                Path("/cache"),
            )

        self.assertEqual(runtime.image_id, "sha256:fixture")

    def test_inner_qualification_rejects_environment_staging_sentinel(self) -> None:
        environment = {
            "STATEFULBENCH_STAGED_DATASET": "1",
            "STATEFULBENCH_IMAGE_ID": "sha256:fixture",
            "STATEFULBENCH_IMAGE_PLATFORM": "linux/arm64",
            "STATEFULBENCH_SERVER_PLATFORM": "linux/arm64",
            "STATEFULBENCH_IMAGE_REPO_DIGESTS": "[]",
        }
        with (
            mock.patch.dict(self.mod.os.environ, environment, clear=True),
            mock.patch.object(Path, "is_file", return_value=True),
            self.assertRaisesRegex(ValueError, "manifest must be under /benchmark"),
        ):
            self.mod._inner_qualification_runtime(
                "statefulbench-realworld:local",
                "docker",
                Path("/private/staged/manifest.json"),
                Path("/cache"),
            )

    def test_inner_qualification_recursion_accepts_module_private_staging(self) -> None:
        environment = {
            "STATEFULBENCH_DOCKER_INNER": "qualification",
            "STATEFULBENCH_IMAGE_ID": "sha256:fixture",
            "STATEFULBENCH_IMAGE_PLATFORM": "linux/arm64",
            "STATEFULBENCH_SERVER_PLATFORM": "linux/arm64",
            "STATEFULBENCH_IMAGE_REPO_DIGESTS": "[]",
        }
        staged_manifest = Path("/private/staged/manifest.json")
        stage_calls = []

        @contextlib.contextmanager
        def stage(manifest: Path):
            stage_calls.append(manifest)
            yield staged_manifest

        with (
            mock.patch.dict(self.mod.os.environ, environment, clear=True),
            mock.patch.object(Path, "is_file", return_value=True),
            mock.patch.object(self.mod, "_staged_dataset_tree", side_effect=stage),
            mock.patch.object(self.mod, "load_manifest", return_value={}),
            mock.patch.object(self.mod, "repo_entries", return_value=()),
            mock.patch.object(
                self.mod,
                "_qualification_tool_provenance",
                return_value={
                    "python": "Python 3.14.6",
                    "omp": "omp 16.4.2",
                    "stateful": "sha256:" + "a" * 64,
                    "git": "git version 2.50.0",
                    "rustc": "rustc 1.90.0",
                    "cargo": "cargo 1.90.0",
                },
            ),
            contextlib.redirect_stderr(io.StringIO()),
            contextlib.redirect_stdout(io.StringIO()),
        ):
            status = self.mod.main(
                [
                    "qualify",
                    "--manifest",
                    "/benchmark/datasets/statefulbench-realworld/manifest.json",
                    "--cache",
                    "/cache",
                    "--docker-image",
                    "statefulbench-realworld:local",
                ]
            )

        self.assertEqual(status, 0)
        self.assertEqual(
            stage_calls,
            [Path("/benchmark/datasets/statefulbench-realworld/manifest.json")],
        )

    def test_dataset_stage_cleanup_invalidates_receipt_written_by_inner_qualification(self) -> None:
        runtime = self.mod._DOCKER.DockerRuntime(
            binary="docker",
            image="statefulbench-realworld:local",
            image_id="sha256:fixture",
            repo_digests=(),
            platform="linux/arm64",
        )
        tools = self._tools()
        original_manifest = self.manifest.read_bytes()
        swapped_manifest = json.loads(original_manifest)
        swapped_manifest["repositories"] = swapped_manifest["repositories"][:1]
        receipt_root = self.cache / "qualification" / "receipts"
        receipt_root.mkdir(parents=True)
        (receipt_root / "unrelated.json").write_text("{}\n", encoding="utf-8")

        @contextlib.contextmanager
        def stage(manifest: Path):
            yield manifest
            self.manifest.write_text(json.dumps(swapped_manifest), encoding="utf-8")
            raise OSError("dataset stage cleanup failed")

        qualified = {
            "key": "fixture",
            "error": None,
            "tasks": [],
            "base_suite_green": True,
            "integrated_green": True,
            "upstream_green": True,
            "isolated_tasks": [],
            "artifacts": {},
        }
        with (
            mock.patch.dict(
                self.mod.os.environ,
                {"STATEFULBENCH_DOCKER_INNER": "qualification"},
                clear=False,
            ),
            mock.patch.object(Path, "is_file", return_value=True),
            mock.patch.object(self.mod, "_staged_dataset_tree", side_effect=stage),
            mock.patch.object(self.mod, "_corpus_matches_repository", return_value=True),
            mock.patch.object(
                self.mod, "_inner_qualification_runtime", return_value=runtime
            ),
            mock.patch.object(
                self.mod, "_qualification_tool_provenance", return_value=tools
            ),
            mock.patch.object(
                self.mod, "qualify_repository", return_value=qualified
            ),
            contextlib.redirect_stdout(io.StringIO()),
            contextlib.redirect_stderr(io.StringIO()),
        ):
            status = self.mod.main(
                [
                    "qualify",
                    "--manifest",
                    str(self.manifest),
                    "--cache",
                    str(self.cache),
                    "--repo",
                    "fixture",
                    "--repo",
                    "fixture-1",
                    "--docker-image",
                    runtime.image,
                ]
            )
        self.manifest.write_bytes(original_manifest)

        self.assertEqual(status, 1)
        for key in ("fixture", "fixture-1", "unrelated"):
            self.assertFalse((receipt_root / f"{key}.json").exists())

    def test_outer_qualification_invalidates_receipt_before_docker_failure(self) -> None:
        runtime = self.mod._DOCKER.DockerRuntime(
            binary="/docker",
            image="statefulbench-realworld:local",
            image_id="sha256:abc",
            repo_digests=(),
            platform="linux/arm64",
        )
        receipt = self.cache / "qualification" / "receipts" / "requests.json"
        receipt.parent.mkdir(parents=True)
        receipt.write_text("{}\n", encoding="utf-8")
        with (
            mock.patch.object(self.mod._DOCKER, "inspect_runtime", return_value=runtime),
            mock.patch.object(
                self.mod._DOCKER, "run_qualification_container", return_value=19
            ),
        ):
            status = self.mod.main(
                [
                    "qualify",
                    "--manifest",
                    str(MANIFEST),
                    "--cache",
                    str(self.cache),
                    "--repo",
                    "requests",
                    "--docker-image",
                    runtime.image,
                ]
            )

        self.assertEqual(status, 19)
        self.assertFalse(receipt.exists())

    def test_run_rejects_missing_receipt_before_creating_rows(self) -> None:
        runtime = self.mod._DOCKER.DockerRuntime(
            binary="/docker",
            image="statefulbench-realworld:local",
            image_id="sha256:abc",
            repo_digests=(),
            platform="linux/arm64",
        )
        out = self.root / "out"
        with (
            mock.patch.object(self.mod._DOCKER, "inspect_runtime", return_value=runtime),
            mock.patch.object(self.mod._LITE, "resolve_omp_binary", return_value="/omp"),
            mock.patch.object(
                self.mod, "run_repo_arm", side_effect=AssertionError("must not run")
            ),
            contextlib.redirect_stderr(io.StringIO()),
        ):
            status = self.mod.main(
                [
                    "run",
                    "--manifest",
                    str(self.manifest),
                    "--cache",
                    str(self.cache),
                    "--out",
                    str(out),
                    "--repos",
                    "fixture",
                    "--arms",
                    "sequential",
                    "--docker-image",
                    runtime.image,
                ]
            )

        self.assertEqual(status, 1)
        self.assertFalse(out.exists())

    def test_qualify_rejects_base_green_evaluator(self) -> None:
        evaluator = self.dataset / "evaluators" / "task-0.py"
        evaluator.write_text("pass\n", encoding="utf-8")

        status, result = self._qualify()

        self.assertEqual(status, 1)
        self.assertFalse(result["repositories"][0]["tasks"][0]["base_red"])

    def test_qualify_rejects_reference_red_evaluator(self) -> None:
        (self.dataset / "references" / "task-0.patch").write_text(
            self._patch("base", "base wrong"),
            encoding="utf-8",
        )

        status, result = self._qualify()

        self.assertEqual(status, 1)
        self.assertFalse(result["repositories"][0]["tasks"][0]["reference_green"])

    def test_qualify_rejects_integrated_evaluator_failure(self) -> None:
        integrated = self.dataset / "references" / "integrated.patch"
        integrated.write_text(
            self._patch("base", "base " + " ".join(f"task-{index}" for index in range(1, 10)))
            + self._patch("base", "integrated", "suite.txt"),
            encoding="utf-8",
        )

        status, result = self._qualify()

        self.assertEqual(status, 1)
        self.assertFalse(result["repositories"][0]["integrated_green"])

    def test_qualify_rejects_upstream_suite_failure(self) -> None:
        with mock.patch.object(self.mod, "_run_suite", side_effect=[True, False]):
            status, result = self._qualify()

        self.assertEqual(status, 1)
        self.assertFalse(result["repositories"][0]["upstream_green"])

    def test_qualify_rejects_base_suite_failure(self) -> None:
        with mock.patch.object(self.mod, "_run_suite", return_value=False):
            status, result = self._qualify()

        self.assertEqual(status, 1)
        self.assertFalse(result["repositories"][0]["base_suite_green"])

    def test_qualify_rejects_corpus_repository_mismatch(self) -> None:
        corpus_path = self.dataset / "repos" / "fixture.json"
        corpus = json.loads(corpus_path.read_text(encoding="utf-8"))
        corpus["repository"] = "other"
        corpus_path.write_text(json.dumps(corpus), encoding="utf-8")

        status, result = self._qualify()

        self.assertEqual(status, 1)
        self.assertIn("repository", result["repositories"][0]["error"])

    def test_qualify_uses_isolated_sanitized_virtualenv(self) -> None:
        manifest = json.loads(self.manifest.read_text(encoding="utf-8"))
        manifest["repositories"][0]["environment"] = {"PROJECT_SETTING": "enabled"}
        self.manifest.write_text(json.dumps(manifest), encoding="utf-8")
        evaluator = self.dataset / "evaluators" / "task-0.py"
        evaluator.write_text(
            "import os\nimport sys\nfrom pathlib import Path\n"
            "assert Path(sys.prefix).resolve() == Path(os.environ['VIRTUAL_ENV']).resolve()\n"
            "assert 'PYTHONPATH' not in os.environ\n"
            "assert 'PYTHONHOME' not in os.environ\n"
            "assert os.environ['PROJECT_SETTING'] == 'enabled'\n"
            f"assert os.environ['PIP_CACHE_DIR'] == {str(self.cache / 'pip-cache')!r}\n"
            "assert 'task-0' in (Path(sys.argv[1]) / 'target.py').read_text()\n",
            encoding="utf-8",
        )

        status, result = self._qualify()

        repository = result["repositories"][0]
        self.assertTrue(repository["base_suite_green"], result)
        self.assertTrue(repository["tasks"][0]["reference_green"], result)
        self.assertTrue(repository["integrated_green"], result)
        self.assertFalse(repository["isolated_tasks"], result)
        self.assertEqual(status, 0, result)
        self.assertEqual(
            set(result["tool_provenance"]),
            {"python", "omp", "stateful", "git", "rustc", "cargo"},
        )
        self.assertTrue((self.cache / "pip-cache").is_dir())

    def test_changed_anchors_require_hunks_to_touch_the_symbol(self) -> None:
        source = self.root / "symbol-workspace" / "src" / "pkg" / "mod.py"
        source.parent.mkdir(parents=True)
        source.write_text(
            "class Class:\n"
            "    def first(self):\n"
            "        return 'base'\n\n"
            "    def second(self):\n"
            "        return 'base'\n",
            encoding="utf-8",
        )
        self.assertEqual(
            self.mod.changed_anchor_symbols(
                source,
                [
                    (source, "src/pkg/mod.py", "pkg.mod.Class.first"),
                    (source, "src/pkg/mod.py", "pkg.mod.Class.second"),
                ],
                [(6, 1)],
            ),
            {"src/pkg/mod.py:pkg.mod.Class.second"},
        )
        package_init = source.parent / "__init__.py"
        package_init.write_text("def from_timestamp():\n    return 'base'\n", encoding="utf-8")
        self.assertEqual(
            self.mod.changed_anchor_symbols(
                package_init,
                [(package_init, "src/pkg/__init__.py", "pkg.from_timestamp")],
                [(2, 1)],
            ),
            {"src/pkg/__init__.py:pkg.from_timestamp"},
        )


    def test_changed_anchors_include_decorated_function_decorators(self) -> None:
        source = self.root / "symbol-workspace" / "src" / "pkg" / "decorated.py"
        source.parent.mkdir(parents=True)
        source.write_text(
            "@pytest.fixture(scope='session')\n"
            "def shared() -> str:\n"
            "    return 'base'\n",
            encoding="utf-8",
        )
        anchors = [(source, "src/pkg/decorated.py", "pkg.decorated.shared")]
        expected = {"src/pkg/decorated.py:pkg.decorated.shared"}
        self.assertEqual(
            self.mod.changed_anchor_symbols(source, anchors, [(1, 1)]),
            expected,
        )
        self.assertEqual(
            self.mod.changed_anchor_symbols(source, anchors, [(2, 1)]),
            expected,
        )

    def test_qualify_reports_malformed_matching_archive(self) -> None:
        contents = b"not a tar archive"
        digest = hashlib.sha256(contents).hexdigest()
        archive = next(self.cache.glob("*.tar.gz"))
        archive.unlink()
        (self.cache / f"{digest}.tar.gz").write_bytes(contents)
        manifest = json.loads(self.manifest.read_text(encoding="utf-8"))
        for repository in manifest["repositories"]:
            repository["archive_sha256"] = digest
        self.manifest.write_text(json.dumps(manifest), encoding="utf-8")

        status, result = self._qualify()

        self.assertEqual(status, 1)
        repository = result["repositories"][0]
        self.assertIn("archive", repository["error"])
        self.assertTrue((self.cache / "qualification" / "fixture" / "artifacts").is_dir())

    def test_qualify_rejects_task_without_changed_shared_anchor(self) -> None:
        evaluator = self.dataset / "evaluators" / "task-0.py"
        evaluator.write_text(
            "import sys\nfrom pathlib import Path\n"
            "assert 'task-0' in (Path(sys.argv[1]) / 'lonely.py').read_text()\n",
            encoding="utf-8",
        )
        corpus_path = self.dataset / "repos" / "fixture.json"
        corpus = json.loads(corpus_path.read_text(encoding="utf-8"))
        corpus["tasks"][0]["overlap_anchors"].append(
            {"path": "lonely.py", "symbol": "lonely.value"}
        )
        corpus_path.write_text(json.dumps(corpus), encoding="utf-8")
        (self.dataset / "references" / "task-0.patch").write_text(
            self._patch("base", "base task-0", "lonely.py"),
            encoding="utf-8",
        )
        integrated = self.dataset / "references" / "integrated.patch"
        integrated.write_text(
            self._patch("base", "base " + " ".join(f"task-{index}" for index in range(1, 10)))
            + self._patch("base", "base task-0", "lonely.py")
            + self._patch("base", "integrated", "suite.txt"),
            encoding="utf-8",
        )
        archive_path = next(self.cache.glob("*.tar.gz"))
        archive_path.unlink()
        archive_bytes = io.BytesIO()
        with tarfile.open(fileobj=archive_bytes, mode="w:gz") as source:
            root = tarfile.TarInfo("fixture")
            root.type = tarfile.DIRTYPE
            source.addfile(root)
            for name, contents in {
                "target.py": b"value = 'base'\n",
                "lonely.py": b"value = 'base'\n",
                "suite.txt": b"base\n",
            }.items():
                member = tarfile.TarInfo(f"fixture/{name}")
                member.size = len(contents)
                source.addfile(member, io.BytesIO(contents))
        archive_bytes = archive_bytes.getvalue()
        digest = hashlib.sha256(archive_bytes).hexdigest()
        (self.cache / f"{digest}.tar.gz").write_bytes(archive_bytes)
        manifest = json.loads(self.manifest.read_text(encoding="utf-8"))
        for repository in manifest["repositories"]:
            repository["archive_sha256"] = digest
        self.manifest.write_text(json.dumps(manifest), encoding="utf-8")

        status, result = self._qualify()

        self.assertEqual(status, 1, result)
        repository = result["repositories"][0]
        self.assertEqual(
            repository["tasks"][0]["changed_anchors"], ["lonely.py:lonely.value"]
        )
        self.assertEqual(repository["isolated_tasks"], ["task-0"])
        self.assertTrue((self.cache / "qualification" / "fixture" / "artifacts").is_dir())


class RealWorldRunnerTests(unittest.TestCase):
    def setUp(self) -> None:
        self.tempdir = tempfile.TemporaryDirectory()
        self.root = Path(self.tempdir.name)
        self.mod = load_script("statefulbench_realworld.py")
        self.repo = {
            "key": "fixture",
            "archive_sha256": "0" * 64,
            "python": "3.14.6",
            "setup": ["python", "-c", "pass"],
            "suite": ["python", "-c", "pass"],
        }
        self.corpus = {
            "repository": "fixture",
            "final_prompt": "repair every task",
            "tasks": [
                {
                    "key": f"task-{index}",
                    "prompt": f"implement task {index}",
                    "evaluator": f"evaluators/task-{index}.py",
                }
                for index in range(10)
            ],
        }
        self.dataset = self.root / "dataset"
        for task in self.corpus["tasks"]:
            evaluator = self.dataset / task["evaluator"]
            evaluator.parent.mkdir(parents=True, exist_ok=True)
            evaluator.write_text("print('evaluator')\n", encoding="utf-8")

    def tearDown(self) -> None:
        self.tempdir.cleanup()

    @staticmethod
    def coordination_aggregate() -> dict:
        return {
            "notifications": {
                "by_kind": {
                    "reservation_granted": {
                        "created": 2,
                        "delivered": 1,
                        "pending": 1,
                        "expired": 0,
                    },
                    "scope_overlap": {
                        "created": 2,
                        "delivered": 1,
                        "pending": 1,
                        "expired": 0,
                    },
                }
            },
            "waits": {
                "by_final_status": {"claimed": 1, "queued": 1},
                "grant_wait_time_s": {
                    "count": 1,
                    "total": 2.5,
                    "mean": 2.5,
                    "max": 2.5,
                },
                "unmeasured_grants": 1,
            },
            "authorization": {
                "denied_by_reason": {"active_claim_conflict": 1},
                "warned_by_reason": {"missing_claim": 1},
            },
        }

    @classmethod
    def container_diagnostics(cls, _container, phase):
        context_render_counts = {
            "initialized": 0,
            "before-tasks": 2,
            "after-tasks": 11,
            "after-final": 16,
            "after-grading": 16,
            "before-remove": 16,
        }
        databases = {}
        if phase == "after-final":
            databases = {
                ".stateful/state.db": {
                    "integrity": "ok",
                    "coordination_metrics": cls.coordination_aggregate(),
                }
            }
        return {
            "schema_version": 1,
            "phase": phase,
            "home": "/home/stateful",
            "files": [],
            "databases": databases,
            "lock_files": [],
            "per_agent_home_tree": False,
            "processes": [],
            "runtime_metrics": {
                "context_render_success_count": context_render_counts[phase],
            },
        }

    @contextlib.contextmanager
    def workspace(self, *_args):
        workspace = self.root / "workspace"
        workspace.mkdir(exist_ok=True)
        yield workspace, Path(sys.executable), {}

    def fake_launch(self, events, final_check=None, exit_codes=None, timeout_agents=None):
        exit_codes = exit_codes or {}
        timeout_agents = timeout_agents or set()

        class Process:
            def __init__(self, agent_id):
                self.agent_id = agent_id
                self.returncode = None
                self.pid = 999999

            def wait(self, timeout=None):
                events.append(("wait", self.agent_id))
                if self.agent_id in timeout_agents and self.returncode is None:
                    self.returncode = -9
                    raise subprocess.TimeoutExpired(self.agent_id, timeout)
                self.returncode = exit_codes.get(self.agent_id, self.returncode or 0)
                return self.returncode

        def launch(arm_dir, workspace, agent_id, prompt_path, mode, cfg):
            if agent_id == "final" and final_check is not None:
                final_check(workspace)
            events.append(("launch", agent_id, mode))
            log = arm_dir / "logs" / f"{agent_id}.stdout.log"
            log.parent.mkdir(parents=True, exist_ok=True)
            log.write_text("", encoding="utf-8")
            return self.mod.AgentHandle(Process(agent_id), agent_id, 0.0)

        return launch

    def run_arm(self, arm, events, **kwargs):
        suite_ok = kwargs.pop("suite_ok", True)
        evaluator = kwargs.pop("evaluator", lambda *_: True)
        suite = kwargs.pop("suite", lambda *_: suite_ok)
        return self.mod.run_repo_arm(
            self.repo,
            self.corpus,
            self.dataset,
            self.root / "cache",
            self.root / "out",
            arm,
            self.mod.RunConfig(tasks=10, stateful_binary="/tmp/stateful"),
            launch=self.fake_launch(events, **kwargs.pop("launch_kwargs", {})),
            workspace_factory=self.workspace,
            archive_loader=lambda *_: self.root / "archive.tar.gz",
            setup=lambda *_: True,
            evaluator=evaluator,
            suite=suite,
            **kwargs,
        )

    def run_container_with_diagnostics(self, arm, diagnostics):
        runtime = self.mod._DOCKER.DockerRuntime(
            binary="/docker",
            image="statefulbench-realworld:local",
            image_id="sha256:fixture",
            repo_digests=(),
            platform="linux/arm64",
        )
        workspace = self.root / f"workspace-{arm}"
        workspace.mkdir(exist_ok=True)
        container = self.mod._DOCKER.ArmContainer(
            runtime, "container-1", arm, workspace, self.root / "runtime"
        )
        starts = []
        waits = []

        def execute(_container, *argv, **_kwargs):
            if argv == ("sha256sum", "/usr/local/bin/stateful"):
                return subprocess.CompletedProcess(
                    [], 0, "a" * 64 + "  /usr/local/bin/stateful\n", ""
                )
            versions = {
                ("python3", "--version"): "Python 3.14.6\n",
                ("/usr/local/bin/omp", "--version"): "omp 1\n",
                ("git", "--version"): "git version 2.50.0\n",
                ("rustc", "--version"): "rustc 1.90.0\n",
                ("cargo", "--version"): "cargo 1.90.0\n",
            }
            return subprocess.CompletedProcess([], 0, versions.get(argv, ""), "")

        def launch(_container, _arm_dir, agent_id, _prompt, _cfg, _env):
            starts.append(agent_id)
            return object()

        def wait(_container, _handle, _arm_dir, kind, _cfg):
            agent_id = starts[len(waits)]
            waits.append(kind)
            return (
                {
                    "agent_id": agent_id,
                    "kind": kind,
                    "exit_code": 0,
                    "timed_out": False,
                    "cleanup_error": None,
                    "wall_time_s": 0.0,
                    "total_tokens": 0,
                    "tool_calls": 0,
                    "context_render_tool_calls": (
                        2
                        if kind == "final"
                        else int(agent_id in {"task-0", "task-1", "task-2"})
                    ),
                },
                float(len(waits)),
            )

        return self.mod.run_repo_arm(
            self.repo,
            self.corpus,
            self.dataset,
            self.root / "cache",
            self.root / "out",
            arm,
            self.mod.RunConfig(
                tasks=10,
                stateful_binary="/usr/local/bin/stateful",
                omp_bin="/usr/local/bin/omp",
            ),
            runtime=runtime,
            archive_loader=lambda *_: self.root / "archive.tar.gz",
            workspace_materializer=lambda *_: workspace,
            arm_container_start=mock.Mock(return_value=container),
            arm_runtime_prepare=mock.Mock(
                return_value={
                    "HOME": "/home/stateful",
                    "PI_CODING_AGENT_DIR": "/home/stateful/.omp/profiles/stateful/agent",
                }
            ),
            arm_container_remove=mock.Mock(),
            container_exec=execute,
            container_agent_launch=launch,
            container_agent_wait=wait,
            container_evaluator_inject=mock.Mock(),
            container_post_checks=mock.Mock(
                return_value=(
                    True,
                    True,
                    [{"key": f"task-{index}", "ok": True} for index in range(10)],
                )
            ),
            container_diagnostics=diagnostics,
            container_inspect=mock.Mock(
                return_value={
                    "id": "container-1",
                    "image_id": "sha256:fixture",
                    "state": {
                        "status": "running",
                        "pid": 42,
                        "started_at": "2026-07-13T00:00:00Z",
                        "finished_at": "",
                    },
                }
            ),
        )

    def test_runtime_arm_serializes_admitted_qualification_identity_in_summary(self) -> None:
        runtime = self.mod._DOCKER.DockerRuntime(
            binary="/docker",
            image="statefulbench-realworld:local",
            image_id="sha256:fixture",
            repo_digests=(),
            platform="linux/arm64",
        )
        container = self.mod._DOCKER.ArmContainer(
            runtime, "container-1", "arm", self.root / "workspace", self.root / "runtime"
        )
        starts = []
        waits = []
        snapshots = []
        inspect = mock.Mock(
            return_value={
                "id": "container-1",
                "image_id": "sha256:fixture",
                "state": {
                    "status": "running",
                    "pid": 42,
                    "started_at": "2026-07-13T00:00:00Z",
                    "finished_at": "",
                },
            }
        )

        def diagnostics(container, phase):
            snapshots.append(phase)
            return self.container_diagnostics(container, phase)


        def launch(container, arm_dir, agent_id, prompt, cfg, env):
            starts.append((agent_id, env["HOME"], prompt.name))
            return object()

        def wait(container, handle, arm_dir, kind, cfg):
            waits.append(kind)
            return (
                {
                    "agent_id": starts[len(waits) - 1][0],
                    "kind": kind,
                    "exit_code": 0,
                    "timed_out": False,
                    "cleanup_error": None,
                    "wall_time_s": 0.0,
                    "total_tokens": 0,
                    "tool_calls": 0,
                },
                float(len(waits)),
            )


        def inject(*_args):
            self.assertEqual(waits, ["task"] * 10)
            self.assertEqual(len(starts), 10)

        def post_checks(*_args, **kwargs):
            self.assertEqual(waits, ["task"] * 10 + ["final"])
            self.assertEqual(len(starts), 11)
            self.assertEqual(snapshots[-1], "after-final")
            self.assertTrue(kwargs["inject"])
            return True, True, [{"key": f"task-{index}", "ok": True} for index in range(10)]
        def execute(_container, *argv, **_kwargs):
            if argv == ("/usr/local/bin/stateful", "--version"):
                return subprocess.CompletedProcess([], 2, "", "unexpected argument --version")
            if argv == ("sha256sum", "/usr/local/bin/stateful"):
                return subprocess.CompletedProcess(
                    [], 0, "a" * 64 + "  /usr/local/bin/stateful\n", ""
                )
            versions = {
                ("python3", "--version"): "Python 3.14.6\n",
                ("/usr/local/bin/omp", "--version"): "omp 1\n",
                ("git", "--version"): "git version 2.50.0\n",
                ("rustc", "--version"): "rustc 1.90.0\n",
                ("cargo", "--version"): "cargo 1.90.0\n",
            }
            return subprocess.CompletedProcess([], 0, versions.get(argv, ""), "")
        admitted_identity = {
            "manifest_sha256": "m" * 64,
            "corpus_sha256": "c" * 64,
            "archive_sha256": "a" * 64,
            "commit": "f" * 40,
            "image_id": "sha256:fixture",
            "platform": "linux/arm64",
            "graded_inputs": {"evaluators": {"evaluators/task-0.py": "e" * 64}},
            "tool_provenance": {
                "python": "Python 3.14.6",
                "omp": "omp 1",
                "stateful": "sha256:" + "a" * 64,
                "git": "git version 2.50.0",
                "rustc": "rustc 1.90.0",
                "cargo": "cargo 1.90.0",
            },
        }
        qualification_receipt = {
            **admitted_identity,
            "qualified": True,
            "qualified_at": "2026-07-13T00:00:00Z",
        }
        input_guard = mock.patch.object(self.mod, "_require_graded_inputs")
        input_guard.start()
        self.addCleanup(input_guard.stop)


        result = self.mod.run_repo_arm(
            {**self.repo, "corpus": "repos/fixture.json"},
            self.corpus,
            self.dataset,
            self.root / "cache",
            self.root / "out",
            "parallel-off",
            self.mod.RunConfig(
                tasks=10,
                stateful_binary="/usr/local/bin/stateful",
                omp_bin="/usr/local/bin/omp",
            ),
            runtime=runtime,
            qualification_receipt=qualification_receipt,
            archive_loader=lambda *_: self.root / "archive.tar.gz",
            workspace_materializer=lambda *_: self.root / "workspace",
            arm_container_start=mock.Mock(return_value=container),
            arm_runtime_prepare=mock.Mock(return_value={"HOME": "/home/stateful", "PI_CODING_AGENT_DIR": "/home/stateful/.omp/profiles/stateful/agent"}),
            arm_container_remove=mock.Mock(),
            container_exec=execute,
            container_agent_launch=launch,
            container_agent_wait=wait,
            container_evaluator_inject=inject,
            container_post_checks=post_checks,
            container_diagnostics=diagnostics,
            container_inspect=inspect,
        )

        self.assertTrue(result["cleared"], result)
        self.assertEqual(
            [agent_id for agent_id, _, _ in starts],
            [f"task-{index}" for index in range(10)] + ["final"],
        )
        self.assertEqual([home for _, home, _ in starts], ["/home/stateful"] * 11)
        self.assertEqual(waits, ["task"] * 10 + ["final"])
        self.assertEqual(
            result["evaluator_results"],
            [{"key": f"task-{index}", "ok": True} for index in range(10)],
        )
        self.assertEqual(snapshots, list(self.mod._DOCKER.DIAGNOSTIC_PHASES))
        self.assertEqual(result["container"]["inspect"]["state"]["pid"], 42)
        self.assertEqual(
            set(result["diagnostics"]["phase_timestamps"]),
            set(self.mod._DOCKER.DIAGNOSTIC_PHASES),
        )
        self.assertEqual(
            result["runtime"]["versions"],
            {
                "python": "Python 3.14.6",
                "omp": "omp 1",
                "stateful": "sha256:" + "a" * 64,
                "git": "git version 2.50.0",
                "rustc": "rustc 1.90.0",
                "cargo": "cargo 1.90.0",
            },
        )
        self.assertEqual(result["qualification"], admitted_identity)
        summary = self.mod.build_run_summary(
            [
                {
                    "key": "fixture",
                    "commit": "f" * 40,
                    "archive_sha256": "a" * 64,
                }
            ],
            ["parallel-off"],
            1,
            "model",
            "high",
            [result],
            "2026-07-13T00:00:00Z",
        )
        self.assertEqual(summary["results"][0]["qualification"], admitted_identity)

    def test_coordination_metrics_assemble_parallel_on_phase_deltas_without_mutating_diagnostics(self) -> None:
        captured = {}

        def diagnostics(container, phase):
            snapshot = self.container_diagnostics(container, phase)
            captured[phase] = snapshot
            return snapshot

        result = self.run_container_with_diagnostics("parallel-on", diagnostics)
        aggregate = captured["after-final"]["databases"][".stateful/state.db"][
            "coordination_metrics"
        ]

        self.assertTrue(result["cleared"], result)
        self.assertEqual(
            result["coordination_metrics"]["context_renders"],
            {
                "server": {"tasks": 9, "final": 5, "total": 14},
                "explicit_tool_calls": {"tasks": 3, "final": 2, "total": 5},
            },
        )
        self.assertEqual(
            result["coordination_metrics"]["notifications"],
            aggregate["notifications"],
        )
        self.assertEqual(result["coordination_metrics"]["waits"], aggregate["waits"])
        self.assertEqual(
            result["coordination_metrics"]["authorization"],
            aggregate["authorization"],
        )
        result["coordination_metrics"]["waits"]["unmeasured_grants"] = 99
        self.assertEqual(aggregate["waits"]["unmeasured_grants"], 1)

    def test_coordination_metrics_are_null_for_off_arms(self) -> None:
        result = self.run_container_with_diagnostics(
            "parallel-off", self.container_diagnostics
        )

        self.assertTrue(result["cleared"], result)
        self.assertIsNone(result["coordination_metrics"])

    def test_coordination_metrics_decreasing_server_counts_unclear_row(self) -> None:
        def diagnostics(container, phase):
            snapshot = self.container_diagnostics(container, phase)
            if phase == "after-tasks":
                snapshot["runtime_metrics"]["context_render_success_count"] = 1
            return snapshot

        result = self.run_container_with_diagnostics("parallel-on", diagnostics)

        self.assertFalse(result["cleared"])
        self.assertIsNone(result["coordination_metrics"])
        self.assertEqual(result["error"], "context render counts decreased across phases")

    def test_coordination_metrics_malformed_database_evidence_unclears_row(self) -> None:
        def diagnostics(container, phase):
            snapshot = self.container_diagnostics(container, phase)
            if phase == "after-final":
                snapshot["databases"][".stateful/state.db"][
                    "coordination_metrics"
                ]["waits"]["unmeasured_grants"] = True
            return snapshot

        result = self.run_container_with_diagnostics("parallel-on", diagnostics)

        self.assertFalse(result["cleared"])
        self.assertIsNone(result["coordination_metrics"])
        self.assertEqual(result["error"], "coordination unmeasured grants is invalid")

    def test_coordination_metrics_missing_required_notification_kinds_unclear_row(self) -> None:
        for missing_kind in ("reservation_granted", "scope_overlap"):
            with self.subTest(missing_kind=missing_kind):
                def diagnostics(container, phase):
                    snapshot = self.container_diagnostics(container, phase)
                    if phase == "after-final":
                        snapshot["databases"][".stateful/state.db"][
                            "coordination_metrics"
                        ]["notifications"]["by_kind"].pop(missing_kind)
                    return snapshot

                result = self.run_container_with_diagnostics("parallel-on", diagnostics)

                self.assertFalse(result["cleared"])
                self.assertIsNone(result["coordination_metrics"])

    def test_coordination_metrics_inconsistent_notification_created_unclears_row(self) -> None:
        def diagnostics(container, phase):
            snapshot = self.container_diagnostics(container, phase)
            if phase == "after-final":
                snapshot["databases"][".stateful/state.db"][
                    "coordination_metrics"
                ]["notifications"]["by_kind"]["scope_overlap"]["created"] = 3
            return snapshot

        result = self.run_container_with_diagnostics("parallel-on", diagnostics)

        self.assertFalse(result["cleared"])
        self.assertIsNone(result["coordination_metrics"])

    def test_coordination_metrics_reject_invalid_and_ambiguous_evidence(self) -> None:
        snapshots = {
            phase: self.container_diagnostics(None, phase)
            for phase in ("before-tasks", "after-tasks", "after-final")
        }
        agents = [
            {"kind": "task", "context_render_tool_calls": 1},
            {"kind": "final", "context_render_tool_calls": 2},
        ]
        snapshots["before-tasks"]["runtime_metrics"][
            "context_render_success_count"
        ] = True
        with self.assertRaisesRegex(
            ValueError, "context render count at before-tasks is invalid"
        ):
            self.mod._build_coordination_metrics("parallel-on", snapshots, agents)

        snapshots["before-tasks"]["runtime_metrics"][
            "context_render_success_count"
        ] = 2
        snapshots["after-final"]["databases"]["another.db"] = {
            "integrity": "ok",
            "coordination_metrics": copy.deepcopy(
                snapshots["after-final"]["databases"][".stateful/state.db"][
                    "coordination_metrics"
                ]
            ),
        }
        with self.assertRaisesRegex(ValueError, "exactly one coordination metrics"):
            self.mod._build_coordination_metrics("parallel-on", snapshots, agents)

    def test_container_cleanup_error_blocks_final_and_grading(self) -> None:
        runtime = self.mod._DOCKER.DockerRuntime(
            binary="/docker",
            image="statefulbench-realworld:local",
            image_id="sha256:fixture",
            repo_digests=(),
            platform="linux/arm64",
        )
        workspace = self.root / "workspace"
        workspace.mkdir()
        container = self.mod._DOCKER.ArmContainer(
            runtime, "container-1", "arm", workspace, self.root / "runtime"
        )
        launched = []
        post_checks = mock.Mock(return_value=(True, True))

        def execute(_container, *argv, **_kwargs):
            versions = {
                ("python3", "--version"): "Python 3.14.6\n",
                ("/usr/local/bin/omp", "--version"): "omp 1\n",
                ("git", "--version"): "git version 2.50.0\n",
                ("rustc", "--version"): "rustc 1.90.0\n",
                ("cargo", "--version"): "cargo 1.90.0\n",
            }
            if argv == ("sha256sum", "/usr/local/bin/stateful"):
                return subprocess.CompletedProcess(
                    [], 0, "a" * 64 + "  /usr/local/bin/stateful\n", ""
                )
            return subprocess.CompletedProcess([], 0, versions.get(argv, ""), "")

        def launch(_container, _arm_dir, agent_id, _prompt, _cfg, _env):
            launched.append(agent_id)
            return type("Handle", (), {"agent_id": agent_id, "started_monotonic": 0.0})()

        def wait(_container, handle, _arm_dir, kind, _cfg):
            return (
                {
                    "agent_id": handle.agent_id,
                    "kind": kind,
                    "exit_code": -9,
                    "timed_out": True,
                    "cleanup_error": "entry subreaper did not reap all descendants before deadline",
                    "wall_time_s": 0.0,
                    "total_tokens": 0,
                    "tool_calls": 0,
                },
                0.0,
            )

        result = self.mod._run_container_repo_arm(
            self.repo,
            self.corpus,
            self.dataset,
            self.root / "cache",
            self.root / "out",
            "sequential",
            self.mod.RunConfig(
                tasks=10,
                stateful_binary="/usr/local/bin/stateful",
                omp_bin="/usr/local/bin/omp",
            ),
            runtime=runtime,
            archive_loader=lambda *_: self.root / "archive.tar.gz",
            arm_container_start=mock.Mock(return_value=container),
            arm_runtime_prepare=mock.Mock(
                return_value={
                    "HOME": "/home/stateful",
                    "PI_CODING_AGENT_DIR": "/home/stateful/.omp/profiles/stateful/agent",
                }
            ),
            arm_container_remove=mock.Mock(),
            credential_seed=None,
            workspace_materializer=lambda *_: workspace,
            container_exec=execute,
            container_agent_launch=launch,
            container_agent_wait=wait,
            container_evaluator_inject=mock.Mock(),
            container_post_checks=post_checks,
            container_diagnostics=self.container_diagnostics,
            container_inspect=mock.Mock(
                return_value={
                    "id": "container-1",
                    "image_id": "sha256:fixture",
                    "state": {
                        "status": "running",
                        "pid": 42,
                        "started_at": "2026-07-13T00:00:00Z",
                        "finished_at": "",
                    },
                }
            ),
        )

        self.assertEqual(launched, ["task-0"])
        post_checks.assert_not_called()
        self.assertFalse(result["cleared"])
        self.assertIn("entry subreaper did not reap", result["error"])

    def test_parallel_on_activates_stateful_after_container_git_init(self) -> None:
        runtime = self.mod._DOCKER.DockerRuntime(
            binary="/docker",
            image="statefulbench-realworld:local",
            image_id="sha256:fixture",
            repo_digests=(),
            platform="linux/arm64",
        )
        workspace = self.root / "workspace"
        workspace.mkdir()
        container = self.mod._DOCKER.ArmContainer(
            runtime, "container-1", "arm", workspace, self.root / "runtime"
        )
        events = []

        def prepare(_container, _arm, *, activate_stateful=True, **_kwargs):
            events.append(("prepare", activate_stateful))
            return {
                "HOME": "/home/stateful",
                "PI_CODING_AGENT_DIR": "/home/stateful/.omp/profiles/stateful/agent",
            }

        def execute(_container, *argv, **_kwargs):
            if argv == ("git", "init"):
                events.append(("git", "init"))
            if argv == ("sha256sum", "/usr/local/bin/stateful"):
                return subprocess.CompletedProcess(
                    [], 0, "b" * 64 + "  /usr/local/bin/stateful\n", ""
                )
            versions = {
                ("python3", "--version"): "Python 3.14.6\n",
                ("/usr/local/bin/omp", "--version"): "omp 1\n",
                ("git", "--version"): "git version 2.50.0\n",
                ("rustc", "--version"): "rustc 1.90.0\n",
                ("cargo", "--version"): "cargo 1.90.0\n",
            }
            return subprocess.CompletedProcess([], 0, versions.get(argv, ""), "")

        def launch(_container, _arm_dir, agent_id, _prompt, _cfg, _env):
            return type("Handle", (), {"agent_id": agent_id, "started_monotonic": 0.0})()

        def wait(_container, handle, _arm_dir, kind, _cfg):
            return (
                {
                    "agent_id": handle.agent_id,
                    "kind": kind,
                    "exit_code": 0,
                    "timed_out": False,
                    "cleanup_error": None,
                    "wall_time_s": 0.0,
                    "total_tokens": 0,
                    "tool_calls": 0,
                    "context_render_tool_calls": 0,
                },
                0.0,
            )

        result = self.mod.run_repo_arm(
            self.repo,
            self.corpus,
            self.dataset,
            self.root / "cache",
            self.root / "out",
            "parallel-on",
            self.mod.RunConfig(
                tasks=10,
                stateful_binary="/usr/local/bin/stateful",
                omp_bin="/usr/local/bin/omp",
            ),
            runtime=runtime,
            archive_loader=lambda *_: self.root / "archive.tar.gz",
            workspace_materializer=lambda *_: workspace,
            arm_container_start=mock.Mock(return_value=container),
            arm_runtime_prepare=prepare,
            arm_container_remove=mock.Mock(),
            container_exec=execute,
            container_agent_launch=launch,
            container_agent_wait=wait,
            container_evaluator_inject=mock.Mock(),
            container_post_checks=mock.Mock(
                return_value=(True, True, [{"key": f"task-{index}", "ok": True} for index in range(10)])
            ),
            container_diagnostics=self.container_diagnostics,
            container_inspect=mock.Mock(
                return_value={
                    "id": "container-1",
                    "image_id": "sha256:fixture",
                    "state": {
                        "status": "running",
                        "pid": 42,
                        "started_at": "2026-07-13T00:00:00Z",
                        "finished_at": "",
                    },
                }
            ),
        )

        self.assertTrue(result["cleared"], result)
        self.assertEqual(events.count(("prepare", False)), 1)
        self.assertEqual(events.count(("prepare", True)), 1)
        self.assertLess(events.index(("git", "init")), events.index(("prepare", True)))

    def test_runner_requires_a_docker_runtime_for_live_execution(self) -> None:
        archive_loader = mock.Mock(side_effect=AssertionError("host execution must not start"))

        result = self.mod.run_repo_arm(
            self.repo,
            self.corpus,
            self.dataset,
            self.root / "cache",
            self.root / "out",
            "parallel-off",
            self.mod.RunConfig(tasks=10),
            archive_loader=archive_loader,
        )

        self.assertEqual(result["error"], "Docker runtime is required for agent execution")
        archive_loader.assert_not_called()

    def test_runtime_arm_removes_container_when_initialization_fails(self) -> None:
        runtime = self.mod._DOCKER.DockerRuntime(
            binary="/docker",
            image="statefulbench-realworld:local",
            image_id="sha256:fixture",
            repo_digests=(),
            platform="linux/arm64",
        )
        container = object()
        start = mock.Mock(return_value=container)
        remove = mock.Mock()

        result = self.mod.run_repo_arm(
            self.repo,
            self.corpus,
            self.dataset,
            self.root / "cache",
            self.root / "out",
            "parallel-off",
            self.mod.RunConfig(tasks=10, stateful_binary="/tmp/stateful"),
            runtime=runtime,
            launch=mock.Mock(side_effect=AssertionError("must not launch")),
            workspace_factory=self.workspace,
            archive_loader=lambda *_: self.root / "archive.tar.gz",
            workspace_materializer=lambda *_: self.root / "workspace",
            setup=lambda *_: True,
            evaluator=lambda *_: True,
            suite=lambda *_: True,
            arm_container_start=start,
            arm_runtime_prepare=mock.Mock(side_effect=RuntimeError("initialization failed")),
            arm_container_remove=remove,
            container_diagnostics=self.container_diagnostics,
        )

        self.assertEqual(result["error"], "initialization failed; diagnostics: missing shared HOME evidence")
        remove.assert_called_once_with(container)


    def test_post_agent_evaluators_and_suite_use_container_exec_paths(self) -> None:
        runtime = self.mod._DOCKER.DockerRuntime(
            binary="/docker",
            image="statefulbench-realworld:local",
            image_id="sha256:fixture",
            repo_digests=(),
            platform="linux/arm64",
        )
        container = self.mod._DOCKER.ArmContainer(
            runtime, "container-id", "arm", self.root / "workspace", self.root / "runtime"
        )
        artifacts = {}
        artifact_dir = self.root / "artifacts"
        artifact_dir.mkdir()
        execute = mock.Mock(return_value=subprocess.CompletedProcess([], 0, "", ""))
        copy = mock.Mock()
        python = Path("/workspace/.statefulbench-venv/bin/python")

        evaluators_ok, suite_ok, evaluator_results = self.mod._run_container_post_agent_checks(
            self.repo,
            self.corpus,
            self.dataset,
            container,
            python,
            {"HOME": "/home/stateful"},
            artifacts,
            artifact_dir,
            execute=execute,
            copy=copy,
        )

        self.assertTrue(evaluators_ok)
        self.assertTrue(suite_ok)
        self.assertEqual(
            evaluator_results,
            [{"key": f"task-{index}", "ok": True} for index in range(10)],
        )
        commands = [call.args[1:] for call in execute.call_args_list]
        self.assertIn(
            (str(python), "/workspace/.statefulbench-evaluators/task-0.py", "/workspace"),
            commands,
        )
        self.assertIn((str(python), "-c", "pass"), commands)
        self.assertEqual(execute.call_args_list[-1].kwargs["timeout_s"], 900)
        self.assertEqual(copy.call_count, 10)

    def test_reinjecting_canonical_evaluators_removes_prior_read_only_copy(self) -> None:
        runtime = self.mod._DOCKER.DockerRuntime(
            binary="/docker",
            image="statefulbench-realworld:local",
            image_id="sha256:fixture",
            repo_digests=(),
            platform="linux/arm64",
        )
        container = self.mod._DOCKER.ArmContainer(
            runtime, "container-id", "arm", self.root / "workspace", self.root / "runtime"
        )
        artifacts = {}
        artifact_dir = self.root / "artifacts-reinject"
        artifact_dir.mkdir()
        commands = []

        def execute(_container, *argv, **_kwargs):
            commands.append(argv)
            return subprocess.CompletedProcess(argv, 0, "", "")

        def copy(_container, _source, destination):
            self.assertIn(("rm", "-f", destination), commands)

        self.mod._inject_container_evaluators(
            self.corpus,
            self.dataset,
            container,
            {"HOME": "/home/stateful"},
            artifacts,
            artifact_dir,
            execute=execute,
            copy=copy,
        )

    def test_sequential_waits_then_injects_evaluators_before_final_with_eleven_records(self) -> None:
        events = []

        def evaluators_visible(workspace):
            self.assertTrue((workspace / ".statefulbench-evaluators" / "task-0.py").is_file())
            self.assertEqual(len([event for event in events if event[0] == "wait"]), 10)

        result = self.run_arm(
            "sequential",
            events,
            launch_kwargs={"final_check": evaluators_visible},
        )

        launches = [event[1] for event in events if event[0] == "launch"]
        waits = [event[1] for event in events if event[0] == "wait"]
        self.assertEqual(launches, [f"task-{index}" for index in range(10)] + ["final"])
        self.assertEqual(waits, [f"task-{index}" for index in range(10)] + ["final"])
        for index in range(9):
            self.assertLess(
                events.index(("wait", f"task-{index}")),
                events.index(("launch", f"task-{index + 1}", "no-state")),
            )
        self.assertTrue(result["cleared"])
        self.assertEqual(len(result["agents"]), 11)
        self.assertTrue((self.root / "out" / "fixture" / "sequential" / "trial-1" / "results.json").is_file())

    def test_parallel_launches_ten_before_waits_and_starts_one_stateful_server(self) -> None:
        events = []

        @contextlib.contextmanager
        def server(*_args):
            events.append(("server", "start"))
            yield {"STATEFUL_SERVER_URL": "http://server"}
            events.append(("server", "stop"))

        result = self.run_arm("parallel-on", events, server=server)

        first_wait = next(index for index, event in enumerate(events) if event[0] == "wait")
        self.assertEqual(
            [event[1] for event in events[:first_wait] if event[0] == "launch"],
            [f"task-{index}" for index in range(10)],
        )
        self.assertEqual([event for event in events if event[0] == "server"], [("server", "start"), ("server", "stop")])
        self.assertTrue(result["cleared"])
        self.assertEqual(len(result["agents"]), 11)

    def test_post_suite_or_agent_failure_prevents_cleared(self) -> None:
        suite_events = []
        suite_failed = self.run_arm("parallel-off", suite_events, suite_ok=False)
        agent_events = []
        agent_failed = self.run_arm(
            "parallel-off",
            agent_events,
            launch_kwargs={"exit_codes": {"task-5": 1}},
        )

        self.assertFalse(suite_failed["cleared"])
        self.assertFalse(suite_failed["post_suite_ok"])
        self.assertFalse(agent_failed["cleared"])
        self.assertEqual(len(suite_failed["agents"]), 11)
        self.assertEqual(len(agent_failed["agents"]), 11)


    def test_failure_summary_derives_exact_agent_and_status_causes(self) -> None:
        cases = (
            (
                "nonzero",
                {"launch_kwargs": {"exit_codes": {"task-5": 1}}},
                "task agent task-5 exited with code 1",
            ),
            (
                "timeout",
                {"launch_kwargs": {"timeout_agents": {"task-5"}}},
                "task agent task-5 timed out",
            ),
            (
                "evaluator",
                {"evaluator": lambda path, *_: path.name != "task-0.py"},
                "evaluator failed: task-0",
            ),
            ("suite", {"suite_ok": False}, "upstream suite failed"),
            (
                "all-statuses",
                {
                    "launch_kwargs": {
                        "exit_codes": {"task-1": 1, "final": 2},
                        "timeout_agents": {"task-5"},
                    },
                    "evaluator": lambda path, *_: path.name
                    not in {"task-0.py", "task-2.py"},
                    "suite_ok": False,
                },
                "task agent task-1 exited with code 1; task agent task-5 timed out; "
                "final agent final exited with code 2; evaluator failed: task-0, task-2; "
                "upstream suite failed",
            ),
        )
        repository = {
            "key": "fixture",
            "commit": "0" * 40,
            "archive_sha256": "0" * 64,
        }
        for name, kwargs, expected in cases:
            with self.subTest(name=name), mock.patch.object(self.mod.os, "killpg"):
                result = self.run_arm("parallel-off", [], **kwargs)
                summary = self.mod.build_run_summary(
                    [repository],
                    ["parallel-off"],
                    1,
                    "model",
                    "thinking",
                    [result],
                    "2026-07-12T00:00:00Z",
                )

            self.assertIsNone(result["error"])
            self.assertEqual(summary["arms"][0]["error"], expected)
            self.assertEqual(
                summary["aggregates"][0]["failures"],
                [
                    {
                        "repo": "fixture",
                        "arm": "parallel-off",
                        "trial": 1,
                        "error": expected,
                    }
                ],
            )
            if name in {"nonzero", "timeout"}:
                failed = result["agents"][5]
                self.assertEqual(failed["agent_id"], "task-5")
                self.assertEqual(failed["timed_out"], name == "timeout")
                self.assertEqual(failed["exit_code"], -9 if name == "timeout" else 1)

    def test_parallel_launch_error_reaps_already_started_agents(self) -> None:
        events = []

        class Process:
            def __init__(self, agent_id):
                self.agent_id = agent_id
                self.pid = 1
                self.returncode = None

            def wait(self, timeout=None):
                events.append(("wait", self.agent_id))
                self.returncode = 0
                return 0

        def launch(arm_dir, workspace, agent_id, prompt_path, mode, cfg):
            if agent_id == "task-3":
                raise RuntimeError("launch failed")
            events.append(("launch", agent_id))
            log = arm_dir / "logs" / f"{agent_id}.stdout.log"
            log.parent.mkdir(parents=True, exist_ok=True)
            log.write_text("", encoding="utf-8")
            return self.mod.AgentHandle(Process(agent_id), agent_id, 0.0)

        result = self.mod.run_repo_arm(
            self.repo,
            self.corpus,
            self.dataset,
            self.root / "cache",
            self.root / "out",
            "parallel-off",
            self.mod.RunConfig(tasks=10, stateful_binary="/tmp/stateful"),
            launch=launch,
            workspace_factory=self.workspace,
            archive_loader=lambda *_: self.root / "archive.tar.gz",
            setup=lambda *_: True,
            evaluator=lambda *_: True,
            suite=lambda *_: True,
        )

        self.assertEqual(result["error"], "launch failed")
        self.assertEqual([event[1] for event in events if event[0] == "launch"], ["task-0", "task-1", "task-2"])
        self.assertEqual([event[1] for event in events if event[0] == "wait"], ["task-0", "task-1", "task-2"])
        self.assertNotIn(("launch", "final"), events)

    def test_agent_launch_environment_omits_rustup_home(self) -> None:
        events = []
        workspace_arguments = []
        workspace_env = {"PATH": "/toolchain/bin:/usr/bin", "RUSTUP_HOME": "/host/rustup"}

        @contextlib.contextmanager
        def workspace(*args):
            workspace_arguments.append(args)
            workspace = self.root / "workspace-with-rustup"
            workspace.mkdir(exist_ok=True)
            yield workspace, Path(sys.executable), workspace_env

        def launch(arm_dir, workspace, agent_id, prompt_path, mode, cfg):
            self.assertNotIn("RUSTUP_HOME", cfg.launch_env)
            return self.fake_launch(events)(arm_dir, workspace, agent_id, prompt_path, mode, cfg)

        result = self.mod.run_repo_arm(
            self.repo,
            self.corpus,
            self.dataset,
            self.root / "cache",
            self.root / "out",
            "parallel-off",
            self.mod.RunConfig(tasks=10, stateful_binary="/tmp/stateful"),
            launch=launch,
            workspace_factory=workspace,
            archive_loader=lambda *_: self.root / "archive.tar.gz",
            setup=lambda *_: True,
            evaluator=lambda *_: True,
            suite=lambda *_: True,
        )

        self.assertTrue(result["cleared"], result)
        self.assertEqual(
            workspace_arguments[0][-1], self.root / "cache" / "pip-cache"
        )
        self.assertTrue((self.root / "cache" / "pip-cache").is_dir())

    def test_repository_environment_reaches_setup_evaluators_suite_and_agents(self) -> None:
        events = []
        seen_environments = []
        venv = self.root / "workspace-venv"
        workspace_env = {"VIRTUAL_ENV": str(venv), "PATH": f"{venv / 'bin'}:/usr/bin"}
        self.repo["environment"] = {"PROJECT_SETTING": "enabled"}
        expected_env = {**workspace_env, "PROJECT_SETTING": "enabled"}

        @contextlib.contextmanager
        def workspace(*_args):
            workspace = self.root / "workspace-with-venv"
            workspace.mkdir(exist_ok=True)
            yield workspace, Path(sys.executable), workspace_env

        def launch(arm_dir, workspace, agent_id, prompt_path, mode, cfg):
            self.assertEqual(cfg.launch_env, expected_env)
            return self.fake_launch(events)(arm_dir, workspace, agent_id, prompt_path, mode, cfg)

        def check_environment(*args):
            seen_environments.append(args[3])
            return True

        result = self.mod.run_repo_arm(
            self.repo,
            self.corpus,
            self.dataset,
            self.root / "cache",
            self.root / "out",
            "parallel-off",
            self.mod.RunConfig(tasks=10, stateful_binary="/tmp/stateful"),
            launch=launch,
            workspace_factory=workspace,
            archive_loader=lambda *_: self.root / "archive.tar.gz",
            setup=check_environment,
            evaluator=lambda *args: check_environment(None, None, None, args[3]),
            suite=check_environment,
        )

        self.assertTrue(result["cleared"], result)
        self.assertEqual(seen_environments, [expected_env] * 12)

    def test_runner_replaces_untrusted_denied_root_with_manifest_root(self) -> None:
        events = []
        expected_root = self.dataset.resolve()

        def launch(arm_dir, workspace, agent_id, prompt_path, mode, cfg):
            self.assertEqual(cfg.denied_read_paths, (expected_root,))
            return self.fake_launch(events)(arm_dir, workspace, agent_id, prompt_path, mode, cfg)

        result = self.mod.run_repo_arm(
            self.repo,
            self.corpus,
            self.dataset,
            self.root / "cache",
            self.root / "out",
            "parallel-off",
            self.mod.RunConfig(
                tasks=10,
                stateful_binary="/tmp/stateful",
                denied_read_paths=(self.root / "untrusted-root",),
            ),
            launch=launch,
            workspace_factory=self.workspace,
            archive_loader=lambda *_: self.root / "archive.tar.gz",
            setup=lambda *_: True,
            evaluator=lambda *_: True,
            suite=lambda *_: True,
        )

        self.assertTrue(result["cleared"], result)

    def test_final_mutation_of_injected_evaluator_cannot_change_grading(self) -> None:
        events = []
        seen = []

        def mutate_injected(workspace):
            evaluator = workspace / ".statefulbench-evaluators" / "task-0.py"
            evaluator.unlink()
            evaluator.write_text("raise SystemExit(0)\n", encoding="utf-8")

        def evaluator(path, *_args):
            seen.append(path)
            self.assertEqual(path.read_text(encoding="utf-8"), "print('evaluator')\n")
            return True

        result = self.run_arm(
            "parallel-off",
            events,
            launch_kwargs={"final_check": mutate_injected},
            evaluator=evaluator,
        )

        self.assertTrue(result["cleared"], result)
        self.assertEqual(seen, [(self.dataset / f"evaluators/task-{index}.py").resolve() for index in range(10)])

    def test_agent_only_timing_excludes_delayed_evaluator_reinjection(self) -> None:
        for arm in ("sequential", "parallel-off"):
            with self.subTest(arm=arm):
                clock = [0.0]

                class Handle:
                    def __init__(self, agent_id):
                        self.agent_id = agent_id
                        self.started_monotonic = clock[0]

                def launch(*args):
                    return Handle(args[2])

                def wait(handle, _arm_dir, kind, _cfg):
                    duration = 7.0 if kind == "final" else 1.0
                    clock[0] += duration
                    return (
                        {
                            "agent_id": handle.agent_id,
                            "kind": kind,
                            "exit_code": 0,
                            "timed_out": False,
                            "cleanup_error": None,
                            "wall_time_s": duration,
                            "total_tokens": 0,
                            "tool_calls": 0,
                        },
                        clock[0],
                    )

                original_inject = self.mod._inject_evaluators

                def delayed_inject(*args):
                    result = original_inject(*args)
                    clock[0] += 100.0
                    return result

                with (
                    mock.patch.object(self.mod._LITE, "_wait_agent", side_effect=wait),
                    mock.patch.object(self.mod, "_inject_evaluators", side_effect=delayed_inject),
                ):
                    result = self.mod.run_repo_arm(
                        self.repo,
                        self.corpus,
                        self.dataset,
                        self.root / "cache",
                        self.root / "out",
                        arm,
                        self.mod.RunConfig(tasks=10, stateful_binary="/tmp/stateful"),
                        launch=launch,
                        workspace_factory=self.workspace,
                        archive_loader=lambda *_: self.root / "archive.tar.gz",
                        setup=lambda *_: True,
                        evaluator=lambda *_: True,
                        suite=lambda *_: True,
                    )

                self.assertEqual(result["tasks_wall_time_s"], 10.0)
                self.assertEqual(result["final_wall_time_s"], 7.0)
                self.assertEqual(result["arm_wall_time_s"], 17.0)

    def test_all_evaluators_and_suite_run_after_an_evaluator_failure(self) -> None:
        events = []
        calls = []

        def evaluator(path, *_args):
            calls.append(path.name)
            return path.name != "task-0.py"

        def suite(*_args):
            calls.append("suite")
            return True

        result = self.run_arm("parallel-off", events, evaluator=evaluator, suite=suite)

        self.assertFalse(result["evaluators_ok"])
        self.assertTrue(result["upstream_suite_ok"])
        self.assertEqual(calls, [f"task-{index}.py" for index in range(10)] + ["suite"])

    def test_run_help_lists_required_realworld_arguments(self) -> None:
        stdout = io.StringIO()
        with contextlib.redirect_stdout(stdout), self.assertRaises(SystemExit) as raised:
            self.mod.main(["run", "--help"])

        self.assertEqual(raised.exception.code, 0)
        self.assertIn("--manifest", stdout.getvalue())
        self.assertIn("--cache", stdout.getvalue())
        self.assertIn("--out", stdout.getvalue())
        self.assertIn("--repos", stdout.getvalue())
        self.assertIn("--arms", stdout.getvalue())
        self.assertIn("--trials", stdout.getvalue())

class RealWorldReportingTests(unittest.TestCase):
    def setUp(self) -> None:
        self.tempdir = tempfile.TemporaryDirectory()
        self.root = Path(self.tempdir.name)
        self.mod = load_script("statefulbench_realworld.py")
        self.repositories = [
            {
                "key": "alpha",
                "commit": "a" * 40,
                "archive_sha256": "b" * 64,
                "corpus": "repos/alpha.json",
            },
            {
                "key": "bravo",
                "commit": "c" * 40,
                "archive_sha256": "d" * 64,
                "corpus": "repos/bravo.json",
            },
        ]

    def tearDown(self) -> None:
        self.tempdir.cleanup()

    @staticmethod
    def result(
        repository: str,
        arm: str,
        trial: int,
        *,
        cleared: bool = True,
        error: str | None = None,
        wall: float = 0.0,
        tokens: int = 0,
        tools: int = 0,
        coordination_metrics: dict | None = None,
    ) -> dict:
        return {
            "repository": repository,
            "arm": arm,
            "trial": trial,
            "cleared": cleared,
            "error": error,
            "arm_wall_time_s": wall,
            "tasks_wall_time_s": wall / 2,
            "final_wall_time_s": wall / 4,
            "total_tokens": tokens,
            "total_tool_calls": tools,
            "coordination_metrics": coordination_metrics,
        }

    @staticmethod
    def admitted_receipt() -> tuple[dict, dict]:
        identity = {
            "manifest_sha256": "m" * 64,
            "corpus_sha256": "c" * 64,
            "archive_sha256": "b" * 64,
            "commit": "a" * 40,
            "image_id": "sha256:fixture",
            "platform": "linux/arm64",
            "graded_inputs": {"evaluators": {"evaluators/task-0.py": "e" * 64}},
            "tool_provenance": {
                "python": "Python 3.14.6",
                "omp": "omp 16.4.2",
                "stateful": "sha256:" + "a" * 64,
                "git": "git version 2.50.0",
                "rustc": "rustc 1.90.0",
                "cargo": "cargo 1.90.0",
            },
        }
        return {**identity, "qualified": True}, identity

    def test_summary_directly_sums_two_repository_rows_and_retains_exact_failures(self) -> None:
        rows = [
            self.result("alpha", "sequential", 1, wall=1.25, tokens=10, tools=2),
            self.result("alpha", "sequential", 2, wall=2.5, tokens=20, tools=3),
            self.result("alpha", "parallel-off", 1, wall=3.0, tokens=30, tools=4),
            self.result(
                "alpha",
                "parallel-off",
                2,
                cleared=False,
                error="final agent exited 1",
                wall=4.0,
                tokens=40,
                tools=5,
            ),
            self.result("bravo", "sequential", 1, wall=5.0, tokens=50, tools=6),
            self.result("bravo", "sequential", 2, wall=6.0, tokens=60, tools=7),
            self.result("bravo", "parallel-off", 1, wall=7.0, tokens=70, tools=8),
            self.result("bravo", "parallel-off", 2, wall=8.0, tokens=80, tools=9),
        ]

        summary = self.mod.build_run_summary(
            self.repositories,
            ["sequential", "parallel-off"],
            2,
            "model",
            "thinking",
            rows,
            "2026-07-12T00:00:00Z",
        )

        self.assertEqual(
            summary["repositories"][0],
            {"key": "alpha", "source_sha": "a" * 40, "archive_sha256": "b" * 64},
        )
        self.assertEqual(
            summary["arms"][3],
            {
                "repo": "alpha",
                "arm": "parallel-off",
                "trial": 2,
                "cleared": False,
                "wall_time_s": 4.0,
                "tokens": 40,
                "tool_calls": 5,
                "error": "final agent exited 1",
                "coordination_metrics": None,
            },
        )
        alpha_sequential = summary["aggregates"][0]
        self.assertEqual(alpha_sequential["repo"], "alpha")
        self.assertEqual(alpha_sequential["arm"], "sequential")
        self.assertEqual(alpha_sequential["row_count"], 2)
        self.assertEqual(alpha_sequential["cleared_count"], 2)
        self.assertEqual(alpha_sequential["wall_time_s"], 3.75)
        self.assertEqual(alpha_sequential["tasks_wall_time_s"], 1.875)
        self.assertEqual(alpha_sequential["final_wall_time_s"], 0.9375)
        self.assertEqual(alpha_sequential["tokens"], 30)
        self.assertEqual(alpha_sequential["tool_calls"], 5)
        self.assertEqual(
            summary["aggregates"][1]["failures"],
            [{"repo": "alpha", "arm": "parallel-off", "trial": 2, "error": "final agent exited 1"}],
        )
        self.assertEqual(summary["model"], "model")
        self.assertEqual(summary["thinking"], "thinking")
        self.assertEqual(summary["trials"], 2)
        self.assertEqual(summary["generated_at"], "2026-07-12T00:00:00Z")

    def test_coordination_metrics_aggregate_complete_parallel_on_trials(self) -> None:
        first = {
            "notifications": {
                "by_kind": {
                    "scope_overlap": {
                        "created": 2,
                        "delivered": 1,
                        "pending": 1,
                        "expired": 0,
                    },
                    "reservation_granted": {
                        "created": 0,
                        "delivered": 0,
                        "pending": 0,
                        "expired": 0,
                    },
                    "first_only": {
                        "created": 1,
                        "delivered": 1,
                        "pending": 0,
                        "expired": 0,
                    },
                }
            },
            "waits": {
                "by_final_status": {"claimed": 2},
                "grant_wait_time_s": {
                    "count": 2,
                    "total": 3.0,
                    "mean": 1.5,
                    "max": 2.5,
                },
                "unmeasured_grants": 1,
            },
            "authorization": {
                "denied_by_reason": {"active_claim_conflict": 1},
                "warned_by_reason": {},
            },
            "context_renders": {
                "server": {"tasks": 9, "final": 5, "total": 14},
                "explicit_tool_calls": {"tasks": 3, "final": 2, "total": 5},
            },
        }
        second = {
            "notifications": {
                "by_kind": {
                    "reservation_granted": {
                        "created": 3,
                        "delivered": 2,
                        "pending": 0,
                        "expired": 1,
                    },
                    "scope_overlap": {
                        "created": 0,
                        "delivered": 0,
                        "pending": 0,
                        "expired": 0,
                    },
                    "second_only": {
                        "created": 2,
                        "delivered": 0,
                        "pending": 1,
                        "expired": 1,
                    },
                }
            },
            "waits": {
                "by_final_status": {"queued": 1, "claimed": 3},
                "grant_wait_time_s": {
                    "count": 3,
                    "total": 12.0,
                    "mean": 4.0,
                    "max": 7.0,
                },
                "unmeasured_grants": 2,
            },
            "authorization": {
                "denied_by_reason": {"missing_claim": 2},
                "warned_by_reason": {"scope_overlap": 1},
            },
            "context_renders": {
                "server": {"tasks": 4, "final": 6, "total": 10},
                "explicit_tool_calls": {"tasks": 1, "final": 3, "total": 4},
            },
        }
        summary = self.mod.build_run_summary(
            [self.repositories[0]],
            ["parallel-on"],
            2,
            "model",
            "thinking",
            [
                self.result(
                    "alpha",
                    "parallel-on",
                    1,
                    coordination_metrics=first,
                ),
                self.result(
                    "alpha",
                    "parallel-on",
                    2,
                    coordination_metrics=second,
                ),
            ],
            "2026-07-12T00:00:00Z",
        )
        metrics = summary["aggregates"][0]["coordination_metrics"]

        self.assertEqual(
            metrics["notifications"]["by_kind"],
            {
                "first_only": {
                    "created": 1,
                    "delivered": 1,
                    "pending": 0,
                    "expired": 0,
                },
                "reservation_granted": {
                    "created": 3,
                    "delivered": 2,
                    "pending": 0,
                    "expired": 1,
                },
                "scope_overlap": {
                    "created": 2,
                    "delivered": 1,
                    "pending": 1,
                    "expired": 0,
                },
                "second_only": {
                    "created": 2,
                    "delivered": 0,
                    "pending": 1,
                    "expired": 1,
                },
            },
        )
        self.assertEqual(
            metrics["waits"]["by_final_status"], {"claimed": 5, "queued": 1}
        )
        self.assertEqual(
            metrics["authorization"]["denied_by_reason"],
            {"active_claim_conflict": 1, "missing_claim": 2},
        )
        self.assertEqual(
            metrics["authorization"]["warned_by_reason"], {"scope_overlap": 1}
        )
        self.assertEqual(
            metrics["context_renders"],
            {
                "server": {"tasks": 13, "final": 11, "total": 24},
                "explicit_tool_calls": {"tasks": 4, "final": 5, "total": 9},
            },
        )
        self.assertEqual(
            metrics["waits"]["grant_wait_time_s"],
            {"count": 5, "total": 15.0, "mean": 3.0, "max": 7.0},
        )
        self.assertEqual(metrics["waits"]["unmeasured_grants"], 3)

        malformed = copy.deepcopy(first)
        malformed["context_renders"]["server"]["tasks"] = True
        self.assertIsNone(
            self.mod._aggregate_coordination_metrics(
                "parallel-on", [malformed, second], 2
            )
        )

    def test_coordination_metrics_are_null_for_incomplete_and_off_aggregates(self) -> None:
        complete = {
            "notifications": {
                "by_kind": {
                    "reservation_granted": {
                        "created": 0,
                        "delivered": 0,
                        "pending": 0,
                        "expired": 0,
                    },
                    "scope_overlap": {
                        "created": 0,
                        "delivered": 0,
                        "pending": 0,
                        "expired": 0,
                    },
                }
            },
            "waits": {
                "by_final_status": {},
                "grant_wait_time_s": {
                    "count": 0,
                    "total": 0.0,
                    "mean": None,
                    "max": None,
                },
                "unmeasured_grants": 0,
            },
            "authorization": {
                "denied_by_reason": {},
                "warned_by_reason": {},
            },
            "context_renders": {
                "server": {"tasks": 0, "final": 0, "total": 0},
                "explicit_tool_calls": {"tasks": 0, "final": 0, "total": 0},
            },
        }
        incomplete = self.mod.build_run_summary(
            [self.repositories[0]],
            ["parallel-on"],
            2,
            "model",
            "thinking",
            [
                self.result(
                    "alpha",
                    "parallel-on",
                    1,
                    coordination_metrics=complete,
                )
            ],
            "2026-07-12T00:00:00Z",
        )
        duplicate_trials = self.mod.build_run_summary(
            [self.repositories[0]],
            ["parallel-on"],
            2,
            "model",
            "thinking",
            [
                self.result(
                    "alpha",
                    "parallel-on",
                    1,
                    coordination_metrics=complete,
                ),
                self.result(
                    "alpha",
                    "parallel-on",
                    1,
                    coordination_metrics=complete,
                ),
            ],
            "2026-07-12T00:00:00Z",
        )
        sequential = self.result("alpha", "sequential", 1)
        parallel_off = self.result("alpha", "parallel-off", 1)
        sequential.pop("coordination_metrics")
        parallel_off.pop("coordination_metrics")
        off_arms = self.mod.build_run_summary(
            [self.repositories[0]],
            ["sequential", "parallel-off"],
            1,
            "model",
            "thinking",
            [sequential, parallel_off],
            "2026-07-12T00:00:00Z",
        )

        self.assertIsNone(incomplete["aggregates"][0]["coordination_metrics"])
        self.assertIsNone(
            duplicate_trials["aggregates"][0]["coordination_metrics"]
        )
        self.assertEqual(
            [aggregate["coordination_metrics"] for aggregate in off_arms["aggregates"]],
            [None, None],
        )

    def test_summary_marks_missing_scheduled_rows_without_inventing_metrics(self) -> None:
        rows = [
            self.result("alpha", "sequential", 1, wall=1.0, tokens=1, tools=1),
            self.result("alpha", "parallel-off", 1, wall=2.0, tokens=2, tools=2),
            self.result("bravo", "sequential", 1, wall=3.0, tokens=3, tools=3),
        ]

        summary = self.mod.build_run_summary(
            self.repositories,
            ["sequential", "parallel-off"],
            1,
            "model",
            "thinking",
            rows,
            "2026-07-12T00:00:00Z",
        )

        missing = summary["aggregates"][3]
        self.assertEqual(missing["repo"], "bravo")
        self.assertEqual(missing["arm"], "parallel-off")
        self.assertEqual(missing["row_count"], 0)
        self.assertEqual(missing["cleared_count"], 0)
        self.assertEqual(missing["wall_time_s"], 0.0)
        self.assertEqual(missing["tokens"], 0)
        self.assertEqual(missing["tool_calls"], 0)
        self.assertEqual(
            missing["failures"],
            [{"repo": "bravo", "arm": "parallel-off", "trial": 1, "error": "missing result"}],
        )

    def test_run_persists_summary_and_prints_each_two_repository_arm_row(self) -> None:
        out_dir = self.root / "out"
        expected = [
            self.result("alpha", "sequential", 1, wall=1.0, tokens=11, tools=2),
            self.result("alpha", "parallel-off", 1, wall=2.0, tokens=12, tools=3),
            self.result("bravo", "sequential", 1, wall=3.0, tokens=13, tools=4),
            self.result(
                "bravo",
                "parallel-off",
                1,
                cleared=False,
                error="repository setup failed",
                wall=4.0,
                tokens=14,
                tools=5,
            ),
        ]
        results = iter(expected)
        stdout = io.StringIO()

        with (
            mock.patch.object(self.mod, "load_manifest", return_value={}),
            mock.patch.object(self.mod, "repo_entries", return_value=tuple(self.repositories)),
            mock.patch.object(
                self.mod,
                "load_corpus",
                side_effect=lambda path: {"repository": path.stem},
            ),
            mock.patch.object(self.mod, "_corpus_matches_repository", return_value=True),
            mock.patch.object(self.mod, "run_repo_arm", side_effect=lambda *_args, **_kwargs: next(results)),
            mock.patch.object(self.mod.time, "strftime", return_value="2026-07-12T00:00:00Z"),
            mock.patch.object(self.mod._DOCKER, "inspect_runtime", return_value=mock.Mock()),
            mock.patch.object(
                self.mod, "load_qualification_receipt", return_value={"graded_inputs": {}}
            ),
            mock.patch.object(
                self.mod,
                "_staged_graded_inputs",
                side_effect=lambda *_args: contextlib.nullcontext(self.root / "staged"),
            ),
            mock.patch.object(
                self.mod,
                "_staged_dataset_tree",
                side_effect=lambda path: contextlib.nullcontext(path),
            ),
            contextlib.redirect_stdout(stdout),
        ):
            status = self.mod.main(
                [
                    "run",
                    "--manifest",
                    str(self.root / "manifest.json"),
                    "--cache",
                    str(self.root / "cache"),
                    "--out",
                    str(out_dir),
                    "--repos",
                    "alpha,bravo",
                    "--arms",
                    "sequential,parallel-off",
                    "--model",
                    "model",
                    "--thinking",
                    "thinking",
                    "--docker-image",
                    "statefulbench-realworld:local",
                ]
            )

        self.assertEqual(status, 1)
        summary = json.loads((out_dir / "summary.json").read_text(encoding="utf-8"))
        self.assertEqual(summary["model"], "model")
        self.assertEqual(summary["thinking"], "thinking")
        self.assertEqual(summary["trials"], 1)
        self.assertEqual(
            summary["repositories"],
            [
                {"key": "alpha", "source_sha": "a" * 40, "archive_sha256": "b" * 64},
                {"key": "bravo", "source_sha": "c" * 40, "archive_sha256": "d" * 64},
            ],
        )
        self.assertEqual(
            summary["aggregates"][3]["failures"],
            [{"repo": "bravo", "arm": "parallel-off", "trial": 1, "error": "repository setup failed"}],
        )
        table = stdout.getvalue()
        self.assertIn("| repository | arm | trial | cleared |", table)
        self.assertIn("| bravo | parallel-off | 1 | False | 4.000 | 14 | 5 | repository setup failed |", table)

    def test_staged_input_cleanup_failure_rewrites_completed_rows_without_duplicates(self) -> None:
        for arms in (("sequential",), ("sequential", "parallel-off")):
            with self.subTest(completed_rows=len(arms)):
                out_dir = self.root / f"cleanup-{len(arms)}"
                results = [
                    {
                        **self.result(
                            "alpha",
                            arm,
                            1,
                            wall=float(index),
                            tokens=index,
                            tools=index + 1,
                        ),
                        "agents": [{"agent_id": f"agent-{index}"}],
                        "artifacts": {"run": {"stdout": f"artifacts/{arm}.log"}},
                    }
                    for index, arm in enumerate(arms, start=1)
                ]
                retained = json.loads(json.dumps(results))
                writes = []
                write_result = self.mod._write_run_result

                @contextlib.contextmanager
                def staged_inputs(*_args):
                    yield self.root / "staged"
                    raise OSError("staged cleanup failed")

                def record_write(out: Path, result: dict) -> None:
                    writes.append(json.loads(json.dumps(result)))
                    write_result(out, result)

                with (
                    mock.patch.object(self.mod, "load_manifest", return_value={}),
                    mock.patch.object(
                        self.mod, "repo_entries", return_value=(self.repositories[0],)
                    ),
                    mock.patch.object(
                        self.mod, "load_corpus", return_value={"repository": "alpha"}
                    ),
                    mock.patch.object(
                        self.mod, "_corpus_matches_repository", return_value=True
                    ),
                    mock.patch.object(
                        self.mod._DOCKER, "inspect_runtime", return_value=mock.Mock()
                    ),
                    mock.patch.object(
                        self.mod,
                        "load_qualification_receipt",
                        return_value={"graded_inputs": {}},
                    ),
                    mock.patch.object(self.mod, "_staged_dataset_tree", side_effect=contextlib.nullcontext),
                    mock.patch.object(
                        self.mod, "_staged_graded_inputs", side_effect=staged_inputs
                    ),
                    mock.patch.object(self.mod, "run_repo_arm", side_effect=results),
                    mock.patch.object(
                        self.mod, "_empty_run_result", wraps=self.mod._empty_run_result
                    ) as empty_run,
                    mock.patch.object(self.mod, "_write_run_result", side_effect=record_write),
                    contextlib.redirect_stdout(io.StringIO()),
                ):
                    status = self.mod.main(
                        [
                            "run",
                            "--manifest",
                            str(self.root / "manifest.json"),
                            "--cache",
                            str(self.root / "cache"),
                            "--out",
                            str(out_dir),
                            "--arms",
                            ",".join(arms),
                            "--docker-image",
                            "statefulbench-realworld:local",
                        ]
                    )

                self.assertEqual(status, 1)
                self.assertEqual(empty_run.call_count, 0)
                summary = json.loads((out_dir / "summary.json").read_text(encoding="utf-8"))
                scheduled = [("alpha", arm, 1) for arm in arms]
                self.assertEqual(
                    [
                        (result["repository"], result["arm"], result["trial"])
                        for result in summary["results"]
                    ],
                    scheduled,
                )
                self.assertEqual(
                    [
                        (row["repo"], row["arm"], row["trial"])
                        for row in summary["arms"]
                    ],
                    scheduled,
                )
                for original in retained:
                    key = (
                        original["repository"],
                        original["arm"],
                        original["trial"],
                    )
                    rewritten = [
                        result
                        for result in writes
                        if (result["repository"], result["arm"], result["trial"]) == key
                    ]
                    self.assertEqual(len(rewritten), 2)
                    self.assertTrue(rewritten[0]["cleared"])
                    self.assertFalse(rewritten[1]["cleared"])
                    self.assertEqual(rewritten[1]["error"], "staged cleanup failed")
                    self.assertEqual(rewritten[1]["agents"], original["agents"])
                    self.assertEqual(rewritten[1]["artifacts"], original["artifacts"])
                    self.assertEqual(
                        rewritten[1]["total_tokens"], original["total_tokens"]
                    )
                    persisted = json.loads(
                        (
                            out_dir
                            / original["repository"]
                            / original["arm"]
                            / f"trial-{original['trial']}"
                            / "results.json"
                        ).read_text(encoding="utf-8")
                    )
                    self.assertEqual(persisted, rewritten[1])

    def test_post_admission_empty_rows_retain_identity_for_staging_and_arm_failures(self) -> None:
        for name, staged_inputs, arm_failure in (
            (
                "pre-arm-staging",
                OSError("staged inputs failed"),
                AssertionError("run_repo_arm must not run"),
            ),
            (
                "arm-exception",
                lambda *_args: contextlib.nullcontext(self.root / "staged"),
                OSError("run_repo_arm failed"),
            ),
        ):
            with self.subTest(failure=name):
                receipt, identity = self.admitted_receipt()
                out_dir = self.root / name
                arms = ("sequential", "parallel-off")
                with (
                    mock.patch.object(self.mod, "load_manifest", return_value={}),
                    mock.patch.object(
                        self.mod, "repo_entries", return_value=(self.repositories[0],)
                    ),
                    mock.patch.object(
                        self.mod, "load_corpus", return_value={"repository": "alpha"}
                    ),
                    mock.patch.object(
                        self.mod, "_corpus_matches_repository", return_value=True
                    ),
                    mock.patch.object(
                        self.mod._DOCKER, "inspect_runtime", return_value=mock.Mock()
                    ),
                    mock.patch.object(
                        self.mod, "load_qualification_receipt", return_value=receipt
                    ),
                    mock.patch.object(
                        self.mod,
                        "_staged_dataset_tree",
                        side_effect=contextlib.nullcontext,
                    ),
                    mock.patch.object(
                        self.mod, "_staged_graded_inputs", side_effect=staged_inputs
                    ),
                    mock.patch.object(
                        self.mod, "run_repo_arm", side_effect=arm_failure
                    ) as run_arm,
                    mock.patch.object(
                        self.mod, "_empty_run_result", wraps=self.mod._empty_run_result
                    ) as empty_run,
                    contextlib.redirect_stdout(io.StringIO()),
                ):
                    status = self.mod.main(
                        [
                            "run",
                            "--manifest",
                            str(self.root / "manifest.json"),
                            "--cache",
                            str(self.root / "cache"),
                            "--out",
                            str(out_dir),
                            "--arms",
                            ",".join(arms),
                            "--docker-image",
                            "statefulbench-realworld:local",
                        ]
                    )

                self.assertEqual(status, 1)
                self.assertEqual(
                    run_arm.call_count, 0 if name == "pre-arm-staging" else len(arms)
                )
                self.assertEqual(empty_run.call_count, len(arms))
                summary = json.loads((out_dir / "summary.json").read_text(encoding="utf-8"))
                self.assertEqual(len(summary["results"]), len(arms))
                for row in summary["results"]:
                    self.assertEqual(row["qualification"], identity)
                    self.assertFalse(row["cleared"])
                    self.assertEqual(row["agents"], [])
                    persisted = json.loads(
                        (
                            out_dir
                            / row["repository"]
                            / row["arm"]
                            / f"trial-{row['trial']}"
                            / "results.json"
                        ).read_text(encoding="utf-8")
                    )
                    self.assertEqual(persisted["qualification"], identity)

    def test_dataset_stage_cleanup_downgrades_persisted_success_rows(self) -> None:
        receipt, identity = self.admitted_receipt()
        arms = ("sequential", "parallel-off")
        out_dir = self.root / "dataset-cleanup"
        results = [
            {
                **self.result(
                    "alpha",
                    arm,
                    1,
                    wall=float(index),
                    tokens=index,
                    tools=index + 1,
                ),
                "agents": [
                    {
                        "agent_id": f"agent-{index}",
                        "kind": "task",
                        "exit_code": 0,
                        "timed_out": False,
                    }
                ],
                "artifacts": {"run": {"stdout": f"artifacts/{arm}.log"}},
                "qualification": identity,
                "runtime": {
                    "image_id": "sha256:fixture",
                    "platform": "linux/arm64",
                    "server_platform": "linux/arm64",
                    "versions": {},
                },
            }
            for index, arm in enumerate(arms, start=1)
        ]
        for result in results:
            result.pop("coordination_metrics")
        retained = json.loads(json.dumps(results))
        writes = []
        write_result = self.mod._write_run_result
        source_manifest = self.root / "manifest.json"
        source_manifest.write_text('{"original": true}\n', encoding="utf-8")
        original_manifest = source_manifest.read_bytes()
        stage_state = {"swapped": False}


        @contextlib.contextmanager
        def stage(manifest: Path):
            yield manifest
            source_manifest.write_text('{"swapped": true}\n', encoding="utf-8")
            stage_state["swapped"] = True
            raise OSError("dataset stage cleanup failed")


        def record_write(out: Path, result: dict) -> None:
            writes.append(json.loads(json.dumps(result)))
            write_result(out, result)

        def load_manifest(_path: Path):
            if stage_state["swapped"]:
                raise AssertionError("cleanup must not reread source manifest")
            return {}

        with (
            mock.patch.object(self.mod, "load_manifest", side_effect=load_manifest),
            mock.patch.object(
                self.mod, "repo_entries", return_value=(self.repositories[0],)
            ),
            mock.patch.object(
                self.mod, "load_corpus", return_value={"repository": "alpha"}
            ),
            mock.patch.object(
                self.mod, "_corpus_matches_repository", return_value=True
            ),
            mock.patch.object(
                self.mod._DOCKER, "inspect_runtime", return_value=mock.Mock()
            ),
            mock.patch.object(
                self.mod, "load_qualification_receipt", return_value=receipt
            ),
            mock.patch.object(self.mod, "_staged_dataset_tree", side_effect=stage),
            mock.patch.object(
                self.mod,
                "_staged_graded_inputs",
                side_effect=lambda *_args: contextlib.nullcontext(self.root / "staged"),
            ),
            mock.patch.object(self.mod, "run_repo_arm", side_effect=results),
            mock.patch.object(self.mod, "_write_run_result", side_effect=record_write),
            contextlib.redirect_stdout(io.StringIO()),
            contextlib.redirect_stderr(io.StringIO()),
        ):
            status = self.mod.main(
                [
                    "run",
                    "--manifest",
                    str(self.root / "manifest.json"),
                    "--cache",
                    str(self.root / "cache"),
                    "--out",
                    str(out_dir),
                    "--arms",
                    ",".join(arms),
                    "--docker-image",
                    "statefulbench-realworld:local",
                ]
            )
        source_manifest.write_bytes(original_manifest)

        self.assertEqual(status, 1)
        summary = json.loads((out_dir / "summary.json").read_text(encoding="utf-8"))
        self.assertEqual(len(summary["results"]), len(retained))
        for original in retained:
            key = (
                original["repository"],
                original["arm"],
                original["trial"],
            )
            rewritten = [
                result
                for result in writes
                if (result["repository"], result["arm"], result["trial"]) == key
            ]
            self.assertEqual(len(rewritten), 2)
            self.assertTrue(rewritten[0]["cleared"])
            self.assertFalse(rewritten[1]["cleared"])
            self.assertIn("dataset stage cleanup failed", rewritten[1]["error"])
            self.assertEqual(rewritten[1]["agents"], original["agents"])
            self.assertEqual(rewritten[1]["artifacts"], original["artifacts"])
            self.assertEqual(rewritten[1]["total_tokens"], original["total_tokens"])
            self.assertEqual(rewritten[1]["qualification"], identity)
            self.assertIsNone(rewritten[1]["coordination_metrics"])
            summary_row = next(
                result
                for result in summary["results"]
                if (result["repository"], result["arm"], result["trial"]) == key
            )
            self.assertEqual(summary_row, rewritten[1])
        self.assertTrue(
            all(
                aggregate["comparison_error"] == "dataset stage cleanup failed"
                for aggregate in summary["aggregates"]
            )
        )
        self.assertTrue(
            all(
                "tokens" not in aggregate and "wall_time_s" not in aggregate
                for aggregate in summary["aggregates"]
            )
        )

    def test_result_write_is_atomic_and_preserves_prior_valid_json(self) -> None:
        result = self.result("alpha", "sequential", 1, tokens=1)
        self.mod._write_run_result(self.root, result)
        target = self.root / "alpha" / "sequential" / "trial-1" / "results.json"
        self.assertEqual(json.loads(target.read_text(encoding="utf-8")), result)
        original = target.read_text(encoding="utf-8")

        with mock.patch.object(self.mod.os, "replace", side_effect=OSError("replace failed")):
            with self.assertRaisesRegex(OSError, "replace failed"):
                self.mod._write_run_result(
                    self.root, self.result("alpha", "sequential", 1, tokens=2)
                )

        self.assertEqual(target.read_text(encoding="utf-8"), original)

    def test_table_normalizes_line_endings_and_escapes_pipes_in_one_physical_row(self) -> None:
        table = self.mod._table(
            [
                self.result(
                    "alpha",
                    "sequential",
                    1,
                    cleared=False,
                    error="first\r\nsecond|third",
                )
            ]
        )

        self.assertEqual(len(table.splitlines()), 3)
        self.assertEqual(
            table.splitlines()[-1],
            "| alpha | sequential | 1 | False | 0.000 | 0 | 0 | first\\nsecond\\|third |",
        )


class RealWorldDiagnosticsReportingTests(unittest.TestCase):
    def setUp(self) -> None:
        self.mod = load_script("statefulbench_realworld.py")

    @staticmethod
    def shared_evidence() -> dict:
        phases = ("initialized", "before-tasks", "after-tasks", "after-final")
        return {
            "snapshots": {
                phase: {
                    "schema_version": 1,
                    "phase": phase,
                    "home": "/home/stateful",
                    "files": [],
                    "databases": {
                        "agent.db": {
                            "integrity": "ok",
                            "schemas": [],
                            "table_counts": {},
                        }
                    },
                    "lock_files": [],
                    "per_agent_home_tree": False,
                    "processes": [],
                }
                for phase in phases
            },
            "agent_identities": {
                "task-a": {
                    "container_id": "container-1",
                    "home": "/home/stateful",
                    "profile": "/home/stateful/.omp/profiles/stateful/agent",
                },
                "final": {
                    "container_id": "container-1",
                    "home": "/home/stateful",
                    "profile": "/home/stateful/.omp/profiles/stateful/agent",
                },
            },
        }

    def test_diagnostic_paths_are_row_local_and_summary_retains_raw_evidence(self) -> None:
        result = {
            "repository": "requests",
            "arm": "parallel-on",
            "trial": 1,
            "cleared": True,
            "error": None,
            "arm_wall_time_s": 1.0,
            "tasks_wall_time_s": 0.5,
            "final_wall_time_s": 0.25,
            "total_tokens": 1,
            "total_tool_calls": 1,
            "runtime": {"image_id": "sha256:fixture", "platform": "linux/arm64", "repo_digests": [], "versions": {}},
            "container": {"id": "container-1", "setup_wall_time_s": 0.1, "teardown_wall_time_s": 0.2, "removed": True},
            "diagnostics": {
                "snapshots": {"after-final": "runtime/diagnostics/after-final.json"},
                "home_changes": [],
                "error_classification": None,
            },
        }

        self.assertEqual(
            self.mod._diagnostic_artifact_paths(result),
            {"after-final": "runtime/diagnostics/after-final.json"},
        )
        summary = self.mod.build_run_summary(
            [{"key": "requests", "commit": "a" * 40, "archive_sha256": "b" * 64}],
            ["parallel-on"],
            1,
            "model",
            "high",
            [result],
            "2026-07-13T00:00:00Z",
        )
        self.assertEqual(summary["results"][0]["diagnostics"]["snapshots"]["after-final"], "runtime/diagnostics/after-final.json")

    def test_mixed_runtime_provenance_excludes_aggregate_but_retains_rows(self) -> None:
        first = {
            "repository": "requests",
            "arm": "parallel-on",
            "trial": 1,
            "cleared": True,
            "error": None,
            "arm_wall_time_s": 1.0,
            "tasks_wall_time_s": 0.5,
            "final_wall_time_s": 0.25,
            "total_tokens": 1,
            "total_tool_calls": 1,
            "runtime": {"image_id": "sha256:first", "platform": "linux/arm64"},
        }
        second = {**first, "trial": 2, "runtime": {"image_id": "sha256:second", "platform": "linux/arm64"}}

        summary = self.mod.build_run_summary(
            [{"key": "requests", "commit": "a" * 40, "archive_sha256": "b" * 64}],
            ["parallel-on"],
            2,
            "model",
            "high",
            [first, second],
            "2026-07-13T00:00:00Z",
        )

        self.assertEqual(summary["results"], [first, second])
        self.assertEqual(
            summary["aggregates"][0]["comparison_error"],
            "mixed Docker runtime provenance",
        )

    def test_mixed_tool_provenance_excludes_aggregate(self) -> None:
        first = {
            "repository": "requests",
            "arm": "parallel-on",
            "trial": 1,
            "cleared": True,
            "error": None,
            "arm_wall_time_s": 1.0,
            "tasks_wall_time_s": 0.5,
            "final_wall_time_s": 0.25,
            "total_tokens": 1,
            "total_tool_calls": 1,
            "runtime": {
                "image_id": "sha256:fixture",
                "platform": "linux/arm64",
                "versions": {"stateful": "sha256:" + "a" * 64},
            },
        }
        second = {
            **first,
            "trial": 2,
            "runtime": {
                **first["runtime"],
                "versions": {"stateful": "sha256:" + "b" * 64},
            },
        }

        summary = self.mod.build_run_summary(
            [{"key": "requests", "commit": "a" * 40, "archive_sha256": "b" * 64}],
            ["parallel-on"],
            2,
            "model",
            "high",
            [first, second],
            "2026-07-13T00:00:00Z",
        )

        self.assertEqual(
            summary["aggregates"][0]["comparison_error"],
            "mixed Docker runtime provenance",
        )

    def test_invalid_shared_home_evidence_is_not_gradeable(self) -> None:
        evidence = {
            "snapshots": {
                phase: {"schema_version": 1, "home": "/home/stateful", "files": [], "databases": {}, "lock_files": [], "per_agent_home_tree": False, "processes": []}
                for phase in ("initialized", "before-tasks", "after-tasks", "after-final")
            },
            "agent_identities": {
                "task-a": {"container_id": "container-1", "home": "/home/stateful", "profile": "/home/stateful/.omp/profiles/stateful/agent"},
                "final": {"container_id": "other-container", "home": "/home/stateful", "profile": "/home/stateful/.omp/profiles/stateful/agent"},
            },
        }

        self.assertEqual(
            self.mod.validate_shared_home_evidence(evidence, "container-1", {"task-a", "final"}),
            "contradictory shared HOME evidence",
        )

    def test_unavailable_database_evidence_blocks_grading(self) -> None:
        evidence = self.shared_evidence()
        evidence["snapshots"]["after-final"]["databases"]["agent.db"]["integrity"] = "unavailable"

        self.assertEqual(
            self.mod.validate_shared_home_evidence(evidence, "container-1", {"task-a", "final"}),
            "sqlite_unavailable",
        )

    def test_malformed_database_evidence_blocks_grading(self) -> None:
        evidence = self.shared_evidence()
        evidence["snapshots"]["after-final"]["databases"]["agent.db"]["integrity"] = "malformed"

        self.assertEqual(
            self.mod.validate_shared_home_evidence(evidence, "container-1", {"task-a", "final"}),
            "sqlite_malformed",
        )

    def test_missing_process_evidence_is_contradictory(self) -> None:
        evidence = self.shared_evidence()
        del evidence["snapshots"]["after-final"]["processes"]

        self.assertEqual(
            self.mod.validate_shared_home_evidence(evidence, "container-1", {"task-a", "final"}),
            "contradictory shared HOME evidence",
        )
if __name__ == "__main__":
    unittest.main()

from __future__ import annotations

import hashlib
import contextlib
import importlib.util
import io
import json
import sys
import subprocess
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
                    "source_hash": f"{index:064x}",
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

    def test_load_corpus_accepts_balanced_connected_tasks(self) -> None:
        corpus = self.mod.load_corpus(self.corpus_path)

        self.assertEqual(corpus["repository"], "fixture")
        self.assertEqual(len(corpus["tasks"]), 10)

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
                    "source_hash": f"{index:064x}",
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
            "archive_url": "https://github.com/example/fixture/archive.tar.gz",
            "archive_sha256": digest,
            "python": "3.14.6",
            "setup": [sys.executable, "-c", "pass"],
            "suite": [
                sys.executable,
                "-c",
                "from pathlib import Path; assert 'base' in Path('target.py').read_text()",
            ],
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

    def _qualify(self) -> tuple[int, dict]:
        stdout = io.StringIO()
        with contextlib.redirect_stdout(stdout):
            status = self.mod.main(
                [
                    "qualify",
                    "--manifest",
                    str(self.manifest),
                    "--cache",
                    str(self.cache),
                    "--repo",
                    "fixture",
                ]
            )
        return status, json.loads(stdout.getvalue())

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
        manifest = json.loads(self.manifest.read_text(encoding="utf-8"))
        manifest["repositories"][0]["suite"] = [sys.executable, "-c", "raise SystemExit(1)"]
        self.manifest.write_text(json.dumps(manifest), encoding="utf-8")

        status, result = self._qualify()

        self.assertEqual(status, 1)
        self.assertFalse(result["repositories"][0]["upstream_green"])

    def test_qualify_rejects_base_suite_failure(self) -> None:
        manifest = json.loads(self.manifest.read_text(encoding="utf-8"))
        manifest["repositories"][0]["suite"] = [
            sys.executable,
            "-c",
            "from pathlib import Path; assert 'integrated' in Path('suite.txt').read_text()",
        ]
        self.manifest.write_text(json.dumps(manifest), encoding="utf-8")

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
        workspace_env = {"PATH": "/toolchain/bin:/usr/bin", "RUSTUP_HOME": "/host/rustup"}

        @contextlib.contextmanager
        def workspace(*_args):
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
        }

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
            mock.patch.object(self.mod, "run_repo_arm", side_effect=lambda *_args: next(results)),
            mock.patch.object(self.mod.time, "strftime", return_value="2026-07-12T00:00:00Z"),
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
if __name__ == "__main__":
    unittest.main()

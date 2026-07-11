from __future__ import annotations

import hashlib
import importlib.util
import io
import json
import sys
import tarfile
import tempfile
import unittest
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
                member = tarfile.TarInfo("source/link")
                member.type = tarfile.SYMTYPE
                member.linkname = symlink
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

    def test_extract_workspace_rejects_unsafe_members_and_multiple_roots(self) -> None:
        cases = (
            ("traversal", self.archive_bytes({"../escape": b"no"})),
            ("symlink", self.archive_bytes({"pyproject.toml": b"[project]\n"}, symlink="../escape")),
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
if __name__ == "__main__":
    unittest.main()

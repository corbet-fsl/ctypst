"""Offline failure fixtures for the irreversible publication boundary."""
import ast
import io
import json
import os
import re
import subprocess
import sys
from pathlib import Path
import tarfile
import tempfile
import textwrap
import unittest
from unittest.mock import Mock, patch

import release


class ArchiveGuards(unittest.TestCase):
    def test_modified_and_added_source_inputs_are_rejected(self):
        for change in ("modified", "added", "commit"):
            with self.subTest(change=change), tempfile.TemporaryDirectory() as temporary:
                root = Path(temporary) / "source"
                root.mkdir()
                files = {"Cargo.toml": b'[package]\nname="ctypst"\nversion="0.3.2"\n',
                         "Cargo.lock": b"lock\n", "src/lib.rs": b"original"}
                for name in ("package.json", "jsr.json"):
                    files["js/@corbet-labs/ctypst/" + name] = b'{"version":"0.3.2"}'
                release.unpack(files, root)
                archive_path = Path(temporary) / "source.tar"
                with tarfile.open(archive_path, "w") as archive:
                    for name, data in files.items():
                        member = tarfile.TarInfo(name)
                        member.size = len(data)
                        archive.addfile(member, io.BytesIO(data))
                if change == "modified":
                    (root / "src/lib.rs").write_bytes(b"modified")
                elif change == "added":
                    (root / "src/additional.rs").write_bytes(b"added")
                environment = {"CI_COMMIT_SHA": "a" * 40, "SOURCE_ARCHIVE": str(archive_path),
                               "SOURCE_SHA256": release.digest(archive_path.read_bytes()), "RELEASE_TAG": "v0.3.2"}
                with patch.dict(os.environ, environment), patch.object(release, "ROOT", root), \
                     patch.object(release.subprocess, "check_output", return_value=(("b" if change == "commit" else "a") * 40).encode()):
                    with self.assertRaises(release.Failure):
                        release.source_identity()

    def test_canonical_source_identity_ignores_tar_encoding_but_binds_modes(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            files = {"script.sh": b"#!/bin/sh\nexit 0\n"}
            release.unpack(files, root)
            archives = []
            for format in (tarfile.USTAR_FORMAT, tarfile.PAX_FORMAT):
                stream = io.BytesIO()
                with tarfile.open(fileobj=stream, mode="w", format=format,
                                  pax_headers={"comment": "a" * 40} if format == tarfile.PAX_FORMAT else None) as archive:
                    member = tarfile.TarInfo("script.sh")
                    member.mode, member.size = 0o644, len(files["script.sh"])
                    archive.addfile(member, io.BytesIO(files["script.sh"]))
                archives.append(stream.getvalue())
            self.assertNotEqual(release.digest(archives[0]), release.digest(archives[1]))
            with patch.object(release, "ROOT", root):
                self.assertEqual(release.source_content_identity(archives[0], files), release.source_content_identity(archives[1], files))
                (root / "script.sh").chmod(0o755)
                with self.assertRaisesRegex(release.Failure, "executable mode differs"):
                    release.source_content_identity(archives[0], files)

    def test_traversal_links_and_duplicates_are_refused(self):
        for kind in ("traversal", "symlink", "duplicate"):
            with self.subTest(kind=kind), tempfile.TemporaryDirectory() as temporary:
                path = Path(temporary) / "bad.tar"
                with tarfile.open(path, "w") as archive:
                    member = tarfile.TarInfo("../outside" if kind == "traversal" else "package/file")
                    if kind == "symlink":
                        member.type = tarfile.SYMTYPE
                        member.linkname = "/outside"
                        archive.addfile(member)
                    else:
                        member.size = 1
                        archive.addfile(member, io.BytesIO(b"x"))
                        if kind == "duplicate":
                            archive.addfile(member, io.BytesIO(b"y"))
                with self.assertRaises(release.Failure):
                    release.archive_files(path, "package/")


class PrepareCredentialGuards(unittest.TestCase):
    def test_actual_driver_refuses_credentials_before_any_build(self):
        with tempfile.TemporaryDirectory() as temporary:
            directory = Path(temporary)
            root = directory / "source"
            root.mkdir()
            marker = directory / "build-was-invoked"
            commit = "a" * 40
            files = {"Cargo.toml": b'[package]\nname="ctypst"\nversion="0.3.2"\n',
                     "Cargo.lock": b"fixture lock\n",
                     "scripts/release.py": (release.ROOT / "scripts/release.py").read_bytes(),
                     "scripts/ci.sh": b'#!/bin/sh\nprintf called > "$FIXTURE_BUILD_MARKER"\nexit 99\n',
                     "js/@corbet-labs/ctypst/package.json": b'{"version":"0.3.2"}',
                     "js/@corbet-labs/ctypst/jsr.json": b'{"version":"0.3.2"}'}
            release.unpack(files, root)
            archive_path = directory / "source.tar"
            with tarfile.open(archive_path, "w", format=tarfile.PAX_FORMAT, pax_headers={"comment": commit}) as archive:
                for name, payload in files.items():
                    member = tarfile.TarInfo(name)
                    member.size, member.mode = len(payload), 0o644
                    archive.addfile(member, io.BytesIO(payload))
            baseline = {key: value for key, value in os.environ.items() if key not in release.TOKENS}
            baseline.update(CI_COMMIT_SHA=commit, SOURCE_ARCHIVE=str(archive_path),
                            SOURCE_SHA256=release.digest(archive_path.read_bytes()), RELEASE_TAG="v0.3.2",
                            RELEASE_ARTIFACT_DIR=str(directory / "artifacts"), FIXTURE_BUILD_MARKER=str(marker))
            for name in release.TOKENS:
                with self.subTest(credential=name):
                    environment = {**baseline, name: "fixture-secret-that-must-not-be-printed"}
                    result = subprocess.run([sys.executable, str(root / "scripts/release.py"), "prepare", "cargo"],
                                            env=environment, stdout=subprocess.PIPE, stderr=subprocess.STDOUT,
                                            text=True, check=False)
                    self.assertNotEqual(result.returncode, 0, result.stdout)
                    self.assertIn("Publication credentials must be absent from preparation", result.stdout)
                    self.assertNotIn(environment[name], result.stdout)
                    self.assertFalse(marker.exists(), "A build started with publication credentials present")


if __name__ == "__main__":
    unittest.main()

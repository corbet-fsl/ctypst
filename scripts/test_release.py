"""Offline failure fixtures for the irreversible publication boundary."""
import ast
import io
import json
import os
import re
import subprocess
import sys
import urllib.error
from pathlib import Path
import tarfile
import tempfile
import textwrap
import unittest
from unittest.mock import Mock, patch

import release


class PublicationGuards(unittest.TestCase):
    def setUp(self):
        self.temporary = tempfile.TemporaryDirectory()
        self.addCleanup(self.temporary.cleanup)
        self.directory = Path(self.temporary.name)
        self.identity = {"version": "0.3.2", "driver_commit": "a" * 40}
        self.expected = {"ctypst-0.3.2.crate": "b" * 64}
        sleeper = patch.object(release.time, "sleep")
        sleeper.start()
        self.addCleanup(sleeper.stop)

    def publish(self, verify, upload, expected=None):
        release.mutation(self.directory, "cargo", self.identity,
                         expected or self.expected, verify, upload)

    def journal(self):
        return json.loads((self.directory / "publication/cargo.json").read_text())

    def test_existing_exact_version_is_not_uploaded(self):
        upload = Mock()
        self.publish(lambda: True, upload)
        upload.assert_not_called()
        self.assertEqual(self.journal()["status"], "verified")

    def test_intent_is_persisted_before_upload(self):
        checks = iter((False, True))
        def upload():
            self.assertEqual(self.journal()["status"], "attempted")
        self.publish(lambda: next(checks), upload)
        self.assertEqual(self.journal()["status"], "verified")

    def test_lost_response_never_causes_a_second_upload(self):
        upload = Mock(side_effect=release.Failure("lost response"))
        with self.assertRaisesRegex(release.Failure, "lost response"):
            self.publish(lambda: False, upload)
        self.assertEqual(self.journal()["status"], "attempted")
        with self.assertRaisesRegex(release.Failure, "prior publication attempt"):
            self.publish(lambda: False, upload)
        upload.assert_called_once()

    def test_delayed_registry_visibility_reconciles_without_upload(self):
        upload = Mock()
        with self.assertRaisesRegex(release.Failure, "visibility is pending"):
            self.publish(lambda: False, upload)
        self.publish(lambda: True, upload)
        upload.assert_called_once()
        self.assertEqual(self.journal()["status"], "verified")

    def test_changed_artifact_cannot_reuse_journal(self):
        upload = Mock()
        self.publish(lambda: True, upload)
        with self.assertRaisesRegex(release.Failure, "another artifact"):
            self.publish(lambda: True, upload, {"ctypst-0.3.2.crate": "c" * 64})
        upload.assert_not_called()

    def test_unknown_remote_state_does_not_upload(self):
        upload = Mock()
        with self.assertRaisesRegex(release.Failure, "unavailable"):
            self.publish(Mock(side_effect=release.Failure("unavailable")), upload)
        upload.assert_not_called()
        self.assertFalse((self.directory / "publication/cargo.json").exists())

    def test_changed_artifact_bytes_fail_receipt_validation(self):
        path = self.directory / "ctypst-0.3.2.crate"
        path.write_bytes(b"original")
        release.write_json(self.directory / "cargo.json", {**self.identity,
                           "artifacts": {path.name: release.digest(path.read_bytes())}})
        path.write_bytes(b"changed")
        with self.assertRaisesRegex(release.Failure, "Artifact changed"):
            release.load_component(self.directory, self.identity, "cargo")

    def test_receipt_cannot_cross_source_commits(self):
        release.write_json(self.directory / "cargo.json", {**self.identity, "artifacts": {}})
        with self.assertRaisesRegex(release.Failure, "another source"):
            release.load_component(self.directory, {**self.identity, "driver_commit": "b" * 40}, "cargo")

    def test_token_route_refuses_trusted_only_without_mutation(self):
        with patch.dict(os.environ, {"CARGO_REGISTRY_TOKEN": "fixture", "RELEASE_CARGO_AUTH": "token"}), \
             patch.object(release, "request", return_value={"crate": {"trustpub_only": True}}) as request:
            with self.assertRaisesRegex(release.Failure, "trusted-publishing-only"):
                release.cargo_authorization()
        request.assert_called_once_with(release.CRATES + "/crates/ctypst", token="fixture")

    def test_unknown_registry_policy_refuses_token_route(self):
        with patch.dict(os.environ, {"CARGO_REGISTRY_TOKEN": "fixture", "RELEASE_CARGO_AUTH": "token"}), \
             patch.object(release, "request", return_value={"crate": {}}):
            with self.assertRaises(release.Failure):
                release.cargo_authorization()

    def test_configured_trusted_token_does_not_change_policy(self):
        with patch.dict(os.environ, {"CARGO_REGISTRY_TOKEN": "fixture", "RELEASE_CARGO_AUTH": "trusted"}), \
             patch.object(release, "request") as request:
            self.assertEqual(release.cargo_authorization(), "fixture")
        request.assert_not_called()

    def test_remote_cargo_checksum_conflict_refuses_success(self):
        manifest = {"artifacts": {"ctypst-0.3.2.crate": release.digest(b"prepared")}}
        with patch.object(release, "request", side_effect=[{"version": {"yanked": False}}, b"different"]):
            with self.assertRaisesRegex(release.Failure, "bytes differ"):
                release.remote_cargo(self.directory, self.identity, manifest)

    def test_remote_cargo_yank_refuses_success(self):
        with patch.object(release, "request", return_value={"version": {"yanked": True}}):
            with self.assertRaisesRegex(release.Failure, "yanked"):
                release.remote_cargo(self.directory, self.identity, {})

    def test_remote_tag_mismatch_refuses_publication(self):
        with patch.object(release, "tag_commit", return_value="b" * 40):
            with self.assertRaisesRegex(release.Failure, "Remote release tag differs"):
                release.verify_tag([{"tag_commit": "a" * 40}], {"tag": "v0.3.2"}, None)

    def test_disagreeing_package_tag_receipts_refuse_publication(self):
        with self.assertRaisesRegex(release.Failure, "receipts disagree"):
            release.verify_tag([{"tag_commit": "a" * 40}, {"tag_commit": "b" * 40}], {}, None)

    def test_hosted_rerun_cannot_discard_old_journal(self):
        with patch.dict(os.environ, {"GITHUB_RUN_ID": "100", "GITHUB_RUN_ATTEMPT": "2"}), \
             patch.object(release, "request") as request:
            with self.assertRaisesRegex(release.Failure, "rerun refused"):
                release.hosted_attempt(self.identity)
        request.assert_not_called()

    def test_second_hosted_dispatch_cannot_discard_old_journal(self):
        with patch.dict(os.environ, {"GITHUB_RUN_ID": "100", "GITHUB_RUN_ATTEMPT": "1"}), \
             patch.object(release, "request", return_value={"total_count": 2, "workflow_runs": [{"id": 100}, {"id": 99}]}):
            with self.assertRaisesRegex(release.Failure, "earlier hosted release"):
                release.hosted_attempt(self.identity)

    def test_provider_transport_change_reuses_receipt_and_prior_upload_journal(self):
        old = {**self.identity, "driver_content_sha256": "c" * 64, "driver_source_sha256": "d" * 64}
        current = {**old, "driver_source_sha256": "e" * 64}
        artifact = self.directory / "ctypst-0.3.2.crate"
        artifact.write_bytes(b"verified crate")
        expected = {artifact.name: release.digest(artifact.read_bytes())}
        release.write_json(self.directory / "cargo.json", {**old, "artifacts": expected})
        self.assertEqual(release.load_component(self.directory, current, "cargo")["driver_source_sha256"], old["driver_source_sha256"])
        upload = Mock(side_effect=release.Failure("lost response"))
        with self.assertRaisesRegex(release.Failure, "lost response"):
            release.mutation(self.directory, "cargo", old, expected, lambda: False, upload)
        with self.assertRaisesRegex(release.Failure, "prior publication attempt"):
            release.mutation(self.directory, "cargo", current, expected, lambda: False, upload)
        release.mutation(self.directory, "cargo", current, expected, lambda: True, upload)
        upload.assert_called_once()
        self.assertEqual(self.journal()["status"], "verified")
        changed = {**current, "driver_content_sha256": "f" * 64}
        with self.assertRaisesRegex(release.Failure, "another source"):
            release.load_component(self.directory, changed, "cargo")
        with self.assertRaisesRegex(release.Failure, "another artifact"):
            release.mutation(self.directory, "cargo", changed, expected, lambda: True, upload)
        upload.assert_called_once()

    def test_authenticated_requests_cannot_target_arbitrary_origin(self):
        with self.assertRaisesRegex(release.Failure, "unexpected host"):
            release.request("https://example.invalid/credential", token="fixture")


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


class JavascriptRegistryGuards(unittest.TestCase):
    def setUp(self):
        self.temporary = tempfile.TemporaryDirectory()
        self.addCleanup(self.temporary.cleanup)
        self.directory = Path(self.temporary.name)
        self.identity = {"version": "0.3.2"}
        self.npm_bytes = b"prepared npm archive"
        self.manifest = {"artifacts": {"ctypst-0.3.2.tgz": release.digest(self.npm_bytes)}}
        self.files = {"jsr.json": b'{"name":"@corbet-labs/ctypst","version":"0.3.2"}',
                      "src/index.ts": b"export const value = 1;\n"}
        with tarfile.open(self.directory / "ctypst-0.3.2-jsr.tar.gz", "w:gz") as archive:
            for name, content in self.files.items():
                member = tarfile.TarInfo(name)
                member.size = len(content)
                archive.addfile(member, io.BytesIO(content))

    def npm_metadata(self, **changes):
        result = {"name": "@corbet-labs/ctypst", "version": "0.3.2",
                  "dist": {"tarball": "https://registry.npmjs.org/@corbet-labs/ctypst/-/ctypst-0.3.2.tgz"}}
        result.update(changes)
        return result

    def jsr_metadata(self):
        return {"manifest": {"/" + name: {"checksum": "sha256-" + release.digest(content)}
                             for name, content in self.files.items()}}

    def test_npm_exact_tarball_is_verified_and_changed_bytes_refused(self):
        for data, accepted in ((self.npm_bytes, True), (b"different npm archive", False)):
            with self.subTest(accepted=accepted), patch.object(release, "request", side_effect=[self.npm_metadata(), data]):
                if accepted:
                    self.assertTrue(release.remote_javascript(self.directory, self.identity, self.manifest, "npm"))
                else:
                    with self.assertRaisesRegex(release.Failure, "npm archive differs"):
                        release.remote_javascript(self.directory, self.identity, self.manifest, "npm")

    def test_npm_wrong_identity_and_download_origin_fail_before_download(self):
        for metadata in (self.npm_metadata(name="@other/ctypst"), self.npm_metadata(version="0.3.1"),
                         self.npm_metadata(dist={"tarball": "https://example.invalid/ctypst.tgz"})):
            with self.subTest(metadata=metadata), patch.object(release, "request", return_value=metadata) as request:
                with self.assertRaises(release.Failure):
                    release.remote_javascript(self.directory, self.identity, self.manifest, "npm")
                request.assert_called_once()

    def test_jsr_requires_complete_file_inventory_before_download(self):
        for difference in ("missing", "extra"):
            metadata = self.jsr_metadata()
            if difference == "missing":
                del metadata["manifest"]["/src/index.ts"]
            else:
                metadata["manifest"]["/extra.ts"] = {"checksum": "sha256-" + "0" * 64}
            with self.subTest(difference=difference), patch.object(release, "request", return_value=metadata) as request:
                with self.assertRaisesRegex(release.Failure, "JSR file inventory differs"):
                    release.remote_javascript(self.directory, self.identity, self.manifest, "jsr")
                request.assert_called_once()

    def test_jsr_downloaded_bytes_must_match_metadata_and_local_files(self):
        for changed in (False, True):
            def response(url, **kwargs):
                if url.endswith("_meta.json"):
                    return self.jsr_metadata()
                name = url.split("/0.3.2/", 1)[1]
                return b"different downloaded contents" if changed and name == "src/index.ts" else self.files[name]
            with self.subTest(changed=changed), patch.object(release, "request", side_effect=response):
                if changed:
                    with self.assertRaisesRegex(release.Failure, "JSR downloaded bytes differ"):
                        release.remote_javascript(self.directory, self.identity, self.manifest, "jsr")
                else:
                    self.assertTrue(release.remote_javascript(self.directory, self.identity, self.manifest, "jsr"))

    def test_jsr_wrong_metadata_checksum_refused_before_download(self):
        metadata = self.jsr_metadata()
        metadata["manifest"]["/jsr.json"]["checksum"] = "sha256-" + "0" * 64
        with patch.object(release, "request", return_value=metadata) as request:
            with self.assertRaisesRegex(release.Failure, "JSR file checksum differs"):
                release.remote_javascript(self.directory, self.identity, self.manifest, "jsr")
            request.assert_called_once()


class AuthenticatedDownloadGuards(unittest.TestCase):
    endpoint = release.GITHUB + "/repos/" + release.REPOSITORY + "/releases/assets/123"

    def test_api_binary_download_requests_octet_stream(self):
        opener = Mock()
        opener.open.return_value = io.BytesIO(b"verified asset")
        with patch.object(release.urllib.request, "build_opener", return_value=opener):
            self.assertEqual(release.request(self.endpoint, token="fixture-secret", binary=True), b"verified asset")
        request = opener.open.call_args.args[0]
        self.assertEqual(request.get_header("Accept"), "application/octet-stream")
        self.assertEqual(request.get_header("Authorization"), "Bearer fixture-secret")

    def test_signed_storage_redirect_does_not_receive_authorization(self):
        location = "https://release-assets.githubusercontent.com/fixture?signature=public-fixture"
        first, second = Mock(), Mock()
        first.open.side_effect = urllib.error.HTTPError(self.endpoint, 302, "Found", {"Location": location}, None)
        second.open.return_value = io.BytesIO(b"verified asset")
        with patch.object(release.urllib.request, "build_opener", side_effect=[first, second]):
            self.assertEqual(release.request(self.endpoint, token="fixture-secret", binary=True), b"verified asset")
        redirected = second.open.call_args.args[0]
        self.assertEqual(redirected.full_url, location)
        self.assertIsNone(redirected.get_header("Authorization"))
        self.assertIsNone(redirected.get_header("Accept"))

    def test_unexpected_redirect_is_refused_without_second_request(self):
        for location in ("https://example.invalid/asset", "http://release-assets.githubusercontent.com/asset"):
            opener = Mock()
            opener.open.side_effect = urllib.error.HTTPError(self.endpoint, 302, "Found", {"Location": location}, None)
            with self.subTest(location=location), patch.object(release.urllib.request, "build_opener", return_value=opener):
                with self.assertRaisesRegex(release.Failure, "Unexpected GitHub artifact download origin"):
                    release.request(self.endpoint, token="fixture-secret", binary=True)
            opener.open.assert_called_once()


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


class GithubInventoryGuards(unittest.TestCase):
    def test_incomplete_public_or_extra_draft_assets_cannot_mutate(self):
        for state in ("draft-extra", "public-missing"):
            with self.subTest(state=state), tempfile.TemporaryDirectory() as temporary:
                root = Path(temporary)
                (root / "CHANGELOG.md").write_text("## 0.3.2 (fixture)\nFixture release notes.\n")
                identity = {"version": "0.3.2", "tag": "v0.3.2", "driver_commit": "a" * 40}
                producing, source_hash, tag_commit = "b" * 40, "c" * 64, "d" * 40
                common = {**identity, "producing_commit": producing, "producing_source_sha256": source_hash,
                          "tag_commit": tag_commit}
                manifests = {"cargo": {**common, "artifacts": {"ctypst-0.3.2.crate": release.digest(b"crate")}},
                             "javascript": {**common, "artifacts": {"ctypst-0.3.2.tgz": release.digest(b"npm"),
                                                                    "ctypst-0.3.2-jsr.tar.gz": release.digest(b"jsr")}}}
                for name, manifest in manifests.items():
                    release.write_json(root / (name + ".json"), manifest)
                notes = "Fixture release notes.\n\nPackage provenance:\n" + "\n".join(
                    f"- {name}: source `{producing}`, source SHA-256 `{source_hash}`."
                    for name in ("Cargo", "JavaScript"))
                notes += f"\n- Immutable tag `v0.3.2`: `{tag_commit}`.\n"
                response = {"id": 1, "tag_name": "v0.3.2", "name": "ctypst 0.3.2", "body": notes,
                            "draft": state == "draft-extra", "assets": []}
                if state == "draft-extra":
                    response["assets"] = [{"name": "unexpected.bin"}]
                with patch.object(release, "ROOT", root), patch.dict(os.environ, {"GH_TOKEN": "fixture"}), \
                     patch.object(release, "load_component", side_effect=lambda directory, value, component: manifests[component]), \
                     patch.object(release, "verify_tag"), patch.object(release, "remote_cargo", return_value=True), \
                     patch.object(release, "remote_javascript", return_value=True), \
                     patch.object(release, "request", return_value=response) as request:
                    with self.assertRaises(release.Failure):
                        release.publish_github(root, identity)
                self.assertTrue(request.call_args_list)
                self.assertTrue(all(call.kwargs.get("method", "GET") == "GET" for call in request.call_args_list),
                                "Invalid release inventory triggered an HTTP mutation")


class CrowReleaseSelection(unittest.TestCase):
    """Exercise the actual YAML selection before Crow resolves step secrets."""

    def setUp(self):
        self.source = (Path(__file__).resolve().parents[1] / ".crow/release.yaml").read_text()
        sections = re.split(r"^  - name: ", self.source, flags=re.M)
        self.assertNotIn("from_secret:", sections[0])
        self.steps = dict(section.split("\n", 1) for section in sections[1:])
        self.assertEqual(list(self.steps), ["validate-release-selection", "prepare-packages-without-credentials",
                                          "publish-prepared-cargo", "publish-prepared-javascript", "publish-prepared-all"])
        self.assertNotIn("depends_on:", self.source, "Selected steps must retain Crow's sequential execution")

    def selected_steps(self, stage, component):
        values = {"RELEASE_STAGE": stage, "RELEASE_COMPONENT": component}

        def condition(node):
            if isinstance(node, ast.BoolOp) and isinstance(node.op, ast.And):
                return all(condition(value) for value in node.values)
            if isinstance(node, ast.Compare) and len(node.ops) == 1 and isinstance(node.left, ast.Name):
                left = values[node.left.id]
                right = ast.literal_eval(node.comparators[0])
                if isinstance(node.ops[0], ast.In):
                    return left in right
                if isinstance(node.ops[0], ast.Eq):
                    return left == right
            self.fail("Unsupported Crow selection expression; extend this fixture deliberately")

        selected = []
        for name, block in self.steps.items():
            guards = re.findall(r"^      - evaluate: '([^']+)'$", block, re.M)
            if name == "validate-release-selection":
                self.assertNotIn("when:", block)
                self.assertNotIn("from_secret:", block)
                selected.append(name)
            else:
                self.assertEqual(len(guards), 1, name + " needs one configuration-time guard")
                self.assertIn("*ccid-environment", block)
                if condition(ast.parse(guards[0].replace("&&", "and"), mode="eval").body):
                    selected.append(name)
        return selected

    def test_selected_steps_need_only_the_selected_component_secrets(self):
        credentials = {"cargo": {"ctypst_github_token", "ctypst_cargo_token"},
                       "javascript": {"ctypst_github_token", "ctypst_npm_token", "ctypst_jsr_token"},
                       "all": {"ctypst_github_token", "ctypst_cargo_token", "ctypst_npm_token", "ctypst_jsr_token"}}
        for stage in ("prepare", "publish", "all"):
            for component in credentials:
                with self.subTest(stage=stage, component=component):
                    selected = self.selected_steps(stage, component)
                    expected = ["validate-release-selection"]
                    if stage in ("prepare", "all"):
                        expected.append("prepare-packages-without-credentials")
                    if stage in ("publish", "all"):
                        expected.append("publish-prepared-" + component)
                    self.assertEqual(selected, expected)
                    required = set(re.findall(r"from_secret:\s*([A-Za-z0-9_]+)",
                                              "\n".join(self.steps[name] for name in selected)))
                    self.assertEqual(required, set() if stage == "prepare" else credentials[component])

    def test_invalid_selection_runs_only_the_failing_credential_free_validator(self):
        script = textwrap.dedent(self.steps["validate-release-selection"].split("      - |\n", 1)[1])
        for stage, component in (("invalid", "all"), ("prepare", "invalid"), ("", "cargo"), ("publish", "cargo,javascript")):
            with self.subTest(stage=stage, component=component):
                self.assertEqual(self.selected_steps(stage, component), ["validate-release-selection"])
                result = subprocess.run(["bash", "-c", script], capture_output=True, text=True,
                                        env={"PATH": os.environ.get("PATH", os.defpath),
                                             "RELEASE_STAGE": stage, "RELEASE_COMPONENT": component})
                self.assertEqual(result.returncode, 2)
                self.assertIn("must be", result.stderr)


if __name__ == "__main__":
    unittest.main()

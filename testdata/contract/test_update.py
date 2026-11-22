"""Update-notice contract tests: GET /api/update and the "Check for updates" setting. "GitHub" is the fake Tiiny's
/__release route (the server gets it as STUDIO_UPDATE_URL); the harness runs it with STUDIO_DOCKER=1, so the check
starts off, as in the Docker image. Turning the setting off and on again clears the server's one-day cache."""
import json
import re
import unittest
from pathlib import Path

from harness import ContractCase, request  # first: puts testdata/ on the path

VERSION = (Path(__file__).resolve().parents[2] / "VERSION").read_text("utf-8").strip()
NEWER = {"tag_name": "v99.0.0", "name": "99.0.0", "body": "## Big news\n\n- **Fast** <script>alert(1)</script>",
         "html_url": "https://github.com/thezolon/tiiny-studio/releases/tag/v99.0.0", "draft": False, "prerelease": False}
OFF = {"available": False}


class Update(ContractCase):
    @classmethod
    def setUpClass(cls):
        super().setUpClass()
        # before any test touches the setting: the Docker default, and no request to "GitHub" while it's off
        cls.initial = request(cls.h.rs_url, "GET", "/api/settings").json["update_check"]
        cls.initial_answers = request(cls.h.rs_url, "GET", "/api/update").json
        cls.initial_hits = cls.hits_now()

    @classmethod
    def hits_now(cls):
        return json.loads(request(cls.h.fake_url, "GET", "/__release_hits").body)

    def hits(self):
        return self.hits_now()

    def setting(self, on):
        request(self.h.rs_url, "POST", "/api/settings/updates", {"update_check": on})

    def serve(self, **flags):
        """Start from nothing remembered, with the fake answering as given, and the check on."""
        self.setting(False)
        request(self.h.fake_url, "POST", "/__reset")
        request(self.h.fake_url, "POST", "/__flags", flags)
        self.setting(True)

    def ask(self):
        r = self.get("/api/update")
        self.assertEqual(r.status, 200)
        return r.json

    def test_default_is_off_in_docker_and_asks_nobody(self):
        self.assertIs(self.initial, False)
        self.assertEqual(self.initial_answers, OFF)
        self.assertEqual(self.initial_hits, [])

    def test_newer_release_is_offered(self):
        self.serve(release=NEWER)
        self.assertEqual(self.ask(), {"available": True, "version": "99.0.0", "notes": NEWER["body"], "url": NEWER["html_url"]})
        hits = self.hits()
        self.assertEqual(len(hits), 1)  # one anonymous GET
        for h in hits:
            self.assertEqual(h["user_agent"], f"tiiny-studio/{VERSION}")
            self.assertEqual(h["accept"], "application/vnd.github+json")

    def test_numeric_compare_and_tag_forms(self):
        self.serve(release={**NEWER, "tag_name": "0.10.0"})  # 0.10.0 is newer than 0.4.0 (not a string compare)
        self.assertTrue(self.ask()["available"])
        self.assertEqual(self.ask()["version"], "0.10.0")

    def test_same_or_older_is_not_offered(self):
        for tag in (f"v{VERSION}", VERSION, "v0.0.1", "0.3.9"):
            with self.subTest(tag=tag):
                self.serve(release={**NEWER, "tag_name": tag})
                self.assertEqual(self.ask(), OFF)
                self.assertEqual(len(self.hits()), 1)  # it did ask; the answer is just "no"

    def test_prereleases_and_drafts_are_not_offered(self):
        for what, release in (("rc tag", {**NEWER, "tag_name": "v99.0.0-rc1"}), ("prerelease flag", {**NEWER, "prerelease": True}),
                              ("draft flag", {**NEWER, "draft": True}), ("beta tag", {**NEWER, "tag_name": "99.0.0-beta.2"}),
                              ("no tag", {"name": "x"}), ("not an object", "a string")):
            with self.subTest(what=what):
                self.serve(release=release)
                self.assertEqual(self.ask(), OFF)

    def test_a_failing_github_says_nothing_and_isnt_asked_again(self):
        for status in (403, 429, 500):
            with self.subTest(status=status):
                self.serve(release_fail=status)
                self.assertEqual(self.ask(), OFF)
                self.assertEqual(len(self.hits()), 1)
                # within the cache window: no new request, even though GitHub would now answer
                request(self.h.fake_url, "POST", "/__flags", {"release_fail": 0, "release": NEWER})
                self.assertEqual(self.ask(), OFF)
                self.assertEqual(self.ask(), OFF)
                self.assertEqual(len(self.hits()), 1)

    def test_an_unknown_release_route_says_nothing(self):
        self.serve()  # the fake answers 404
        self.assertEqual(self.ask(), OFF)

    def test_a_good_answer_is_cached_too(self):
        self.serve(release=NEWER)
        first = self.ask()
        for _ in range(3):
            self.assertEqual(self.ask(), first)
        self.assertEqual(len(self.hits()), 1)

    def test_setting_off_makes_no_request(self):
        self.serve(release=NEWER)
        self.assertTrue(self.ask()["available"])
        self.setting(False)
        request(self.h.fake_url, "POST", "/__reset")  # forgets the hits
        request(self.h.fake_url, "POST", "/__flags", {"release": NEWER})
        self.assertEqual(self.get("/api/settings").json["update_check"], False)
        self.assertEqual(self.ask(), OFF)
        self.assertEqual(self.hits(), [])

    def test_the_setting_shows_in_settings_and_survives_a_bad_value(self):
        self.setting(True)
        self.assertIs(self.get("/api/settings").json["update_check"], True)
        self.post("/api/settings/updates", {})  # nothing given counts as off
        self.assertIs(self.get("/api/settings").json["update_check"], False)

    def test_the_first_launch_question_is_not_asked_in_docker_and_prompt_tags_switch(self):
        s = self.get("/api/settings").json
        self.assertIs(s["update_ask"], False)  # Docker: never asked
        self.assertIs(s["tag_prompts"], True)  # on unless switched off
        try:
            self.assertEqual(self.post("/api/settings/tags", {"tag_prompts": False}).status, 200)
            self.assertIs(self.get("/api/settings").json["tag_prompts"], False)
        finally:
            self.post("/api/settings/tags", {"tag_prompts": True})
        self.assertIs(self.get("/api/settings").json["tag_prompts"], True)

    def test_the_page_has_the_notice(self):
        page = self.get("/").body.decode()
        self.assertIn('id="updatePill"', page)
        self.assertIn('id="updateModal"', page)
        js = self.get("/static/update.js")
        self.assertEqual(js.status, 200)
        self.assertTrue(re.search(rb"/api/update", js.body))


if __name__ == "__main__":
    unittest.main()

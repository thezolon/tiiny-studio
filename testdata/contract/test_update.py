"""Update-notice contract tests: GET /api/update and the "Check for updates" setting. "GitHub" is the fake Tiiny's
/__release route (both servers get it as STUDIO_UPDATE_URL); the harness runs them with STUDIO_DOCKER=1, so the check
starts off, as in the Docker image. Turning the setting off and on again clears a server's one-day cache."""
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
        cls.initial = [request(b, "GET", "/api/settings").json["update_check"] for b in (cls.h.py_url, cls.h.rs_url)]
        cls.initial_answers = [request(b, "GET", "/api/update").json for b in (cls.h.py_url, cls.h.rs_url)]
        cls.initial_hits = cls.hits_now()

    @classmethod
    def hits_now(cls):
        return json.loads(request(cls.h.fake_url, "GET", "/__release_hits").body)

    def hits(self):
        return self.hits_now()

    def servers(self):
        return (self.h.py_url, self.h.rs_url)

    def setting(self, on):
        for base in self.servers():
            request(base, "POST", "/api/settings/updates", {"update_check": on})

    def serve(self, **flags):
        """Start from nothing remembered, with the fake answering as given, and the check on."""
        self.setting(False)
        request(self.h.fake_url, "POST", "/__reset")
        request(self.h.fake_url, "POST", "/__flags", flags)
        self.setting(True)

    def ask(self):
        pair = self.get("/api/update")
        self.same(pair)
        return pair[0].json

    def test_default_is_off_in_docker_and_asks_nobody(self):
        self.assertEqual(self.initial, [False, False])
        self.assertEqual(self.initial_answers, [OFF, OFF])
        self.assertEqual(self.initial_hits, [])

    def test_newer_release_is_offered_the_same_by_both(self):
        self.serve(release=NEWER)
        self.assertEqual(self.ask(), {"available": True, "version": "99.0.0", "notes": NEWER["body"], "url": NEWER["html_url"]})
        hits = self.hits()
        self.assertEqual(len(hits), 2)  # one anonymous GET per server
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
                self.assertEqual(len(self.hits()), 2)  # it did ask; the answer is just "no"

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
                self.assertEqual(len(self.hits()), 2)
                # within the cache window: no new request, even though GitHub would now answer
                request(self.h.fake_url, "POST", "/__flags", {"release_fail": 0, "release": NEWER})
                self.assertEqual(self.ask(), OFF)
                self.assertEqual(self.ask(), OFF)
                self.assertEqual(len(self.hits()), 2)

    def test_an_unknown_release_route_says_nothing(self):
        self.serve()  # the fake answers 404
        self.assertEqual(self.ask(), OFF)

    def test_a_good_answer_is_cached_too(self):
        self.serve(release=NEWER)
        first = self.ask()
        for _ in range(3):
            self.assertEqual(self.ask(), first)
        self.assertEqual(len(self.hits()), 2)

    def test_setting_off_makes_no_request(self):
        self.serve(release=NEWER)
        self.assertTrue(self.ask()["available"])
        self.setting(False)
        request(self.h.fake_url, "POST", "/__reset")  # forgets the hits
        request(self.h.fake_url, "POST", "/__flags", {"release": NEWER})
        for base in self.servers():
            self.assertEqual(request(base, "GET", "/api/settings").json["update_check"], False)
        self.assertEqual(self.ask(), OFF)
        self.assertEqual(self.hits(), [])

    def test_the_setting_shows_in_settings_and_survives_a_bad_value(self):
        self.setting(True)
        pair = self.get("/api/settings")
        self.assertEqual([r.json["update_check"] for r in pair], [True, True])
        for base in self.servers():
            request(base, "POST", "/api/settings/updates", {})  # nothing given counts as off
        pair = self.get("/api/settings")
        self.assertEqual([r.json["update_check"] for r in pair], [False, False])

    def test_the_page_has_the_notice(self):
        for base in self.servers():
            page = request(base, "GET", "/").body.decode()
            self.assertIn('id="updatePill"', page)
            self.assertIn('id="updateModal"', page)
            js = request(base, "GET", "/static/update.js")
            self.assertEqual(js.status, 200)
            self.assertTrue(re.search(rb"/api/update", js.body))


if __name__ == "__main__":
    unittest.main()

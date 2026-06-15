"""Frontmatter parser: the flat-mapping subset kb reads."""

from __future__ import annotations

import unittest

from rhizome import contract
from rhizome.contract import ContractError


class TestSplit(unittest.TestCase):
    def test_no_fence_returns_none(self):
        self.assertIsNone(contract.split_frontmatter("# just a body\n\ntext"))
        self.assertIsNone(contract.split_frontmatter(""))

    def test_splits_block_and_body(self):
        text = "---\ndescription: x\n---\n\n# Title\nbody\n"
        block, body = contract.split_frontmatter(text)
        self.assertEqual(block, "description: x\n")
        self.assertEqual(body, "\n# Title\nbody\n")

    def test_unterminated_raises(self):
        with self.assertRaises(ContractError):
            contract.split_frontmatter("---\ndescription: x\nno closing fence\n")


class TestParse(unittest.TestCase):
    def test_flow_list(self):
        fm = contract.parse_frontmatter(
            '---\nkeywords: [a, b, "§存", "5字段"]\n---\nbody'
        )
        self.assertEqual(fm["keywords"], ["a", "b", "§存", "5字段"])

    def test_block_list(self):
        text = "---\nkeywords:\n  - one\n  - two\nkind: note\n---\n"
        fm = contract.parse_frontmatter(text)
        self.assertEqual(fm["keywords"], ["one", "two"])
        self.assertEqual(fm["kind"], "note")

    def test_quoted_and_bare_and_null(self):
        text = '---\ndescription: "has: colon"\nkind: reference\nsupersedes: ~\n---\n'
        fm = contract.parse_frontmatter(text)
        self.assertEqual(fm["description"], "has: colon")
        self.assertEqual(fm["kind"], "reference")
        self.assertIsNone(fm["supersedes"])

    def test_empty_flow_list(self):
        fm = contract.parse_frontmatter("---\nkeywords: []\n---\n")
        self.assertEqual(fm["keywords"], [])

    def test_comments_and_blanks_skipped(self):
        text = "---\n# a comment\n\ndescription: x\n---\n"
        fm = contract.parse_frontmatter(text)
        self.assertEqual(fm, {"description": "x"})

    def test_no_frontmatter_returns_none(self):
        self.assertIsNone(contract.parse_frontmatter("plain body, no fence"))

    def test_inline_comment_stripped_from_scalar(self):
        fm = contract.parse_frontmatter("---\nkind: note  # default\n---\n")
        self.assertEqual(fm["kind"], "note")

    def test_comment_only_value_is_null(self):
        # description with only a # comment → empty (the FN kb check must catch)
        fm = contract.parse_frontmatter(
            "---\ndescription:  # TODO fill me\nkeywords: [a]\n---\n"
        )
        self.assertIsNone(fm["description"])

    def test_hash_inside_quotes_preserved(self):
        fm = contract.parse_frontmatter('---\ndescription: "has # hash"\n---\n')
        self.assertEqual(fm["description"], "has # hash")

    def test_hash_without_leading_space_kept(self):
        fm = contract.parse_frontmatter("---\ncode: [repo/path#frag]\n---\n")
        self.assertEqual(fm["code"], ["repo/path#frag"])

    def test_multiline_flow_list(self):
        text = "---\nkeywords: [alpha,\n  beta,\n  gamma]\nkind: note\n---\n"
        fm = contract.parse_frontmatter(text)
        self.assertEqual(fm["keywords"], ["alpha", "beta", "gamma"])
        self.assertEqual(fm["kind"], "note")

    def test_bom_tolerated(self):
        fm = contract.parse_frontmatter(
            "\ufeff---\ndescription: x\nkeywords: [a]\n---\nbody\n"
        )
        self.assertEqual(fm["description"], "x")

    def test_block_scalar_folded(self):
        text = "---\ndescription: |\n  real text here\nkeywords: [a]\n---\n"
        fm = contract.parse_frontmatter(text)
        self.assertEqual(fm["description"], "real text here")

    def test_roundtrip_with_renderer(self):
        rendered = contract.render_frontmatter(
            description='d: with colon, "q" and \\ slash',
            keywords=["示例", "blue", "2026", "true"],
            kind="decision",
            links=["adr-003"],
            code=["my-kb/tools/kb"],
        )
        fm = contract.parse_frontmatter(rendered + "\nbody\n")
        self.assertEqual(fm["description"], 'd: with colon, "q" and \\ slash')
        self.assertEqual(fm["keywords"], ["示例", "blue", "2026", "true"])
        self.assertEqual(fm["kind"], "decision")
        self.assertEqual(fm["links"], ["adr-003"])
        self.assertEqual(fm["code"], ["my-kb/tools/kb"])


if __name__ == "__main__":
    unittest.main()

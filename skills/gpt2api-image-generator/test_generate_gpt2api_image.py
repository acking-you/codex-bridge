#!/usr/bin/env python3
"""Unit tests for the GPT2API image helper."""

from __future__ import annotations

import argparse
import importlib.util
import sys
import tempfile
import unittest
from pathlib import Path

SCRIPT_PATH = Path(__file__).with_name("generate_gpt2api_image.py")
spec = importlib.util.spec_from_file_location("generate_gpt2api_image", SCRIPT_PATH)
assert spec is not None and spec.loader is not None
tool = importlib.util.module_from_spec(spec)
sys.modules[spec.name] = tool
spec.loader.exec_module(tool)


def args(**overrides: object) -> argparse.Namespace:
    values = {
        "prompt": "画一张雨夜霓虹街道",
        "config": Path("unused"),
        "model": None,
        "size": None,
        "n": None,
        "output_dir": Path("unused"),
        "timeout_seconds": None,
        "dry_run": False,
        "image": [],
    }
    values.update(overrides)
    return argparse.Namespace(**values)


class BuildRequestTests(unittest.TestCase):
    def test_generation_uses_native_generation_endpoint(self) -> None:
        request = tool.build_request(
            args(size="1536x1024"),
            {"base_url": "https://example.test/api/gpt2api", "api_key": "secret"},
        )

        self.assertEqual(request.mode, "generation")
        self.assertEqual(request.url, "https://example.test/api/gpt2api/images/generations")
        self.assertEqual(request.payload["prompt"], "画一张雨夜霓虹街道")
        self.assertEqual(request.payload["size"], "1536x1024")

    def test_edit_uses_native_edit_endpoint_and_keeps_image_file(self) -> None:
        with tempfile.TemporaryDirectory() as temp:
            image = Path(temp) / "input.png"
            image.write_bytes(b"\x89PNG\r\n\x1a\n")

            request = tool.build_request(
                args(image=[image], size="1024x1536"),
                {"base_url": "https://example.test/api/gpt2api", "api_key": "secret"},
            )

        self.assertEqual(request.mode, "edit")
        self.assertEqual(request.url, "https://example.test/api/gpt2api/images/edits")
        self.assertEqual(request.payload["prompt"], "画一张雨夜霓虹街道")
        self.assertEqual(request.payload["size"], "1024x1536")
        self.assertEqual([part.path for part in request.image_parts], [image])


if __name__ == "__main__":
    unittest.main()

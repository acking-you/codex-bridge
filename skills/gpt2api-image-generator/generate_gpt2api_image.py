#!/usr/bin/env python3
"""Generate images through the configured GPT2API public endpoint."""

from __future__ import annotations

import argparse
import base64
import datetime as dt
import hashlib
import json
import mimetypes
import os
import re
import sys
import urllib.error
import urllib.request
import uuid
from dataclasses import dataclass
from pathlib import Path
from typing import Any


REPO_ROOT = Path(__file__).resolve().parents[2]
DEFAULT_CONFIG_PATH = REPO_ROOT / ".bridge-private" / "gpt2api-image.json"
DEFAULT_OUTPUT_DIR = REPO_ROOT / ".run" / "artifacts" / "generated"
DEFAULT_MODEL = "gpt-image-2"
DEFAULT_SIZE = "1024x1024"
DEFAULT_RESPONSE_FORMAT = "b64_json"
DEFAULT_TIMEOUT_SECONDS = 300
MAX_IMAGES = 4
SIZE_RE = re.compile(r"^[1-9][0-9]{1,4}x[1-9][0-9]{1,4}$")


@dataclass(frozen=True)
class ImagePart:
    path: Path
    file_name: str
    mime_type: str


@dataclass(frozen=True)
class ImageRequest:
    mode: str
    url: str
    api_key: str
    payload: dict[str, Any]
    timeout: int
    image_parts: list[ImagePart]


def parse_args() -> argparse.Namespace:
    config_from_env = os.environ.get("GPT2API_IMAGE_CONFIG")
    default_config_path = Path(config_from_env) if config_from_env else DEFAULT_CONFIG_PATH
    parser = argparse.ArgumentParser(description="Generate GPT2API image artifacts.")
    parser.add_argument("--prompt", required=True, help="Image prompt to send to GPT2API.")
    parser.add_argument(
        "--config",
        type=Path,
        default=default_config_path,
        help="Path to the private GPT2API image config JSON.",
    )
    parser.add_argument("--model", help=f"Image model. Default: {DEFAULT_MODEL}.")
    parser.add_argument("--size", help=f"Image size WIDTHxHEIGHT. Default: {DEFAULT_SIZE}.")
    parser.add_argument("--n", type=int, help="Number of images to generate, 1-4.")
    parser.add_argument(
        "--image",
        type=Path,
        action="append",
        default=[],
        help="Input image path for direct image edit mode. May be passed more than once.",
    )
    parser.add_argument(
        "--output-dir",
        type=Path,
        default=DEFAULT_OUTPUT_DIR,
        help="Artifact output directory. Defaults under .run/artifacts/generated/.",
    )
    parser.add_argument("--timeout-seconds", type=int, help="HTTP timeout in seconds.")
    parser.add_argument(
        "--dry-run",
        action="store_true",
        help="Validate config and print the redacted request without calling GPT2API.",
    )
    return parser.parse_args()


def load_config(path: Path) -> dict[str, Any]:
    try:
        data = json.loads(path.read_text(encoding="utf-8"))
    except FileNotFoundError as exc:
        raise SystemExit(f"GPT2API image config not found: {path}") from exc
    except json.JSONDecodeError as exc:
        raise SystemExit(f"invalid GPT2API image config JSON: {path}: {exc}") from exc
    if not isinstance(data, dict):
        raise SystemExit(f"GPT2API image config must be a JSON object: {path}")
    return data


def required_str(config: dict[str, Any], key: str) -> str:
    value = config.get(key)
    if not isinstance(value, str) or not value.strip():
        raise SystemExit(f"GPT2API image config missing non-empty `{key}`")
    return value.strip()


def optional_str(config: dict[str, Any], key: str, default: str) -> str:
    value = config.get(key, default)
    if not isinstance(value, str) or not value.strip():
        raise SystemExit(f"GPT2API image config `{key}` must be a non-empty string")
    return value.strip()


def optional_int(config: dict[str, Any], key: str, default: int) -> int:
    value = config.get(key, default)
    if not isinstance(value, int):
        raise SystemExit(f"GPT2API image config `{key}` must be an integer")
    return value


def clamp_image_count(value: int) -> int:
    if value < 1:
        raise SystemExit("image count must be at least 1")
    return min(value, MAX_IMAGES)


def validate_size(size: str) -> str:
    if not SIZE_RE.match(size):
        raise SystemExit(f"invalid image size `{size}`; expected WIDTHxHEIGHT")
    width_text, height_text = size.split("x", 1)
    width = int(width_text)
    height = int(height_text)
    if width < 256 or height < 256 or width > 4096 or height > 4096:
        raise SystemExit("image size must be between 256 and 4096 pixels per side")
    return size


def endpoint_url(base_url: str, endpoint_path: str) -> str:
    return f"{base_url.rstrip('/')}/{endpoint_path.lstrip('/')}"


def build_request(args: argparse.Namespace, config: dict[str, Any]) -> ImageRequest:
    prompt = args.prompt.strip()
    if not prompt:
        raise SystemExit("prompt must not be empty")

    base_url = required_str(config, "base_url")
    api_key = required_str(config, "api_key")
    image_paths = [Path(path) for path in getattr(args, "image", [])]
    mode = "edit" if image_paths else "generation"
    endpoint_path = (
        optional_str(config, "edit_endpoint_path", "/images/edits")
        if mode == "edit"
        else optional_str(config, "endpoint_path", "/images/generations")
    )
    model = args.model or optional_str(config, "model", DEFAULT_MODEL)
    size = validate_size(args.size or optional_str(config, "size", DEFAULT_SIZE))
    n = clamp_image_count(args.n if args.n is not None else optional_int(config, "n", 1))
    response_format = optional_str(config, "response_format", DEFAULT_RESPONSE_FORMAT)
    timeout = args.timeout_seconds or optional_int(
        config,
        "timeout_seconds",
        DEFAULT_TIMEOUT_SECONDS,
    )
    if timeout < 1:
        raise SystemExit("timeout must be positive")

    image_parts = [image_part(path) for path in image_paths]
    payload = {
        "prompt": prompt,
        "model": model,
        "n": n,
        "size": size,
        "response_format": response_format,
    }
    return ImageRequest(
        mode=mode,
        url=endpoint_url(base_url, endpoint_path),
        api_key=api_key,
        payload=payload,
        timeout=timeout,
        image_parts=image_parts,
    )


def post_json(url: str, api_key: str, payload: dict[str, Any], timeout: int) -> dict[str, Any]:
    request = urllib.request.Request(
        url=url,
        data=json.dumps(payload).encode("utf-8"),
        headers={
            "Authorization": f"Bearer {api_key}",
            "Content-Type": "application/json",
        },
        method="POST",
    )
    try:
        with urllib.request.urlopen(request, timeout=timeout) as response:
            return json.loads(response.read().decode("utf-8"))
    except urllib.error.HTTPError as exc:
        body = exc.read().decode("utf-8", errors="replace")
        raise SystemExit(f"GPT2API image request failed: {exc.code} {body}") from exc
    except urllib.error.URLError as exc:
        raise SystemExit(f"failed to reach GPT2API image endpoint: {exc}") from exc
    except json.JSONDecodeError as exc:
        raise SystemExit(f"GPT2API returned invalid JSON: {exc}") from exc


def image_part(path: Path) -> ImagePart:
    if not path.is_file():
        raise SystemExit(f"input image not found: {path}")
    mime_type = mimetypes.guess_type(path.name)[0] or "application/octet-stream"
    if not mime_type.startswith("image/"):
        raise SystemExit(f"input file does not look like an image: {path}")
    return ImagePart(path=path, file_name=path.name, mime_type=mime_type)


def multipart_body(payload: dict[str, Any], image_parts: list[ImagePart]) -> tuple[bytes, str]:
    boundary = f"----gpt2api-{uuid.uuid4().hex}"
    chunks: list[bytes] = []

    for key, value in payload.items():
        chunks.extend(
            [
                f"--{boundary}\r\n".encode("utf-8"),
                f'Content-Disposition: form-data; name="{key}"\r\n\r\n'.encode("utf-8"),
                str(value).encode("utf-8"),
                b"\r\n",
            ]
        )
    for part in image_parts:
        chunks.extend(
            [
                f"--{boundary}\r\n".encode("utf-8"),
                (
                    f'Content-Disposition: form-data; name="image"; '
                    f'filename="{part.file_name}"\r\n'
                ).encode("utf-8"),
                f"Content-Type: {part.mime_type}\r\n\r\n".encode("utf-8"),
                part.path.read_bytes(),
                b"\r\n",
            ]
        )
    chunks.append(f"--{boundary}--\r\n".encode("utf-8"))
    return b"".join(chunks), boundary


def post_multipart(request_data: ImageRequest) -> dict[str, Any]:
    body, boundary = multipart_body(request_data.payload, request_data.image_parts)
    request = urllib.request.Request(
        url=request_data.url,
        data=body,
        headers={
            "Authorization": f"Bearer {request_data.api_key}",
            "Content-Type": f"multipart/form-data; boundary={boundary}",
        },
        method="POST",
    )
    try:
        with urllib.request.urlopen(request, timeout=request_data.timeout) as response:
            return json.loads(response.read().decode("utf-8"))
    except urllib.error.HTTPError as exc:
        body = exc.read().decode("utf-8", errors="replace")
        raise SystemExit(f"GPT2API image edit request failed: {exc.code} {body}") from exc
    except urllib.error.URLError as exc:
        raise SystemExit(f"failed to reach GPT2API image edit endpoint: {exc}") from exc
    except json.JSONDecodeError as exc:
        raise SystemExit(f"GPT2API returned invalid JSON: {exc}") from exc


def post_image_request(request_data: ImageRequest) -> dict[str, Any]:
    if request_data.mode == "edit":
        return post_multipart(request_data)
    return post_json(
        request_data.url,
        request_data.api_key,
        request_data.payload,
        request_data.timeout,
    )


def download_url(url: str, api_key: str, timeout: int) -> bytes:
    request = urllib.request.Request(
        url=url,
        headers={"Authorization": f"Bearer {api_key}"},
        method="GET",
    )
    try:
        with urllib.request.urlopen(request, timeout=timeout) as response:
            return response.read()
    except urllib.error.HTTPError as exc:
        body = exc.read().decode("utf-8", errors="replace")
        raise SystemExit(f"failed to download GPT2API image URL: {exc.code} {body}") from exc
    except urllib.error.URLError as exc:
        raise SystemExit(f"failed to download GPT2API image URL: {exc}") from exc


def decode_base64_image(value: str) -> bytes:
    value = value.strip()
    if "," in value and value.lower().startswith("data:"):
        value = value.split(",", 1)[1]
    try:
        return base64.b64decode(value, validate=True)
    except ValueError as exc:
        raise SystemExit("GPT2API returned invalid base64 image data") from exc


def image_bytes_from_item(item: dict[str, Any], api_key: str, timeout: int) -> bytes:
    for key in ("b64_json", "image_base64", "base64", "b64"):
        value = item.get(key)
        if isinstance(value, str) and value.strip():
            return decode_base64_image(value)
    value = item.get("url")
    if isinstance(value, str) and value.strip():
        return download_url(value.strip(), api_key, timeout)
    raise SystemExit("GPT2API image response item has neither base64 data nor url")


def image_extension(image: bytes) -> str:
    if image.startswith(b"\x89PNG\r\n\x1a\n"):
        return "png"
    if image.startswith(b"\xff\xd8\xff"):
        return "jpg"
    if image.startswith(b"RIFF") and image[8:12] == b"WEBP":
        return "webp"
    if image.startswith(b"GIF87a") or image.startswith(b"GIF89a"):
        return "gif"
    return "png"


def artifact_path(output_dir: Path, prompt: str, index: int, image: bytes) -> Path:
    timestamp = dt.datetime.now(dt.timezone.utc).strftime("%Y%m%dT%H%M%SZ")
    digest = hashlib.sha256(prompt.encode("utf-8") + image[:4096]).hexdigest()[:10]
    return output_dir / f"gpt2api_{timestamp}_{index}_{digest}.{image_extension(image)}"


def relative_to_repo(path: Path) -> str:
    try:
        return str(path.relative_to(REPO_ROOT))
    except ValueError:
        return str(path)


def save_images(
    response: dict[str, Any],
    api_key: str,
    timeout: int,
    output_dir: Path,
    prompt: str,
) -> list[Path]:
    data = response.get("data")
    if not isinstance(data, list) or not data:
        raise SystemExit("GPT2API image response missing non-empty `data` array")

    output_dir.mkdir(parents=True, exist_ok=True)
    paths = []
    for index, item in enumerate(data, start=1):
        if not isinstance(item, dict):
            raise SystemExit("GPT2API image response item must be an object")
        image = image_bytes_from_item(item, api_key, timeout)
        path = artifact_path(output_dir, prompt, index, image)
        path.write_bytes(image)
        paths.append(path)
    return paths


def redacted_request(request: ImageRequest) -> dict[str, Any]:
    return {
        "mode": request.mode,
        "url": request.url,
        "headers": {
            "Authorization": "Bearer <redacted>",
            "Content-Type": (
                "multipart/form-data"
                if request.mode == "edit"
                else "application/json"
            ),
        },
        "payload": request.payload,
        "images": [str(part.path) for part in request.image_parts],
        "timeout_seconds": request.timeout,
    }


def main() -> int:
    args = parse_args()
    config = load_config(args.config)
    request = build_request(args, config)
    if args.dry_run:
        print(json.dumps(redacted_request(request), ensure_ascii=False, indent=2))
        return 0

    response = post_image_request(request)
    paths = save_images(
        response,
        request.api_key,
        request.timeout,
        args.output_dir,
        request.payload["prompt"],
    )
    result = {
        "kind": "image",
        "path": relative_to_repo(paths[0]),
        "paths": [relative_to_repo(path) for path in paths],
        "mode": request.mode,
        "model": request.payload["model"],
        "size": request.payload["size"],
        "count": len(paths),
    }
    revised_prompts = [
        item.get("revised_prompt")
        for item in response.get("data", [])
        if isinstance(item, dict) and item.get("revised_prompt")
    ]
    if revised_prompts:
        result["revised_prompts"] = revised_prompts
    print(json.dumps(result, ensure_ascii=False, indent=2))
    return 0


if __name__ == "__main__":
    sys.exit(main())

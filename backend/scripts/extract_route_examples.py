#!/usr/bin/env python3
"""OpenAPIのPOST /api/route 200応答例を、Rustテスト用のJSON fixtureへ書き出す。

正本はdocs/api/openapi.yaml。fixtureは手で編集せず、このスクリプトで再生成する。
--check は書き込まずに差分の有無だけを検査する（差分ありで終了コード1）。
前提：Python 3.9以上、PyYAML。
"""

import argparse
import json
import sys
from pathlib import Path

import yaml

ROOT = Path(__file__).resolve().parents[2]
SOURCE = ROOT / "docs" / "api" / "openapi.yaml"
DEST = ROOT / "backend" / "tests" / "fixtures" / "route-responses.json"


def build() -> str:
    document = yaml.safe_load(SOURCE.read_text(encoding="utf-8"))
    examples = document["paths"]["/api/route"]["post"]["responses"]["200"]["content"][
        "application/json"
    ]["examples"]
    data = {
        "description": (
            "docs/api/openapi.yaml のPOST /api/route 200応答例から生成した期待値。"
            "入力は docs/api/fixtures/route-example-inputs.json の同名の例。"
            "手で編集せず backend/scripts/extract_route_examples.py で再生成する。"
        ),
        "examples": {name: example["value"] for name, example in examples.items()},
    }
    return json.dumps(data, ensure_ascii=False, indent=2) + "\n"


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--check", action="store_true", help="書き込まずに差分を検査する")
    args = parser.parse_args()

    expected = build()
    if args.check:
        current = DEST.read_text(encoding="utf-8") if DEST.exists() else ""
        if current != expected:
            print(f"{DEST.relative_to(ROOT)} がopenapi.yamlと一致しません。再生成してください。")
            return 1
        print(f"{DEST.relative_to(ROOT)} はopenapi.yamlと一致しています。")
        return 0

    DEST.parent.mkdir(parents=True, exist_ok=True)
    DEST.write_text(expected, encoding="utf-8")
    print(f"{DEST.relative_to(ROOT)} を書き出しました。")
    return 0


if __name__ == "__main__":
    sys.exit(main())

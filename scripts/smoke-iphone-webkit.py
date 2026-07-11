#!/usr/bin/env python3
"""Run a small WebKit smoke check with Playwright's iPhone 13 profile."""

from __future__ import annotations

import argparse
import json
import sys
from pathlib import Path


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser()
    parser.add_argument("--url", required=True)
    parser.add_argument("--screenshot", default="")
    parser.add_argument("--expect", action="append", default=[])
    parser.add_argument("--expect-selector", action="append", default=[])
    parser.add_argument("--timeout-ms", type=int, default=30_000)
    return parser.parse_args()


def main() -> int:
    args = parse_args()
    try:
        from playwright.sync_api import Error as PlaywrightError
        from playwright.sync_api import sync_playwright
    except Exception as err:
        print(
            "Playwright is not available. Run scripts/ensure-playwright-webkit.sh first.",
            file=sys.stderr,
        )
        print(str(err), file=sys.stderr)
        return 2

    screenshot = Path(args.screenshot) if args.screenshot else None
    if screenshot:
        screenshot.parent.mkdir(parents=True, exist_ok=True)

    try:
        with sync_playwright() as playwright:
            device = playwright.devices["iPhone 13"]
            browser = playwright.webkit.launch()
            context = browser.new_context(**device)
            page = context.new_page()
            page.goto(args.url, wait_until="networkidle", timeout=args.timeout_ms)

            for selector in args.expect_selector:
                page.locator(selector).first.wait_for(
                    state="visible", timeout=args.timeout_ms
                )

            body_text = page.locator("body").inner_text(timeout=args.timeout_ms)
            for expected in args.expect:
                if expected not in body_text:
                    raise AssertionError(f"Expected text not found: {expected!r}")

            metrics = page.evaluate(
                """() => {
                  const list = document.querySelector("[data-agent-messages]");
                  const composer = document.querySelector(".agent-composer");
                  return {
                    title: document.title,
                    bodyTextLength: document.body.innerText.length,
                    messageEntries: document.querySelectorAll("[data-message-entry]").length,
                    hasMessageList: !!list,
                    hasComposer: !!composer,
                    viewport: { width: window.innerWidth, height: window.innerHeight },
                    scrollHeight: document.documentElement.scrollHeight
                  };
                }"""
            )

            if screenshot:
                page.screenshot(path=str(screenshot), full_page=True)
                metrics["screenshot"] = str(screenshot)

            context.close()
            browser.close()
    except (AssertionError, PlaywrightError) as err:
        print(f"iPhone WebKit smoke failed: {err}", file=sys.stderr)
        return 1

    print(json.dumps(metrics, indent=2, sort_keys=True))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())

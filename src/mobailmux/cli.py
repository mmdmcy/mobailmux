from __future__ import annotations

import sys


def main() -> None:
    from .app import main as mattermost_main

    mattermost_main()


if __name__ == "__main__":
    main()

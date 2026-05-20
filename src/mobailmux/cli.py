from __future__ import annotations

import sys


def main() -> None:
    args = sys.argv[1:]
    if args and args[0] in {"web", "browser", "server"}:
        from .web import main as web_main

        sys.argv = [sys.argv[0], *args[1:]]
        web_main()
        return

    from .app import main as mattermost_main

    mattermost_main()


if __name__ == "__main__":
    main()

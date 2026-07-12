from __future__ import annotations

import sys
from pathlib import Path


def main() -> None:
    checkout = Path(sys.argv[1]).resolve()
    sys.path.insert(0, str(checkout / "src"))

    import pendulum

    permissive = pendulum.from_format("2024-2-3", "YYYY-MM-DD", tz="UTC")
    assert permissive.isoformat() == "2024-02-03T00:00:00+00:00"

    strict = pendulum.from_format(
        "2024 mars 03 04:05:06.123456 Europe/Paris",
        "YYYY MMMM DD HH:mm:ss.SSSSSS z",
        locale="fr",
        strict=True,
    )
    assert strict.isoformat() == "2024-03-03T04:05:06.123456+01:00"

    for text in ("2024-2-03", "2024-02-3"):
        try:
            pendulum.from_format(text, "YYYY-MM-DD", strict=True)
        except ValueError:
            pass
        else:
            raise AssertionError(f"strict from_format accepted {text!r}")


if __name__ == "__main__":
    main()

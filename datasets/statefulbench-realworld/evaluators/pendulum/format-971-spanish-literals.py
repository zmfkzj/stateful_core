from __future__ import annotations

import sys
from pathlib import Path


def main() -> None:
    checkout = Path(sys.argv[1]).resolve()
    sys.path.insert(0, str(checkout / "src"))

    import pendulum
    def rejects(text: str, fmt: str) -> None:
        try:
            pendulum.from_format(text, fmt, tz="UTC")
        except ValueError:
            return

        raise AssertionError(f"from_format accepted {text!r} for {fmt!r}")


    parsed = pendulum.from_format(
        "21 de noviembre del 2023 04:05:06.123456 Europe/Paris",
        "DD [de] MMMM [del] YYYY HH:mm:ss.SSSSSS z",
        locale="es",
    )
    assert parsed.isoformat() == "2023-11-21T04:05:06.123456+01:00"

    repeated = pendulum.from_format(
        "21 de noviembre de 2023",
        "DD [de] MMMM [de] YYYY",
        locale="es",
        tz="Pacific/Auckland",
    )
    assert repeated.isoformat() == "2023-11-21T00:00:00+13:00"

    metacharacters = pendulum.from_format(
        "2024.+03",
        "YYYY[.+]MM",
        tz="UTC",
    )
    assert metacharacters.isoformat() == "2024-03-01T00:00:00+00:00"
    rejects("2024x03", "YYYY[.+]MM")

    backslash = pendulum.from_format(r"2024\03", r"YYYY[\]MM", tz="UTC")
    assert backslash.isoformat() == "2024-03-01T00:00:00+00:00"
    rejects("2024x03", r"YYYY[\]MM")


if __name__ == "__main__":
    main()

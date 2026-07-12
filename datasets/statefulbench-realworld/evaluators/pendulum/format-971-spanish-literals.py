from __future__ import annotations

import sys
from pathlib import Path


def main() -> None:
    checkout = Path(sys.argv[1]).resolve()
    sys.path.insert(0, str(checkout / "src"))

    import pendulum

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


if __name__ == "__main__":
    main()

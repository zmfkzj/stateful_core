#!/usr/bin/env python3
"""Evaluator for Requests issue #7040."""
import argparse
import sys
from pathlib import Path


def connection(*, ssl_context=None):
    values = {"cert_reqs": None, "ca_certs": None, "ca_cert_dir": None}
    if ssl_context is not None:
        values["ssl_context"] = ssl_context
    return type("Connection", (), values)()


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument("repo", type=Path)
    args = parser.parse_args()
    sys.path.insert(0, str(args.repo / "src"))
    from requests.adapters import HTTPAdapter

    adapter = HTTPAdapter()
    custom = connection(ssl_context=object())
    adapter.cert_verify(custom, "https://example.test", True, None)
    assert custom.cert_reqs == "CERT_REQUIRED"
    assert custom.ca_certs is None
    assert custom.ca_cert_dir is None

    default = connection()
    adapter.cert_verify(default, "https://example.test", True, None)
    assert default.cert_reqs == "CERT_REQUIRED"
    assert default.ca_certs is not None

    disabled = connection(ssl_context=object())
    adapter.cert_verify(disabled, "https://example.test", False, None)
    assert disabled.cert_reqs == "CERT_NONE"
    assert disabled.ca_certs is None
    assert disabled.ca_cert_dir is None

    try:
        adapter.cert_verify(custom, "https://example.test", "/definitely/missing/ca.pem", None)
    except OSError as error:
        assert "TLS CA certificate bundle" in str(error)
    else:
        raise AssertionError("an explicit missing CA bundle must still fail")


if __name__ == "__main__":
    main()

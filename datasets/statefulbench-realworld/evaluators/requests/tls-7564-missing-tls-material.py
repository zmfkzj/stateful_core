#!/usr/bin/env python3
"""Evaluator for Requests issue #7564."""
import argparse
import errno
import sys
from pathlib import Path


def connection():
    return type(
        "Connection", (), {"cert_reqs": None, "ca_certs": None, "ca_cert_dir": None}
    )()


def expect_missing(adapter, cert, expected_path, expected_message):
    try:
        adapter.cert_verify(connection(), "https://example.test", False, cert)
    except FileNotFoundError as error:
        assert error.errno == errno.ENOENT
        assert error.filename == expected_path
        assert expected_message in error.strerror
    else:
        raise AssertionError("missing TLS material must raise FileNotFoundError")


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument("repo", type=Path)
    args = parser.parse_args()
    sys.path.insert(0, str(args.repo / "src"))
    from requests.adapters import HTTPAdapter

    adapter = HTTPAdapter()
    missing_certificate = "/definitely/missing/client.pem"
    expect_missing(adapter, (missing_certificate, None), missing_certificate, "TLS certificate file")
    legacy_certificate = "/definitely/missing/legacy.pem"
    try:
        adapter.cert_verify(connection(), "https://example.test", False, legacy_certificate)
    except OSError as error:
        assert type(error) is OSError
        assert str(error) == (
            "Could not find the TLS certificate file, "
            f"invalid path: {legacy_certificate}"
        )
    else:
        raise AssertionError("legacy certificate paths must retain their OSError")
    legacy_key = "/definitely/missing/legacy.key"
    try:
        adapter.cert_verify(connection(), "https://example.test", False, (".", legacy_key))
    except OSError as error:
        assert type(error) is OSError
        assert str(error) == f"Could not find the TLS key file, invalid path: {legacy_key}"
    else:
        raise AssertionError("legacy directory certificate tuples must retain their OSError")



    certificate = str(args.repo / "tests/certs/valid/server/server.pem")
    missing_key = "/definitely/missing/client.key"
    expect_missing(adapter, (certificate, missing_key), missing_key, "TLS key file")
    conn = connection()
    adapter.cert_verify(conn, "https://example.test", False, (certificate, None))
    assert conn.cert_file == certificate
    assert conn.key_file is None

    no_certificate = connection()
    adapter.cert_verify(no_certificate, "https://example.test", False, None)
    assert no_certificate.cert_reqs == "CERT_NONE"
    assert not hasattr(no_certificate, "cert_file")


if __name__ == "__main__":
    main()

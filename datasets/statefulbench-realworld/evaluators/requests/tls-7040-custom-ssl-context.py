#!/usr/bin/env python3
"""Evaluator for Requests issue #7040."""
import argparse
import ssl
import sys
from pathlib import Path


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument("repo", type=Path)
    args = parser.parse_args()
    sys.path.insert(0, str(args.repo / "src"))
    from requests.adapters import HTTPAdapter
    from urllib3.connectionpool import HTTPSConnectionPool

    custom_context = ssl.create_default_context()

    class ContextAdapter(HTTPAdapter):
        def init_poolmanager(self, connections, maxsize, block=False, **pool_kwargs):
            pool_kwargs["ssl_context"] = custom_context
            return super().init_poolmanager(connections, maxsize, block, **pool_kwargs)

    adapter = ContextAdapter()
    custom = adapter.poolmanager.connection_from_url("https://example.test")
    assert isinstance(custom, HTTPSConnectionPool)
    assert custom.conn_kw["ssl_context"] is custom_context
    adapter.cert_verify(custom, "https://example.test", True, None)
    assert custom.cert_reqs == "CERT_REQUIRED"
    assert getattr(custom, "ca_certs", None) is None
    assert getattr(custom, "ca_cert_dir", None) is None

    default = HTTPAdapter().poolmanager.connection_from_url("https://example.test")
    adapter.cert_verify(default, "https://example.test", True, None)
    assert default.cert_reqs == "CERT_REQUIRED"
    assert default.ca_certs is not None

    adapter.cert_verify(custom, "https://example.test", False, None)
    assert custom.cert_reqs == "CERT_NONE"
    assert custom.ca_certs is None
    assert custom.ca_cert_dir is None

    try:
        adapter.cert_verify(
            custom, "https://example.test", "/definitely/missing/ca.pem", None
        )
    except OSError as error:
        assert "TLS CA certificate bundle" in str(error)
    else:
        raise AssertionError("an explicit missing CA bundle must still fail")


if __name__ == "__main__":
    main()

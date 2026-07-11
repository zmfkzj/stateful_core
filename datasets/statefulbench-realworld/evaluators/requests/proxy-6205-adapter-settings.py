#!/usr/bin/env python3
"""Evaluator for requests issue #6205."""
import argparse
import sys
from pathlib import Path
from unittest.mock import patch


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument("repo", type=Path)
    args = parser.parse_args()
    dependency_dir = args.repo / ".deps"
    if dependency_dir.is_dir():
        sys.path.insert(0, str(dependency_dir))
    sys.path.insert(0, str(args.repo / "src"))

    import requests
    from requests.adapters import HTTPAdapter

    class Connection:
        def __init__(self) -> None:
            self.host_args = None
            self.urlopen_args = None

        def urlopen(self, **kwargs):
            self.urlopen_args = kwargs
            return object()

    class Manager:
        def __init__(self) -> None:
            self.connection = Connection()

        def connection_from_host(self, **kwargs):
            self.connection.host_args = kwargs
            return self.connection

    adapter = HTTPAdapter()
    adapter.poolmanager.connection_pool_kw["ssl_version"] = "TLSv1.2"
    created = []

    def proxy_factory(*args, **kwargs):
        manager = Manager()
        created.append((kwargs, manager))
        return manager

    request = requests.Request("GET", "https://example.test/").prepare()
    with (
        patch("requests.adapters.proxy_from_url", side_effect=proxy_factory),
        patch.object(adapter, "build_response", return_value=object()),
    ):
        adapter.send(
            request,
            proxies={"https": "http://proxy.example:8080"},
            verify=False,
        )

    proxy_kwargs, manager = created[0]
    assert proxy_kwargs["ssl_version"] == "TLSv1.2"
    assert len(created) == 1
    assert manager.connection.host_args["host"] == "example.test"
    assert manager.connection.urlopen_args["url"] == "/"


if __name__ == "__main__":
    main()

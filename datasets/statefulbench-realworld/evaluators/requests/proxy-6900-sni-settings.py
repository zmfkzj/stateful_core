#!/usr/bin/env python3
"""Evaluator for requests issue #6900."""
import argparse
import sys
import tempfile
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
            self.pool_kwargs = None

        def connection_from_host(self, **kwargs):
            self.connection.host_args = kwargs
            self.pool_kwargs = kwargs["pool_kwargs"]
            return self.connection

    adapter = HTTPAdapter()
    adapter.poolmanager.connection_pool_kw.update(
        assert_hostname="service.internal",
        server_hostname="service.internal",
    )
    created = []

    def proxy_factory(*args, **kwargs):
        manager = Manager()
        created.append((kwargs, manager))
        return manager

    request = requests.Request("GET", "https://192.0.2.1/check").prepare()
    with tempfile.NamedTemporaryFile() as ca_file:
        with (
            patch("requests.adapters.proxy_from_url", side_effect=proxy_factory),
            patch.object(adapter, "build_response", return_value=object()),
        ):
            adapter.send(
                request,
                proxies={"https": "http://proxy.example:8080"},
                verify=ca_file.name,
            )

        proxy_kwargs, manager = created[0]
        assert proxy_kwargs["assert_hostname"] == "service.internal"
        assert proxy_kwargs["server_hostname"] == "service.internal"
        assert manager.connection.host_args["host"] == "192.0.2.1"
        assert manager.pool_kwargs["ca_certs"] == ca_file.name
        assert manager.connection.ca_certs == ca_file.name


if __name__ == "__main__":
    main()

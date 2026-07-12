#!/usr/bin/env python3
"""Evaluator for Authlib issue #741 OIDC max_age authorization requests."""

import argparse
import sys
import types
from pathlib import Path
from types import SimpleNamespace


def install_joserfc_stub() -> None:
    try:
        import joserfc  # noqa: F401
    except ModuleNotFoundError:
        package = types.ModuleType("joserfc")
        errors = types.ModuleType("joserfc.errors")
        errors.InvalidClaimError = type("InvalidClaimError", (Exception,), {})
        errors.MissingClaimError = type("MissingClaimError", (Exception,), {})
        jwk = types.ModuleType("joserfc.jwk")
        jwk.KeySet = type("KeySet", (), {})
        jwk.import_key = lambda value: value
        jwt = types.ModuleType("joserfc.jwt")
        jwt.BaseClaimsRegistry = type("BaseClaimsRegistry", (), {})
        jwt.Claims = dict
        jwt.JWTClaimsRegistry = type("JWTClaimsRegistry", (), {})
        jws = types.ModuleType("joserfc.jws")
        jws.JWSRegistry = type("JWSRegistry", (), {"recommended": []})
        registry = types.ModuleType("joserfc.registry")
        registry.Header = dict
        package.jwt = jwt
        sys.modules.update(
            {
                "joserfc": package,
                "joserfc.errors": errors,
                "joserfc.jwk": jwk,
                "joserfc.jwt": jwt,
                "joserfc.jws": jws,
                "joserfc.registry": registry,
            }
        )


def validate(params):
    install_joserfc_stub()
    from authlib.oauth2.rfc6749.requests import BasicOAuth2Payload
    from authlib.oidc.core.grants.code import OpenIDCode

    class Extension(OpenIDCode):
        def exists_nonce(self, nonce, request):
            return False

    request = SimpleNamespace(payload=BasicOAuth2Payload(params))
    grant = SimpleNamespace(request=request)
    Extension().validate_openid_authorization_request(grant, "https://client.test/cb")
    return request


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument("repo", type=Path)
    args = parser.parse_args()
    sys.path.insert(0, str(args.repo))

    from authlib.oauth2.rfc6749 import InvalidRequestError

    # A valid OIDC maximum authentication age is parsed once for application hooks.
    request = validate({"max_age": "300"})
    assert request.max_age == 300

    # Boundary: zero is valid and must not be treated as an absent parameter.
    assert validate({"max_age": "0"}).max_age == 0

    # Trust boundary: malformed and negative values are rejected before consent.
    for value in ("", "-1", "1.5", "+1", "three", "٣٠٠", 300):
        try:
            validate({"max_age": value})
        except InvalidRequestError as error:
            assert error.description == "Invalid 'max_age' parameter."
        else:
            raise AssertionError(f"max_age={value!r} must be rejected")


if __name__ == "__main__":
    main()

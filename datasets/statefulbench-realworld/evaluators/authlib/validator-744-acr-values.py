#!/usr/bin/env python3
"""Evaluator for Authlib issue #744 OIDC acr_values authorization requests."""

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


def validate(params, nonce_exists=False):
    install_joserfc_stub()
    from authlib.oauth2.rfc6749.requests import BasicOAuth2Payload
    from authlib.oidc.core.grants.code import OpenIDCode

    class Extension(OpenIDCode):
        def exists_nonce(self, nonce, request):
            return nonce_exists

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

    # Preference order is preserved while the request is normalized for application hooks.
    request = validate({"acr_values": "urn:example:strong urn:example:basic"})
    assert request.acr_values == ("urn:example:strong", "urn:example:basic")

    # Boundary: standard whitespace separates values without manufacturing empty values.
    assert validate({"acr_values": "  gold\tbronze  "}).acr_values == ("gold", "bronze")

    # Trust boundary: an explicitly empty request is not an ACR preference list.
    try:
        validate({"acr_values": ""})
    except InvalidRequestError as error:
        assert error.description == "Invalid 'acr_values' parameter."
    else:
        raise AssertionError("an empty acr_values parameter must be rejected")

    # Trust boundary: acr_values is a wire string, not an arbitrary payload value.
    for value in (0, [], {}):
        try:
            validate({"acr_values": value})
        except InvalidRequestError as error:
            assert error.description == "Invalid 'acr_values' parameter."
        else:
            raise AssertionError(f"acr_values={value!r} must be rejected")

    # Existing nonces are rejected before ACR preference application logic can run.
    try:
        validate({"nonce": "already-used"}, nonce_exists=True)
    except InvalidRequestError:
        pass
    else:
        raise AssertionError("a replayed nonce must be rejected")


if __name__ == "__main__":
    main()

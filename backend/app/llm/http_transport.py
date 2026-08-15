"""HTTP transport policy shared by compatible model endpoints."""
from __future__ import annotations

from ipaddress import ip_address
from urllib.parse import urlparse


def trust_environment_proxy(url: str) -> bool:
    """Keep configured proxies for remote services, but never proxy loopback model endpoints."""
    hostname = (urlparse(url).hostname or "").rstrip(".").lower()
    if hostname == "localhost" or hostname.endswith(".localhost"):
        return False
    try:
        return not ip_address(hostname).is_loopback
    except ValueError:
        return True

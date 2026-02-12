"""
retry_utils.py - typed retry decorator helper for nx.
"""

from __future__ import annotations

from collections.abc import Callable
from typing import Any, ParamSpec, TypeVar, cast

from tenacity import retry

P = ParamSpec("P")
R = TypeVar("R")


def typed_retry(*args: Any, **kwargs: Any) -> Callable[[Callable[P, R]], Callable[P, R]]:
    """Typed wrapper to avoid untyped decorator issues with tenacity retry."""
    return cast(Callable[[Callable[P, R]], Callable[P, R]], retry(*args, **kwargs))

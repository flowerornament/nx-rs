"""
cache.py - JSON cache for nx package search results

Caches package search results, keyed to source revision from flake.lock.
"""

from __future__ import annotations

import json
from pathlib import Path

from shared import NAME_MAPPINGS
from sources import SourceResult

CACHE_SCHEMA_VERSION = 1


class MultiSourceCache:
    """JSON cache for package lookups, keyed to source revision."""

    def __init__(self, repo_root: Path):
        self.cache_dir = Path.home() / ".cache" / "nx"
        self.cache_dir.mkdir(parents=True, exist_ok=True)
        self.cache_path = self.cache_dir / "packages_v4.json"
        self.repo_root = repo_root
        self._revisions: dict[str, str] = {}
        self._data: dict[str, dict] = {}
        self._load_revisions()
        self._load_cache()

    def _load_revisions(self) -> None:
        """Load source revisions from flake.lock."""
        lock_path = self.repo_root / "flake.lock"
        if not lock_path.exists():
            return

        try:
            with open(lock_path) as f:
                lock = json.load(f)

            nodes = lock.get("nodes", {})

            # Extract nixpkgs revision (stored internally as "nxs" for consistency)
            nixpkgs = nodes.get("nixpkgs", {})
            locked = nixpkgs.get("locked", {})
            self._revisions["nxs"] = locked.get("rev", "unknown")[:12]

            # Extract other input revisions
            for name, data in nodes.items():
                if name == "root":
                    continue
                if "locked" in data:
                    self._revisions[name] = data["locked"].get("rev", "unknown")[:12]

        except Exception:
            self._revisions["nxs"] = "unknown"

    def _load_cache(self) -> None:
        """Load cache from JSON file."""
        if self.cache_path.exists():
            try:
                with open(self.cache_path) as f:
                    raw_data = json.load(f)
                    self._data = self._extract_entries(raw_data)
            except (OSError, json.JSONDecodeError):
                self._data = {}
        else:
            self._data = {}

    def _extract_entries(self, raw_data: object) -> dict[str, dict]:
        if not isinstance(raw_data, dict):
            return {}

        schema_version = raw_data.get("schema_version")
        if schema_version != CACHE_SCHEMA_VERSION:
            return {}

        entries = raw_data.get("entries")
        if not isinstance(entries, dict):
            return {}

        extracted: dict[str, dict] = {}
        for key, value in entries.items():
            if isinstance(key, str) and isinstance(value, dict):
                extracted[key] = value
        return extracted

    def _save_cache(self) -> None:
        """Save cache to JSON file."""
        with open(self.cache_path, "w") as f:
            json.dump(
                {
                    "schema_version": CACHE_SCHEMA_VERSION,
                    "entries": self._data,
                },
                f,
                indent=2,
            )

    def _normalize_name(self, name: str) -> str:
        mapped = NAME_MAPPINGS.get(name.lower(), NAME_MAPPINGS.get(name, name))
        return mapped.lower()

    def _cache_key(self, name: str, source: str) -> str:
        """Generate cache key: name|source|revision."""
        rev = self.get_revision(source)
        normalized_name = self._normalize_name(name)
        return f"{normalized_name}|{source}|{rev}"

    def get_revision(self, source: str) -> str:
        """Get revision for a source (for cache key)."""
        source_to_input = {
            "nxs": "nxs",
            "nur": "nur",
            "homebrew": "homebrew",
            "cask": "homebrew",
            "mas": "mas",
        }
        input_name = source_to_input.get(source, source)
        return self._revisions.get(input_name, "unknown")

    def get(self, name: str, source: str) -> SourceResult | None:
        """Get cached result for a name+source combination."""
        key = self._cache_key(name, source)
        entry = self._data.get(key)
        if entry:
            description = entry.get("description", "")
            if not isinstance(description, str):
                description = ""
            return SourceResult(
                name=name,
                source=source,
                attr=entry.get("attr"),
                version=entry.get("version"),
                description=description,
                confidence=entry.get("confidence", 0.0),
                requires_flake_mod=entry.get("requires_flake_mod", False),
                flake_url=entry.get("flake_url"),
            )
        return None

    def get_all(self, name: str) -> list[SourceResult]:
        """Get all cached results for a package name.

        Returns results sorted by source priority (nxs first).
        If only homebrew/cask results exist (no nxs), returns empty
        to force a fresh search - homebrew should only be used as fallback.
        """
        results = []
        has_nxs = False
        for source in ["nxs", "nur", "homebrew", "cask"]:
            result = self.get(name, source)
            if result:
                results.append(result)
                if source in ("nxs", "nur"):
                    has_nxs = True

        # Don't return stale homebrew-only results; force fresh search
        # This ensures nxs is always tried first
        if results and not has_nxs:
            return []

        return results

    def set(self, result: SourceResult) -> None:
        """Cache a search result."""
        if not result.attr:
            return

        key = self._cache_key(result.name, result.source)
        self._data[key] = {
            "attr": result.attr,
            "version": result.version,
            "description": result.description,
            "confidence": result.confidence,
            "requires_flake_mod": result.requires_flake_mod,
            "flake_url": result.flake_url,
        }
        self._save_cache()

    def set_many(self, results: list[SourceResult]) -> None:
        """Cache multiple search results (best per source only)."""
        # Group by (name, source) and keep only the highest confidence result
        best_by_source: dict[tuple[str, str], SourceResult] = {}
        for result in results:
            key = (result.name, result.source)
            if key not in best_by_source or result.confidence > best_by_source[key].confidence:
                best_by_source[key] = result

        # Batch update without saving each time
        for result in best_by_source.values():
            if not result.attr:
                continue
            cache_key = self._cache_key(result.name, result.source)
            self._data[cache_key] = {
                "attr": result.attr,
                "version": result.version,
                "description": result.description,
                "confidence": result.confidence,
                "requires_flake_mod": result.requires_flake_mod,
                "flake_url": result.flake_url,
            }

        self._save_cache()

    def invalidate(self, name: str, source: str | None = None) -> None:
        """Invalidate cache entries for a package."""
        normalized_name = self._normalize_name(name)
        keys_to_delete = []
        for key in self._data:
            parts = key.split("|")
            if len(parts) >= 2:
                cached_name, cached_source = parts[0], parts[1]
                if cached_name == normalized_name:
                    if source is None or cached_source == source:
                        keys_to_delete.append(key)

        for key in keys_to_delete:
            del self._data[key]

        if keys_to_delete:
            self._save_cache()

    def clear(self) -> None:
        """Clear entire cache."""
        self._data = {}
        self._save_cache()

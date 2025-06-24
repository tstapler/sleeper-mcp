"""Caching implementation for Sleeper API responses.

This module provides a comprehensive caching solution using diskcache and HTTP caching
strategies. It includes:
- Disk-based cache storage
- HTTP cache control directive handling
- Conditional request support (ETags, Last-Modified)
- Cache invalidation strategies
- Cache statistics and monitoring
"""

import hashlib
import json
import time
from datetime import datetime, timedelta
from email.utils import formatdate, parsedate_to_datetime
from pathlib import Path
from typing import Any, Dict, Optional, Tuple, Union
from urllib.parse import urlparse

from dateutil.parser import parse as parse_date
from diskcache import Cache
from httpx import Request, Response
from pydantic import BaseModel
from structlog import get_logger

logger = get_logger(__name__)

# Cache configuration
CACHE_DIR = Path(__file__).parent.parent.parent / "cache"
DEFAULT_CACHE_TTL = 3600  # 1 hour
MAX_CACHE_SIZE = 1024 * 1024 * 1024  # 1GB


class CacheMetadata(BaseModel):
    """Metadata for cached responses."""

    url: str
    method: str
    etag: Optional[str] = None
    last_modified: Optional[datetime] = None
    cache_control: Dict[str, Any] = {}
    expires: Optional[datetime] = None
    date: Optional[datetime] = None
    vary_headers: Dict[str, str] = {}
    created_at: datetime
    accessed_at: datetime
    access_count: int = 0


class CacheEntry(BaseModel):
    """Complete cache entry including response data and metadata."""

    status_code: int
    headers: Dict[str, str]
    content: bytes
    metadata: CacheMetadata


class APICache:
    """Comprehensive caching implementation for API responses."""

    def __init__(
        self,
        cache_dir: Optional[Path] = None,
        ttl: int = DEFAULT_CACHE_TTL,
        max_size: int = MAX_CACHE_SIZE
    ):
        """Initialize the cache.
        
        Args:
            cache_dir: Directory to store cache files
            ttl: Default time-to-live for cache entries in seconds
            max_size: Maximum cache size in bytes
        """
        self.cache_dir = cache_dir or CACHE_DIR
        self.ttl = ttl
        self.max_size = max_size
        
        # Create cache directory if it doesn't exist
        self.cache_dir.mkdir(parents=True, exist_ok=True)
        
        # Initialize disk cache
        self.cache = Cache(
            directory=str(self.cache_dir),
            size_limit=max_size,
            eviction_policy='least-recently-used'
        )
        
        logger.info(
            "cache_initialized",
            directory=str(self.cache_dir),
            ttl=ttl,
            max_size=max_size
        )

    def _generate_cache_key(self, request: Request) -> str:
        """Generate a unique cache key for a request.
        
        Args:
            request: HTTP request
            
        Returns:
            str: Cache key
        """
        # Include relevant request information in key
        key_parts = [
            request.method,
            str(request.url),
            request.content.decode() if request.content else "",
            str(sorted(request.headers.items()))
        ]
        
        # Create stable hash
        key = hashlib.sha256("".join(key_parts).encode()).hexdigest()
        logger.debug("cache_key_generated", key=key, url=str(request.url))
        return key

    def _parse_cache_control(self, headers: Dict[str, str]) -> Dict[str, Any]:
        """Parse Cache-Control header into a dictionary.
        
        Args:
            headers: Response headers
            
        Returns:
            Dict[str, Any]: Parsed cache control directives
        """
        cache_control = {}
        
        if "Cache-Control" not in headers:
            return cache_control
            
        directives = headers["Cache-Control"].split(",")
        for directive in directives:
            directive = directive.strip().lower()
            if "=" in directive:
                key, value = directive.split("=", 1)
                try:
                    cache_control[key] = int(value)
                except ValueError:
                    cache_control[key] = value
            else:
                cache_control[directive] = True
                
        return cache_control

    def _should_cache_response(
        self,
        response: Response,
        cache_control: Dict[str, Any]
    ) -> bool:
        """Determine if a response should be cached.
        
        Args:
            response: HTTP response
            cache_control: Parsed cache control directives
            
        Returns:
            bool: Whether the response should be cached
        """
        # Never cache errors
        if response.status_code >= 400:
            return False
            
        # Check cache control directives
        if cache_control.get("no-store", False):
            return False
            
        if cache_control.get("private", False):
            return False
            
        # Cache successful GET and HEAD requests by default
        return response.request.method in ("GET", "HEAD")

    def _get_ttl(
        self,
        response: Response,
        cache_control: Dict[str, Any]
    ) -> int:
        """Calculate time-to-live for a response.
        
        Args:
            response: HTTP response
            cache_control: Parsed cache control directives
            
        Returns:
            int: TTL in seconds
        """
        # Check for max-age directive
        if "max-age" in cache_control:
            return cache_control["max-age"]
            
        # Check for Expires header
        if "Expires" in response.headers:
            try:
                expires = parse_date(response.headers["Expires"])
                if expires > datetime.utcnow():
                    return int((expires - datetime.utcnow()).total_seconds())
            except (ValueError, TypeError):
                pass
                
        return self.ttl

    def _create_cache_entry(
        self,
        request: Request,
        response: Response
    ) -> CacheEntry:
        """Create a cache entry from a response.
        
        Args:
            request: HTTP request
            response: HTTP response
            
        Returns:
            CacheEntry: Cache entry
        """
        cache_control = self._parse_cache_control(response.headers)
        
        # Parse date headers
        date = None
        if "Date" in response.headers:
            try:
                date = parse_date(response.headers["Date"])
            except ValueError:
                pass
                
        last_modified = None
        if "Last-Modified" in response.headers:
            try:
                last_modified = parse_date(response.headers["Last-Modified"])
            except ValueError:
                pass
                
        expires = None
        if "Expires" in response.headers:
            try:
                expires = parse_date(response.headers["Expires"])
            except ValueError:
                pass
        
        # Create metadata
        metadata = CacheMetadata(
            url=str(request.url),
            method=request.method,
            etag=response.headers.get("ETag"),
            last_modified=last_modified,
            cache_control=cache_control,
            expires=expires,
            date=date,
            vary_headers={
                k: v for k, v in request.headers.items()
                if k in response.headers.get("Vary", "").split(",")
            },
            created_at=datetime.utcnow(),
            accessed_at=datetime.utcnow(),
            access_count=1
        )
        
        return CacheEntry(
            status_code=response.status_code,
            headers=dict(response.headers),
            content=response.content,
            metadata=metadata
        )

    def _update_cache_entry(self, entry: CacheEntry) -> CacheEntry:
        """Update cache entry metadata on access.
        
        Args:
            entry: Cache entry to update
            
        Returns:
            CacheEntry: Updated cache entry
        """
        entry.metadata.accessed_at = datetime.utcnow()
        entry.metadata.access_count += 1
        return entry

    def _is_entry_fresh(
        self,
        entry: CacheEntry,
        request: Request
    ) -> Tuple[bool, Optional[Dict[str, str]]]:
        """Check if a cache entry is still fresh.
        
        Args:
            entry: Cache entry to check
            request: Current request
            
        Returns:
            Tuple[bool, Optional[Dict[str, str]]]: (is_fresh, conditional_headers)
        """
        now = datetime.utcnow()
        metadata = entry.metadata
        
        # Check cache control max-age
        if "max-age" in metadata.cache_control:
            age = (now - metadata.created_at).total_seconds()
            if age > metadata.cache_control["max-age"]:
                # Prepare conditional request headers
                headers = {}
                if metadata.etag:
                    headers["If-None-Match"] = metadata.etag
                if metadata.last_modified:
                    headers["If-Modified-Since"] = formatdate(
                        metadata.last_modified.timestamp(),
                        usegmt=True
                    )
                return False, headers
                
        # Check expires header
        if metadata.expires and now > metadata.expires:
            return False, None
            
        # Check vary headers
        for header, value in metadata.vary_headers.items():
            if request.headers.get(header) != value:
                return False, None
                
        return True, None

    async def get(
        self,
        request: Request
    ) -> Optional[Tuple[Response, bool]]:
        """Get a cached response for a request.
        
        Args:
            request: HTTP request
            
        Returns:
            Optional[Tuple[Response, bool]]: (response, is_fresh) if cached,
            None if not cached
        """
        key = self._generate_cache_key(request)
        entry = self.cache.get(key)
        
        if not entry:
            return None
            
        # Convert from JSON if needed
        if isinstance(entry, str):
            entry = CacheEntry.model_validate_json(entry)
            
        # Check freshness and get conditional headers
        is_fresh, conditional_headers = self._is_entry_fresh(entry, request)
        
        if not is_fresh and conditional_headers:
            # Return cached response with conditional headers
            response = Response(
                status_code=entry.status_code,
                headers=entry.headers | conditional_headers,
                content=entry.content,
                request=request
            )
            return response, False
            
        if not is_fresh:
            # Remove stale entry
            self.cache.delete(key)
            return None
            
        # Update access metadata
        entry = self._update_cache_entry(entry)
        self.cache.set(key, entry.model_dump_json())
        
        # Return fresh cached response
        response = Response(
            status_code=entry.status_code,
            headers=entry.headers,
            content=entry.content,
            request=request
        )
        return response, True

    def set(
        self,
        request: Request,
        response: Response
    ) -> None:
        """Cache a response.
        
        Args:
            request: HTTP request
            response: HTTP response to cache
        """
        cache_control = self._parse_cache_control(response.headers)
        
        if not self._should_cache_response(response, cache_control):
            return
            
        # Create and store cache entry
        key = self._generate_cache_key(request)
        entry = self._create_cache_entry(request, response)
        ttl = self._get_ttl(response, cache_control)
        
        self.cache.set(key, entry.model_dump_json(), expire=ttl)
        
        logger.debug(
            "response_cached",
            url=str(request.url),
            ttl=ttl,
            size=len(response.content)
        )

    def delete(self, request: Request) -> None:
        """Remove a cached response.
        
        Args:
            request: HTTP request whose response should be removed
        """
        key = self._generate_cache_key(request)
        self.cache.delete(key)
        
        logger.debug("cache_entry_deleted", url=str(request.url))

    def clear(self) -> None:
        """Clear all cached responses."""
        self.cache.clear()
        logger.info("cache_cleared")

    def get_stats(self) -> Dict[str, Any]:
        """Get cache statistics.
        
        Returns:
            Dict[str, Any]: Cache statistics
        """
        stats = {
            "size": self.cache.size,
            "max_size": self.max_size,
            "directory": str(self.cache_dir),
            "entry_count": len(self.cache),
            "hit_count": self.cache.stats(enable=True)["hits"],
            "miss_count": self.cache.stats(enable=True)["misses"]
        }
        
        if stats["hit_count"] + stats["miss_count"] > 0:
            stats["hit_ratio"] = (
                stats["hit_count"] /
                (stats["hit_count"] + stats["miss_count"])
            )
        else:
            stats["hit_ratio"] = 0.0
            
        return stats

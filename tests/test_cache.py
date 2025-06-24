"""Tests for API caching implementation."""

import json
from datetime import datetime, timedelta
from pathlib import Path
from typing import Dict, Any

import pytest
from freezegun import freeze_time
from httpx import Request, Response
import respx

from src.services.cache import APICache, CacheEntry, CacheMetadata

# Test data
TEST_URL = "https://api.sleeper.app/v1/user/testuser"
TEST_HEADERS = {
    "Content-Type": "application/json",
    "ETag": "abc123",
    "Last-Modified": "Wed, 21 Oct 2023 07:28:00 GMT",
    "Cache-Control": "max-age=3600",
}
TEST_CONTENT = {"username": "testuser", "id": "123"}


@pytest.fixture
def temp_cache_dir(tmp_path):
    """Create a temporary cache directory."""
    cache_dir = tmp_path / "cache"
    cache_dir.mkdir()
    return cache_dir


@pytest.fixture
def api_cache(temp_cache_dir):
    """Create an API cache instance."""
    return APICache(cache_dir=temp_cache_dir)


def create_test_request():
    """Create a test request."""
    return Request("GET", TEST_URL)


def create_test_response(request, status_code=200):
    """Create a test response."""
    return Response(
        status_code=status_code,
        headers=TEST_HEADERS,
        content=json.dumps(TEST_CONTENT).encode(),
        request=request
    )


def test_cache_initialization(temp_cache_dir):
    """Test cache initialization."""
    cache = APICache(cache_dir=temp_cache_dir, ttl=1800, max_size=1024*1024)
    
    assert cache.ttl == 1800
    assert cache.max_size == 1024*1024
    assert cache.cache_dir == temp_cache_dir
    assert temp_cache_dir.exists()


def test_cache_key_generation(api_cache):
    """Test cache key generation."""
    request1 = Request("GET", TEST_URL)
    request2 = Request("GET", TEST_URL, headers={"Accept": "application/json"})
    request3 = Request("POST", TEST_URL, json={"data": "test"})
    
    # Same requests should generate same keys
    assert api_cache._generate_cache_key(request1) == api_cache._generate_cache_key(request1)
    
    # Different requests should generate different keys
    assert api_cache._generate_cache_key(request1) != api_cache._generate_cache_key(request2)
    assert api_cache._generate_cache_key(request1) != api_cache._generate_cache_key(request3)


def test_cache_control_parsing(api_cache):
    """Test parsing of Cache-Control headers."""
    headers = {
        "Cache-Control": "max-age=3600, public, must-revalidate"
    }
    
    parsed = api_cache._parse_cache_control(headers)
    assert parsed["max-age"] == 3600
    assert parsed["public"] is True
    assert parsed["must-revalidate"] is True


@pytest.mark.asyncio
async def test_basic_caching(api_cache):
    """Test basic caching functionality."""
    request = create_test_request()
    response = create_test_response(request)
    
    # Initially not cached
    assert await api_cache.get(request) is None
    
    # Cache the response
    api_cache.set(request, response)
    
    # Should now be cached
    cached_response, is_fresh = await api_cache.get(request)
    assert cached_response is not None
    assert is_fresh is True
    assert cached_response.status_code == 200
    assert cached_response.content == response.content


@pytest.mark.asyncio
async def test_cache_freshness(api_cache):
    """Test cache freshness checking."""
    request = create_test_request()
    response = create_test_response(request)
    
    with freeze_time("2023-10-21 07:00:00"):
        api_cache.set(request, response)
        
        # Should be fresh
        cached_response, is_fresh = await api_cache.get(request)
        assert is_fresh is True
    
    with freeze_time("2023-10-21 08:30:00"):  # 90 minutes later
        # Should be stale (max-age=3600 in headers)
        cached_response, is_fresh = await api_cache.get(request)
        assert is_fresh is False
        assert "If-None-Match" in cached_response.headers


@pytest.mark.asyncio
async def test_conditional_requests(api_cache):
    """Test handling of conditional requests."""
    request = create_test_request()
    response = create_test_response(request)
    
    # Cache initial response
    api_cache.set(request, response)
    
    # Simulate a conditional request
    cached_response, is_fresh = await api_cache.get(request)
    assert "ETag" in cached_response.headers
    
    # Simulate a 304 Not Modified response
    not_modified_response = Response(
        status_code=304,
        headers={"ETag": "abc123"},
        content=b"",
        request=request
    )
    
    # Should use cached content
    assert cached_response.content == response.content


def test_cache_invalidation(api_cache):
    """Test cache invalidation."""
    request = create_test_request()
    response = create_test_response(request)
    
    # Cache and verify
    api_cache.set(request, response)
    api_cache.delete(request)
    
    assert api_cache.cache.get(api_cache._generate_cache_key(request)) is None


def test_cache_statistics(api_cache):
    """Test cache statistics."""
    request = create_test_request()
    response = create_test_response(request)
    
    # Initial stats
    stats = api_cache.get_stats()
    initial_hits = stats["hit_count"]
    initial_misses = stats["miss_count"]
    
    # Cache miss
    api_cache.cache.get("nonexistent")
    stats = api_cache.get_stats()
    assert stats["miss_count"] == initial_misses + 1
    
    # Cache hit
    api_cache.set(request, response)
    api_cache.cache.get(api_cache._generate_cache_key(request))
    stats = api_cache.get_stats()
    assert stats["hit_count"] == initial_hits + 1


@pytest.mark.asyncio
async def test_cache_size_limit(temp_cache_dir):
    """Test cache size limiting."""
    small_cache = APICache(
        cache_dir=temp_cache_dir,
        max_size=1024  # 1KB
    )
    
    # Create large response
    large_content = b"x" * 2048  # 2KB
    request = create_test_request()
    response = Response(
        status_code=200,
        headers=TEST_HEADERS,
        content=large_content,
        request=request
    )
    
    # Should not cache (too large)
    small_cache.set(request, response)
    cached = await small_cache.get(request)
    assert cached is None


@pytest.mark.asyncio
async def test_vary_header_handling(api_cache):
    """Test handling of Vary headers."""
    base_request = create_test_request()
    vary_headers = base_request.headers.copy()
    vary_headers["Accept"] = "application/json"
    vary_request = Request("GET", TEST_URL, headers=vary_headers)
    
    response = Response(
        status_code=200,
        headers={"Vary": "Accept"} | TEST_HEADERS,
        content=json.dumps(TEST_CONTENT).encode(),
        request=vary_request
    )
    
    # Cache the response
    api_cache.set(vary_request, response)
    
    # Different Accept header should miss
    different_headers = base_request.headers.copy()
    different_headers["Accept"] = "text/html"
    different_request = Request("GET", TEST_URL, headers=different_headers)
    
    assert await api_cache.get(different_request) is None


@pytest.mark.asyncio
async def test_error_response_handling(api_cache):
    """Test handling of error responses."""
    request = create_test_request()
    error_response = create_test_response(request, status_code=500)
    
    # Should not cache errors
    api_cache.set(request, error_response)
    assert await api_cache.get(request) is None


@pytest.mark.asyncio
async def test_cache_clear(api_cache):
    """Test clearing the cache."""
    request = create_test_request()
    response = create_test_response(request)
    
    # Cache something
    api_cache.set(request, response)
    assert await api_cache.get(request) is not None
    
    # Clear cache
    api_cache.clear()
    assert await api_cache.get(request) is None
    
    # Verify stats reset
    stats = api_cache.get_stats()
    assert stats["entry_count"] == 0

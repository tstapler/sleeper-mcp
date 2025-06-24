"""Sleeper API client service with enhanced caching.

This module provides a clean interface to interact with the Sleeper API,
handling rate limiting, caching, and error handling.
"""

import asyncio
from datetime import datetime, timedelta
from typing import Any, Dict, List, Optional, Union

import httpx
from fastapi import HTTPException
from structlog import get_logger

from ..config import Config, get_config
from ..models import League, NFLState, Player, Roster, User
from .cache import APICache

logger = get_logger(__name__)


class SleeperAPIClient:
    """Client for interacting with the Sleeper API with comprehensive caching."""

    def __init__(self, config: Optional[Config] = None):
        """Initialize the Sleeper API client.
        
        Args:
            config: Optional configuration override
        """
        self.config = config or get_config()
        self.base_url = str(self.config.sleeper_api.base_url)
        
        # Initialize HTTP client with custom transport for caching
        self._client = httpx.AsyncClient(
            timeout=self.config.sleeper_api.timeout_seconds,
            base_url=self.base_url,
        )
        
        # Setup caching
        self.cache = APICache(
            ttl=self.config.cache.ttl_seconds,
            max_size=self.config.cache.max_size
        )
        
        # Setup rate limiting
        self._request_times: List[datetime] = []
        self._rate_limit = self.config.sleeper_api.rate_limit_per_minute

    async def _check_rate_limit(self) -> None:
        """Check and enforce rate limiting.
        
        Raises:
            HTTPException: If rate limit would be exceeded
        """
        now = datetime.now()
        minute_ago = now - timedelta(minutes=1)
        
        # Remove old requests from tracking
        self._request_times = [t for t in self._request_times if t > minute_ago]
        
        if len(self._request_times) >= self._rate_limit:
            raise HTTPException(
                status_code=429,
                detail="Rate limit exceeded. Please try again later."
            )
        
        self._request_times.append(now)

    async def _make_request(
        self,
        method: str,
        path: str,
        **kwargs: Any,
    ) -> Any:
        """Make a cached, rate-limited request to the Sleeper API.
        
        Args:
            method: HTTP method to use
            path: API path to request
            **kwargs: Additional arguments to pass to httpx
            
        Returns:
            Any: Parsed JSON response
            
        Raises:
            HTTPException: If the request fails
        """
        # Prepare request
        request = self._client.build_request(method, path, **kwargs)
        
        # Check cache first
        cache_result = await self.cache.get(request)
        if cache_result:
            response, is_fresh = cache_result
            if is_fresh:
                logger.debug("cache_hit", path=path)
                return response.json()
            elif "If-None-Match" in response.headers or "If-Modified-Since" in response.headers:
                # Use cached response for conditional request
                kwargs["headers"] = kwargs.get("headers", {}) | {
                    k: v for k, v in response.headers.items()
                    if k in ("If-None-Match", "If-Modified-Since")
                }

        # Check rate limit before making request
        await self._check_rate_limit()
        
        try:
            response = await self._client.send(request)
            response.raise_for_status()
            
            # Handle 304 Not Modified
            if response.status_code == 304 and cache_result:
                logger.debug("cache_revalidated", path=path)
                cached_response, _ = cache_result
                return cached_response.json()
            
            # Cache successful responses
            if response.status_code < 400:
                self.cache.set(request, response)
            
            return response.json()
            
        except httpx.HTTPStatusError as e:
            logger.error("request_failed", path=path, status_code=e.response.status_code)
            raise HTTPException(
                status_code=e.response.status_code,
                detail=str(e),
            )
        except httpx.RequestError as e:
            logger.error("request_error", path=path, error=str(e))
            raise HTTPException(
                status_code=500,
                detail="Failed to communicate with Sleeper API",
            )

    async def get_user(self, identifier: str) -> User:
        """Get a user by username or user_id.
        
        Args:
            identifier: Username or user_id to look up
            
        Returns:
            User: User information
        """
        data = await self._make_request("GET", f"/user/{identifier}")
        return User.model_validate(data)

    async def get_user_leagues(
        self,
        user_id: str,
        season: str,
        sport: str = "nfl",
    ) -> List[League]:
        """Get all leagues for a user.
        
        Args:
            user_id: User ID to look up leagues for
            season: Season year (e.g., "2023")
            sport: Sport type (default: "nfl")
            
        Returns:
            List[League]: List of leagues
        """
        data = await self._make_request(
            "GET",
            f"/user/{user_id}/leagues/{sport}/{season}",
        )
        return [League.model_validate(league) for league in data]

    async def get_league(self, league_id: str) -> League:
        """Get information about a specific league.
        
        Args:
            league_id: League ID to look up
            
        Returns:
            League: League information
        """
        data = await self._make_request("GET", f"/league/{league_id}")
        return League.model_validate(data)

    async def get_league_rosters(self, league_id: str) -> List[Roster]:
        """Get all rosters in a league.
        
        Args:
            league_id: League ID to look up rosters for
            
        Returns:
            List[Roster]: List of rosters
        """
        data = await self._make_request("GET", f"/league/{league_id}/rosters")
        return [Roster.model_validate(roster) for roster in data]

    async def get_league_users(self, league_id: str) -> List[User]:
        """Get all users in a league.
        
        Args:
            league_id: League ID to look up users for
            
        Returns:
            List[User]: List of users
        """
        data = await self._make_request("GET", f"/league/{league_id}/users")
        return [User.model_validate(user) for user in data]

    async def get_nfl_state(self) -> NFLState:
        """Get current NFL season state.
        
        Returns:
            NFLState: Current NFL state information
        """
        data = await self._make_request("GET", "/state/nfl")
        return NFLState.model_validate(data)

    async def close(self) -> None:
        """Close the HTTP client session and cache."""
        await self._client.aclose()
        self.cache.clear()

    def get_cache_stats(self) -> Dict[str, Any]:
        """Get cache statistics.
        
        Returns:
            Dict[str, Any]: Cache statistics
        """
        return self.cache.get_stats()

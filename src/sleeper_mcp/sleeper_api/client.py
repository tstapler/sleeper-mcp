"""Sleeper API client implementation."""

import aiohttp
from typing import Optional, Dict, Any, List
from datetime import datetime

class SleeperClient:
    """Async client for interacting with the Sleeper API."""
    
    BASE_URL = "https://api.sleeper.app/v1"
    
    def __init__(self):
        """Initialize the client."""
        self._session: Optional[aiohttp.ClientSession] = None
    
    async def __aenter__(self):
        """Set up async context manager."""
        self._session = aiohttp.ClientSession()
        return self
    
    async def __aexit__(self, exc_type, exc_val, exc_tb):
        """Clean up async context manager."""
        if self._session:
            await self._session.close()
            
    async def _get(self, endpoint: str) -> Dict[str, Any]:
        """Make a GET request to the Sleeper API.
        
        Args:
            endpoint: API endpoint path
            
        Returns:
            Dict[str, Any]: JSON response data
            
        Raises:
            RuntimeError: If client is not used as context manager
            aiohttp.ClientError: On API request failure
        """
        if not self._session:
            raise RuntimeError("Client must be used as async context manager")
        
        async with self._session.get(f"{self.BASE_URL}/{endpoint}") as response:
            response.raise_for_status()
            return await response.json()
    
    # User endpoints
    async def get_user(self, username_or_id: str) -> Dict[str, Any]:
        """Get user information.
        
        Args:
            username_or_id: Sleeper username or user ID
            
        Returns:
            Dict[str, Any]: User data
        """
        return await self._get(f"user/{username_or_id}")
    
    # League endpoints
    async def get_user_leagues(
        self, 
        user_id: str, 
        sport: str = "nfl",
        season: Optional[str] = None
    ) -> List[Dict[str, Any]]:
        """Get user's leagues for a sport and season.
        
        Args:
            user_id: Sleeper user ID
            sport: Sport name (default: "nfl")
            season: Season year (default: current year)
            
        Returns:
            List[Dict[str, Any]]: List of league data
        """
        if not season:
            season = str(datetime.now().year)
        return await self._get(f"user/{user_id}/leagues/{sport}/{season}")
    
    async def get_league(self, league_id: str) -> Dict[str, Any]:
        """Get league information.
        
        Args:
            league_id: Sleeper league ID
            
        Returns:
            Dict[str, Any]: League data
        """
        return await self._get(f"league/{league_id}")
    
    async def get_league_users(self, league_id: str) -> List[Dict[str, Any]]:
        """Get all users in a league.
        
        Args:
            league_id: Sleeper league ID
            
        Returns:
            List[Dict[str, Any]]: List of user data
        """
        return await self._get(f"league/{league_id}/users")
    
    async def get_league_rosters(self, league_id: str) -> List[Dict[str, Any]]:
        """Get all rosters in a league.
        
        Args:
            league_id: Sleeper league ID
            
        Returns:
            List[Dict[str, Any]]: List of roster data
        """
        return await self._get(f"league/{league_id}/rosters")

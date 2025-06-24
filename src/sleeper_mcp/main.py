"""Sleeper MCP FastAPI server implementation."""

from fastapi import FastAPI, HTTPException
from typing import List

from .sleeper_api import SleeperClient
from .sleeper_api.models.base import User, League, Roster

app = FastAPI(
    title="Sleeper MCP",
    description="MCP implementation for Sleeper Fantasy Sports API",
    version="0.1.0"
)

@app.get("/users/{username_or_id}", response_model=User)
async def get_user(username_or_id: str) -> User:
    """Get user information."""
    async with SleeperClient() as client:
        try:
            data = await client.get_user(username_or_id)
            return User(**data)
        except Exception as e:
            raise HTTPException(status_code=500, detail=str(e))

@app.get("/users/{user_id}/leagues", response_model=List[League])
async def get_user_leagues(
    user_id: str,
    sport: str = "nfl",
    season: str | None = None
) -> List[League]:
    """Get user's leagues for a sport and season."""
    async with SleeperClient() as client:
        try:
            data = await client.get_user_leagues(user_id, sport, season)
            return [League(**league_data) for league_data in data]
        except Exception as e:
            raise HTTPException(status_code=500, detail=str(e))

@app.get("/leagues/{league_id}", response_model=League)
async def get_league(league_id: str) -> League:
    """Get league information."""
    async with SleeperClient() as client:
        try:
            data = await client.get_league(league_id)
            return League(**data)
        except Exception as e:
            raise HTTPException(status_code=500, detail=str(e))

@app.get("/leagues/{league_id}/users", response_model=List[User])
async def get_league_users(league_id: str) -> List[User]:
    """Get all users in a league."""
    async with SleeperClient() as client:
        try:
            data = await client.get_league_users(league_id)
            return [User(**user_data) for user_data in data]
        except Exception as e:
            raise HTTPException(status_code=500, detail=str(e))

@app.get("/leagues/{league_id}/rosters", response_model=List[Roster])
async def get_league_rosters(league_id: str) -> List[Roster]:
    """Get all rosters in a league."""
    async with SleeperClient() as client:
        try:
            data = await client.get_league_rosters(league_id)
            return [Roster(**roster_data) for roster_data in data]
        except Exception as e:
            raise HTTPException(status_code=500, detail=str(e))

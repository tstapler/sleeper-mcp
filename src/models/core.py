"""Core data models for the Sleeper MCP server.

This module defines the base data models used throughout the application,
following the OpenAPI specification and providing type safety.
"""

from datetime import datetime
from typing import Dict, List, Optional, Union

from pydantic import BaseModel, Field, HttpUrl


class User(BaseModel):
    """Representation of a Sleeper user."""

    username: str = Field(..., description="User's username")
    user_id: str = Field(..., description="Unique identifier for the user")
    display_name: str = Field(..., description="User's display name")
    avatar: Optional[str] = Field(None, description="User's avatar identifier")

    def get_avatar_url(self, thumbnail: bool = False) -> Optional[HttpUrl]:
        """Get the URL for the user's avatar.
        
        Args:
            thumbnail: Whether to return the thumbnail URL
            
        Returns:
            Optional[HttpUrl]: The avatar URL if available
        """
        if not self.avatar:
            return None
        
        base_url = "https://sleepercdn.com/avatars"
        path = f"thumbs/{self.avatar}" if thumbnail else self.avatar
        return HttpUrl(f"{base_url}/{path}")


class LeagueSettings(BaseModel):
    """League configuration settings."""

    draft_type: str = Field(..., description="Type of draft")
    num_teams: int = Field(..., description="Number of teams in the league")
    scoring_type: Optional[str] = Field(None, description="Scoring type for the league")


class League(BaseModel):
    """Representation of a Sleeper league."""

    league_id: str = Field(..., description="Unique identifier for the league")
    name: str = Field(..., description="League name")
    season: str = Field(..., description="Season year")
    status: str = Field(..., description="League status")
    sport: str = Field(..., description="Sport type")
    settings: LeagueSettings = Field(..., description="League settings")
    total_rosters: int = Field(..., description="Total number of rosters")
    draft_id: Optional[str] = Field(None, description="Associated draft ID")
    avatar: Optional[str] = Field(None, description="League avatar identifier")


class RosterSettings(BaseModel):
    """Settings for a team's roster."""

    wins: int = Field(0, description="Number of wins")
    losses: int = Field(0, description="Number of losses")
    ties: int = Field(0, description="Number of ties")
    fpts: float = Field(0.0, description="Fantasy points")
    fpts_against: float = Field(0.0, description="Fantasy points against")


class Roster(BaseModel):
    """Representation of a team's roster."""

    roster_id: int = Field(..., description="Unique identifier for the roster")
    owner_id: str = Field(..., description="User ID of the owner")
    league_id: str = Field(..., description="League ID this roster belongs to")
    players: List[str] = Field(default_factory=list, description="List of player IDs")
    starters: List[str] = Field(default_factory=list, description="List of starter IDs")
    settings: RosterSettings = Field(
        default_factory=RosterSettings,
        description="Roster settings and statistics"
    )


class Player(BaseModel):
    """Representation of an NFL player."""

    player_id: str = Field(..., description="Unique identifier for the player")
    full_name: str = Field(..., description="Player's full name")
    position: str = Field(..., description="Player's position")
    team: Optional[str] = Field(None, description="Player's NFL team")
    number: Optional[int] = Field(None, description="Player's jersey number")
    status: Optional[str] = Field(None, description="Player's status")


class NFLState(BaseModel):
    """Current state of the NFL season."""

    week: int = Field(..., description="Current week number")
    season_type: str = Field(..., description="Type of season")
    season: str = Field(..., description="Current season year")
    previous_season: str = Field(..., description="Previous season year")
    league_season: str = Field(..., description="League season")
    league_create_season: str = Field(..., description="Season when league was created")
    display_week: int = Field(..., description="Week to display")
